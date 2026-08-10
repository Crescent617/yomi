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
        // Fixture plays the "global config = safe" scenario: bound sessions
        // must be floored to caution.
        crate::permission::Level::Safe,
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
async fn create_send_message_without_session_binds_new_session() {
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
    let session_id = v["session_id"].as_str().unwrap();
    assert!(v["next_run_at"].as_str().is_some());

    // New session exists, titled after the job.
    let info = f
        .session_store
        .get(&SessionId::from(session_id))
        .await
        .unwrap()
        .expect("session should exist");
    assert_eq!(info.title.as_deref(), Some("daily standup"));
    // Config says safe, but unattended cron sessions floor at caution.
    assert_eq!(info.auto_approve_level.as_deref(), Some("caution"));

    // Persisted job references the new session concretely.
    let job = f
        .cron_store
        .get(&CronJobId::from(job_id))
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        job.action,
        CronAction::SendMessage {
            session_id: Some(ref sid),
            ..
        } if sid == session_id
    ));
    assert_eq!(job.status, CronJobStatus::Active);
    assert!(job.next_run_at.is_some());
}

#[tokio::test]
async fn create_send_message_new_session_follows_context_but_not_model() {
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
    let session_id = v["session_id"].as_str().unwrap();
    let info = f
        .session_store
        .get(&SessionId::from(session_id))
        .await
        .unwrap()
        .unwrap();

    // working_dir / project follow the current session...
    assert_eq!(info.working_dir.as_deref(), Some("/repo/demo"));
    assert_eq!(
        info.project_id.map(|p| p.0.to_string()).as_deref(),
        Some("proj_1")
    );
    // ...model does not: stays unset so the default model is used.
    assert_eq!(info.model_key, None);
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
async fn create_send_message_without_session_store_errors() {
    let f = fixture(false, false).await;
    let err = exec(
        &f.tool,
        json!({
            "action": "create",
            "name": "x",
            "schedule": "0 9 * * *",
            "type": "send_message",
            "content": "hi",
        }),
    )
    .await;
    assert!(err.is_err());
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
    let session_id = v["session_id"].as_str().unwrap().to_string();

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

    // Resume + change schedule and content; session binding is preserved.
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
            session_id: Some(ref sid),
            ref content,
        } if *sid == session_id && content == "new hi"
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
async fn update_sessionless_send_message_errors_without_session_store() {
    let f = fixture(false, false).await;
    // A job with no bound session can only come from older or external
    // writes (both create paths either bind or require an explicit id).
    let job = crate::cron::CronJob {
        id: CronJobId::new(),
        name: "legacy".to_string(),
        schedule: "0 9 * * *".to_string(),
        action: CronAction::SendMessage {
            session_id: None,
            content: "hi".to_string(),
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

    // An action edit would leave the job sessionless and failing every
    // fire — with no session store available it must error explicitly,
    // mirroring the create path.
    let err = exec(
        &f.tool,
        json!({"action": "update", "id": job.id.0.to_string(), "content": "new"}),
    )
    .await
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("session store not available; pass session_id explicitly"),
        "unexpected error: {err}"
    );
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
