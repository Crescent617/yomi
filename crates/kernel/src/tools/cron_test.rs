use super::*;

use crate::cron::SqliteCronStore;
use crate::storage::migrations::run_migrations;
use crate::storage::{NewSession, SqliteSessionStore};
use serde_json::json;
use sqlx::sqlite::SqlitePoolOptions;

struct TestFixture {
    tool: CronTool,
    cron_store: Arc<dyn CronStore>,
    session_store: Arc<dyn SessionStore>,
}

async fn fixture(with_session_store: bool, with_input_bus: bool) -> TestFixture {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();

    let cron_store: Arc<dyn CronStore> = Arc::new(SqliteCronStore::new(pool.clone()));
    let session_store: Arc<dyn SessionStore> = Arc::new(SqliteSessionStore::new(pool));
    let input_bus = with_input_bus.then(InputBus::new);

    let tool = CronTool::new(
        Arc::clone(&cron_store),
        Arc::new(std::sync::Mutex::new(None)),
        with_session_store.then(|| Arc::clone(&session_store)),
        input_bus,
        // Fixture plays the "global config = safe" scenario: per-run
        // session templates must be floored to caution.
        crate::permission::Level::Safe,
        std::path::PathBuf::from("/tmp/yomi-cron-test"),
    );

    TestFixture {
        tool,
        cron_store,
        session_store,
    }
}

fn ctx() -> ToolExecCtx<'static> {
    ToolExecCtx::new("tc-1", "/tmp", "sess-1")
}

async fn exec(tool: &CronTool, args: Value) -> Result<ToolOutput> {
    tool.exec(args, ctx()).await
}

fn output_text(out: &ToolOutput) -> String {
    out.contents.iter().filter_map(|b| b.as_text()).collect()
}

#[tokio::test]
async fn create_send_message_without_session_is_per_run() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({
            "action": "create",
            "name": "daily standup",
            "schedule": "0 9 * * *",
            "type": "send_message",
            "content": "Good morning {{date}}",
        }),
    )
    .await
    .unwrap();

    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap();
    // 不绑定固定会话：输出 session_id 为 null，创建时不新建任何 session
    assert!(v["session_id"].is_null());
    assert!(v["next_run_at"].as_str().is_some());
    let (sessions, _) = f
        .session_store
        .list(
            None,
            crate::storage::session::SessionListScope::All,
            None,
            10,
        )
        .await
        .unwrap();
    assert!(sessions.is_empty());

    // 持久化的 job 保持未绑定，并捕获了 per-run 模板（config=safe → 下限 caution）
    let job = f
        .cron_store
        .get(&CronJobId::from(job_id))
        .await
        .unwrap()
        .unwrap();
    let CronAction::SendMessage {
        session_id: None,
        session_template: Some(tpl),
        ..
    } = job.action
    else {
        panic!("expected per-run send_message with template");
    };
    assert_eq!(tpl.auto_approve_level.as_deref(), Some("caution"));
    assert_eq!(job.status, CronJobStatus::Active);
    assert!(job.next_run_at.is_some());
}

#[tokio::test]
async fn create_send_message_per_run_template_follows_context() {
    let f = fixture(true, false).await;
    // Current session carries a working dir, a project and a custom model.
    f.session_store
        .create(NewSession {
            project_id: Some(crate::types::ProjectId::from("proj_1")),
            working_dir: Some("/repo/demo".into()),
            auto_approve_level: Some("caution".into()),
            model_key: Some("custom-model".into()),
            ..NewSession::new(SessionId::from("sess-1"))
        })
        .await
        .unwrap();

    let out = exec(
        &f.tool,
        json!({
            "action": "create",
            "name": "daily standup",
            "schedule": "0 9 * * *",
            "type": "send_message",
            "content": "Good morning",
        }),
    )
    .await
    .unwrap();

    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job = f
        .cron_store
        .get(&CronJobId::from(v["job_id"].as_str().unwrap()))
        .await
        .unwrap()
        .unwrap();
    let CronAction::SendMessage {
        session_template: Some(tpl),
        ..
    } = job.action
    else {
        panic!("expected template on per-run job");
    };

    // working_dir / project 跟随当前 session…（model 不在模板里，天然不继承）
    assert_eq!(tpl.working_dir.as_deref(), Some("/repo/demo"));
    assert_eq!(
        tpl.project_id.as_ref().map(|p| p.0.to_string()).as_deref(),
        Some("proj_1")
    );
}

#[tokio::test]
async fn create_send_message_with_explicit_session_keeps_it() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({
            "action": "create",
            "name": "ping me",
            "schedule": "*/5 * * * *",
            "type": "send_message",
            "content": "ping",
            "session_id": "sess-1",
        }),
    )
    .await
    .unwrap();

    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    assert_eq!(v["session_id"].as_str().unwrap(), "sess-1");

    // No extra session was created.
    assert!(f
        .session_store
        .get(&SessionId::from("sess-1"))
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn create_shell_job() {
    let f = fixture(false, false).await;
    let out = exec(
        &f.tool,
        json!({
            "action": "create",
            "name": "backup",
            "schedule": "0 3 * * *",
            "type": "shell",
            "command": "echo ok",
            "max_runs": 3,
        }),
    )
    .await
    .unwrap();

    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = CronJobId::from(v["job_id"].as_str().unwrap());
    let job = f.cron_store.get(&job_id).await.unwrap().unwrap();
    assert!(matches!(job.action, CronAction::Shell { .. }));
    assert_eq!(job.max_runs, 3);
}

#[tokio::test]
async fn create_validates_input() {
    let f = fixture(true, false).await;

    // Invalid schedule
    assert!(exec(
        &f.tool,
        json!({"action": "create", "name": "x", "schedule": "nope", "type": "shell", "command": "true"}),
    )
    .await
    .is_err());

    // Missing content for send_message
    assert!(exec(
        &f.tool,
        json!({"action": "create", "name": "x", "schedule": "0 9 * * *", "type": "send_message"}),
    )
    .await
    .is_err());

    // Missing command for shell
    assert!(exec(
        &f.tool,
        json!({"action": "create", "name": "x", "schedule": "0 9 * * *", "type": "shell"}),
    )
    .await
    .is_err());

    // Unknown type
    assert!(exec(
        &f.tool,
        json!({"action": "create", "name": "x", "schedule": "0 9 * * *", "type": "http"}),
    )
    .await
    .is_err());

    // Invalid expires_at
    assert!(exec(
        &f.tool,
        json!({"action": "create", "name": "x", "schedule": "0 9 * * *", "type": "shell", "command": "true", "expires_at": "not-a-date"}),
    )
    .await
    .is_err());
}

#[tokio::test]
async fn create_send_message_without_session_store_still_works() {
    // per-run 模板捕获不需要 session store（没有就放弃 follow），
    // 会话到触发时才真正创建。
    let f = fixture(false, false).await;
    let out = exec(
        &f.tool,
        json!({
            "action": "create",
            "name": "x",
            "schedule": "0 9 * * *",
            "type": "send_message",
            "content": "hi",
        }),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    assert!(v["session_id"].is_null());

    let job = f
        .cron_store
        .get(&CronJobId::from(v["job_id"].as_str().unwrap()))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        job.action,
        CronAction::SendMessage {
            session_id: None,
            session_template: Some(_),
            ..
        }
    ));
}

#[tokio::test]
async fn list_jobs_with_status_filter() {
    let f = fixture(true, false).await;
    exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "shell", "command": "true"}),
    )
    .await
    .unwrap();
    exec(
        &f.tool,
        json!({"action": "create", "name": "b", "schedule": "0 10 * * *", "type": "shell", "command": "true"}),
    )
    .await
    .unwrap();

    let out = exec(&f.tool, json!({"action": "list"})).await.unwrap();
    let jobs: Value = serde_json::from_str(&output_text(&out)).unwrap();
    assert_eq!(jobs.as_array().unwrap().len(), 2);

    let out = exec(&f.tool, json!({"action": "list", "status": "paused"}))
        .await
        .unwrap();
    let jobs: Value = serde_json::from_str(&output_text(&out)).unwrap();
    assert_eq!(jobs.as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn update_pause_resume_and_schedule() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "send_message", "content": "hi"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();
    assert!(v["session_id"].is_null());

    // Pause
    exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "status": "paused"}),
    )
    .await
    .unwrap();
    let job = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.status, CronJobStatus::Paused);

    // Resume + change schedule and content; per-run 形态（未绑定 + 模板）保持。
    exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "status": "active", "schedule": "30 8 * * *", "content": "new hi"}),
    )
    .await
    .unwrap();
    let job = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.status, CronJobStatus::Active);
    assert_eq!(job.schedule, "30 8 * * *");
    assert!(job.next_run_at.is_some());
    assert!(matches!(
        job.action,
        CronAction::SendMessage {
            session_id: None,
            ref content,
            session_template: Some(_),
        } if content == "new hi"
    ));

    // Invalid status
    assert!(exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "status": "completed"}),
    )
    .await
    .is_err());

    // Unknown job
    assert!(exec(
        &f.tool,
        json!({"action": "update", "id": "missing", "status": "paused"}),
    )
    .await
    .is_err());
}

#[tokio::test]
async fn delete_job() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "shell", "command": "true"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();

    let out = exec(&f.tool, json!({"action": "delete", "id": job_id}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    assert_eq!(v["deleted"], json!(true));

    let out = exec(&f.tool, json!({"action": "delete", "id": job_id}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    assert_eq!(v["deleted"], json!(false));
}

#[tokio::test]
async fn trigger_shell_executes_without_recording() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "shell", "command": "echo hello-cron"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();

    let out = exec(&f.tool, json!({"action": "trigger", "id": job_id}))
        .await
        .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    assert_eq!(v["triggered"], json!(true));
    assert_eq!(v["stdout"], json!("hello-cron"));

    // Manual triggers are not recorded: run_count / last_run_at stay unset
    // so triggers never consume a job's max_runs budget.
    let job = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.run_count, 0);
    assert!(job.last_run_at.is_none());
    assert!(job.last_error.is_none());
}

#[tokio::test]
async fn trigger_shell_failure_is_not_recorded() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "shell", "command": "echo oops >&2; exit 1"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();

    assert!(exec(&f.tool, json!({"action": "trigger", "id": job_id}))
        .await
        .is_err());

    let job = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.run_count, 0);
    assert!(job.last_error.is_none());
}

#[tokio::test]
async fn trigger_send_message_publishes_to_bus() {
    let f = fixture(true, true).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "send_message", "content": "wake up", "session_id": "sess-target"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();

    let mut rx = f.tool.input_bus.as_ref().unwrap().subscribe_all();
    exec(&f.tool, json!({"action": "trigger", "id": job_id}))
        .await
        .unwrap();

    let (sid, input) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sid, SessionId::from("sess-target"));
    assert!(matches!(input, AgentInput::User { .. }));
}

#[tokio::test]
async fn trigger_send_message_without_input_bus_errors() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "send_message", "content": "hi", "session_id": "sess-1"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();

    assert!(exec(&f.tool, json!({"action": "trigger", "id": job_id}))
        .await
        .is_err());
}

#[tokio::test]
async fn unknown_action_errors() {
    let f = fixture(true, false).await;
    assert!(exec(&f.tool, json!({"action": "bogus"})).await.is_err());
    assert!(exec(&f.tool, json!({})).await.is_err());
}

#[tokio::test]
async fn update_rejects_never_firing_schedule() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "shell", "command": "true"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();
    let before = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();

    // 2 月 31 日不存在：update 必须报错，且 next_run_at 保持原值
    let err = exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "schedule": "0 0 0 31 2 *"}),
    )
    .await;
    assert!(err.is_err());

    let after = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.schedule, before.schedule);
    assert_eq!(after.next_run_at, before.next_run_at);
}

#[tokio::test]
async fn update_clears_max_runs_and_expires_at_with_null() {
    let f = fixture(true, false).await;
    let future = (chrono::Utc::now() + chrono::Duration::days(1)).to_rfc3339();
    let out = exec(
        &f.tool,
        json!({
            "action": "create", "name": "a", "schedule": "0 9 * * *",
            "type": "shell", "command": "true",
            "max_runs": 5, "expires_at": future,
        }),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();

    let job = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.max_runs, 5);
    assert!(job.has_expiry());

    exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "max_runs": null, "expires_at": null}),
    )
    .await
    .unwrap();

    let job = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.max_runs, crate::cron::UNLIMITED_MAX_RUNS);
    assert_eq!(job.expires_at, crate::cron::NEVER_EXPIRES);
}

#[tokio::test]
async fn update_rejects_action_fields_of_wrong_type() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "shell", "command": "true"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();

    // shell job 不接受 content/session_id
    assert!(exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "content": "hi"})
    )
    .await
    .is_err());

    // send_message job 不接受 command
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "b", "schedule": "0 9 * * *", "type": "send_message", "content": "hi"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let msg_job_id = v["job_id"].as_str().unwrap().to_string();
    assert!(exec(
        &f.tool,
        json!({"action": "update", "id": msg_job_id, "command": "true"})
    )
    .await
    .is_err());
}

#[tokio::test]
async fn update_session_id_bind_and_unbind() {
    let f = fixture(true, false).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "send_message", "content": "hi"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();

    // 绑定到固定会话：session_id 落库，模板清掉
    exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "session_id": "sess-x"}),
    )
    .await
    .unwrap();
    let job = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        job.action,
        CronAction::SendMessage {
            session_id: Some(ref sid),
            session_template: None,
            ..
        } if sid == "sess-x"
    ));

    // 显式 null 解绑：回到 per-run，现场补抓模板
    exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "session_id": null}),
    )
    .await
    .unwrap();
    let job = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        job.action,
        CronAction::SendMessage {
            session_id: None,
            session_template: Some(_),
            ..
        }
    ));

    // 省略 session_id 的 action 编辑不动绑定状态（仍 per-run）
    exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "content": "new"}),
    )
    .await
    .unwrap();
    let job = f
        .cron_store
        .get(&CronJobId::from(job_id.as_str()))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        job.action,
        CronAction::SendMessage {
            session_id: None,
            ref content,
            session_template: Some(_),
        } if content == "new"
    ));

    // 非字符串非 null 的 session_id 直接报错
    assert!(exec(
        &f.tool,
        json!({"action": "update", "id": job_id, "session_id": 123}),
    )
    .await
    .is_err());
}

#[tokio::test]
async fn update_sessionless_legacy_job_captures_template_without_store() {
    let f = fixture(false, false).await;
    // 未绑定且缺模板的 job 只会来自旧版本或外部写入。
    let job = crate::cron::CronJob {
        id: CronJobId::new(),
        name: "legacy".to_string(),
        schedule: "0 9 * * *".to_string(),
        action: CronAction::SendMessage {
            session_id: None,
            content: "hi".to_string(),
            session_template: None,
        },
        status: CronJobStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        next_run_at: None,
        last_run_at: None,
        run_count: 0,
        max_runs: 0,
        expires_at: crate::cron::NEVER_EXPIRES,
        last_error: None,
    };
    f.cron_store.create(&job).await.unwrap();

    // action 编辑保持 per-run，并现场补抓模板（没有 session store 也能
    // 兜底出 caution 级默认模板，不再报错）
    exec(
        &f.tool,
        json!({"action": "update", "id": job.id.0.to_string(), "content": "new"}),
    )
    .await
    .unwrap();
    let job = f.cron_store.get(&job.id).await.unwrap().unwrap();
    assert!(matches!(
        job.action,
        CronAction::SendMessage {
            session_id: None,
            session_template: Some(ref tpl),
            ..
        } if tpl.auto_approve_level.as_deref() == Some("caution")
    ));
}

#[tokio::test]
async fn trigger_per_run_send_message_spawns_fresh_session_each_time() {
    let f = fixture(true, true).await;
    let out = exec(
        &f.tool,
        json!({"action": "create", "name": "a", "schedule": "0 9 * * *", "type": "send_message", "content": "wake up"}),
    )
    .await
    .unwrap();
    let v: Value = serde_json::from_str(&output_text(&out)).unwrap();
    let job_id = v["job_id"].as_str().unwrap().to_string();

    let mut rx = f.tool.input_bus.as_ref().unwrap().subscribe_all();
    exec(&f.tool, json!({"action": "trigger", "id": job_id}))
        .await
        .unwrap();
    exec(&f.tool, json!({"action": "trigger", "id": job_id}))
        .await
        .unwrap();

    let (sid1, _) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let (sid2, _) = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .unwrap()
        .unwrap();

    // 两次触发 → 两个不同的新 session，都已落库并以 job 名开头命名
    assert_ne!(sid1.0, sid2.0);
    for sid in [&sid1, &sid2] {
        let info = f.session_store.get(sid).await.unwrap().unwrap();
        assert!(info.title.as_deref().unwrap().starts_with("a · "));
        assert_eq!(info.auto_approve_level.as_deref(), Some("caution"));
    }
}

#[tokio::test]
async fn create_with_existing_name_returns_existing_job_unchanged() {
    let f = fixture(false, false).await;

    let first = exec(
        &f.tool,
        json!({
            "action": "create",
            "name": "janitor",
            "schedule": "0 9 * * *",
            "type": "shell",
            "command": "true",
        }),
    )
    .await
    .unwrap();
    let v1: Value = serde_json::from_str(&output_text(&first)).unwrap();
    assert_eq!(v1["created"], json!(true));

    // 同名再 create：返回同一个 job id，schedule/命令均不被改写
    let second = exec(
        &f.tool,
        json!({
            "action": "create",
            "name": "janitor",
            "schedule": "0 10 * * *",
            "type": "shell",
            "command": "false",
        }),
    )
    .await
    .unwrap();
    let v2: Value = serde_json::from_str(&output_text(&second)).unwrap();
    assert_eq!(v2["created"], json!(false));
    assert_eq!(v2["job_id"], v1["job_id"]);

    let jobs = f.cron_store.list(None, 10).await.unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0].schedule, "0 9 * * *");
}
