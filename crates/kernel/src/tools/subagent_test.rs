use super::{agent_prefix, subagent_prompt, SubagentTool, SUBAGENT_TOOL_NAME};
use crate::agent::{AgentShared, SubAgentMode};
use crate::comms::{EventBus, InputBus};
use crate::permission::{Level, PermissionState};
use crate::storage::migrations::run_migrations;
use crate::storage::{NewSession, SessionStore, SqliteSessionStore};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::SessionId;
use std::sync::Arc;

#[tokio::test]
async fn schema_does_not_accept_agent_id() {
    let tool = SubagentTool::new(
        std::sync::Arc::new(AgentShared::new(
            Default::default(),
            String::new(),
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
        )),
        InputBus::new(),
        SessionId::from("parent_session"),
    );
    let schema = tool.schema();

    assert_eq!(tool.name(), SUBAGENT_TOOL_NAME);
    assert!(schema["properties"].get("agent_id").is_none());
    assert_eq!(
        schema["required"],
        serde_json::json!(["description", "prompt"])
    );
    assert!(tool
        .desc()
        .contains("background or concurrent collaboration"));
    assert!(tool.desc().contains("`wait_for_completion: false`"));
    assert!(tool
        .desc()
        .contains("You can continue working while the agent runs"));
    assert!(tool
        .desc()
        .contains("returned agent ID with `post_message`"));
    assert!(schema["properties"]["wait_for_completion"]["description"]
        .as_str()
        .is_some_and(|description| description.contains("Whether you wait")));
}

#[tokio::test]
async fn subagent_inherits_current_runtime_auto_approve_level() {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let session_store: Arc<dyn SessionStore> = Arc::new(SqliteSessionStore::new(pool));
    let parent_id = SessionId::from("parent_session");
    session_store
        .create(NewSession {
            auto_approve_level: Some(Level::Safe.as_str().to_string()),
            ..NewSession::new(parent_id.clone())
        })
        .await
        .unwrap();

    let permission_state = PermissionState::new(Level::Safe);
    permission_state
        .set_auto_approve_level(Level::Dangerous)
        .await;
    let event_bus = Arc::new(EventBus::new());
    let shared = Arc::new(
        AgentShared::new(
            Default::default(),
            String::new(),
            None,
            None,
            None,
            Some(Arc::clone(&session_store)),
            None,
            None,
            Some(permission_state),
            Vec::new(),
            None,
            None,
        )
        .with_event_bus(Arc::clone(&event_bus)),
    );
    let input_bus = InputBus::new();
    let mut input_subscriber = input_bus.subscribe_all();
    let tool = SubagentTool::new(shared, input_bus, parent_id.clone());

    let exec = tokio::spawn(async move {
        tool.exec(
            serde_json::json!({
                "description": "Check inheritance",
                "prompt": "Report the inherited permission level.",
                "wait_for_completion": true
            }),
            ToolExecCtx::new("call_1", ".", parent_id.as_str()),
        )
        .await
    });

    let (subagent_id, _) = input_subscriber.recv().await.unwrap();
    let child = session_store.get(&subagent_id).await.unwrap().unwrap();
    assert_eq!(child.auto_approve_level.as_deref(), Some("dangerous"));

    event_bus
        .publish(
            subagent_id.clone(),
            crate::event::Envelope::new(
                subagent_id,
                crate::event::Event::Agent(crate::event::AgentEvent::Lifecycle {
                    state: crate::event::AgentStatus::Stopped {
                        reason: crate::event::StopReason::Completed {
                            finish_reason: None,
                        },
                    },
                }),
            ),
        )
        .unwrap();
    exec.await.unwrap().unwrap();
}

#[test]
fn agent_results_use_the_shared_from_agent_prefix() {
    assert_eq!(
        agent_prefix(&SessionId::from("sub_123"), "Review complete"),
        "[From Agent: sub_123] Review complete"
    );
}

#[test]
fn async_prompt_includes_parent_agent_id_and_post_message_guidance() {
    let prompt = subagent_prompt(
        "Review the implementation.".to_string(),
        SubAgentMode::Async,
        &SessionId::from("parent_session"),
    );

    assert!(prompt.contains("Your parent agent ID is `parent_session`"));
    assert!(prompt.contains("Use the `post_message` tool with this ID"));
    assert!(prompt.ends_with("Review the implementation."));
}

#[test]
fn sync_prompt_is_unchanged() {
    let original = "Review the implementation.";

    assert_eq!(
        subagent_prompt(
            original.to_string(),
            SubAgentMode::Sync,
            &SessionId::from("parent_session"),
        ),
        original
    );
}

// ── template 参数 ───────────────────────────────────────────────────────

struct TemplateFixture {
    tool: SubagentTool,
    session_store: Arc<dyn SessionStore>,
    event_bus: Arc<EventBus>,
    input_bus: Arc<InputBus>,
}

async fn template_fixture(parent_working_dir: Option<&str>) -> (TemplateFixture, SessionId) {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let session_store: Arc<dyn SessionStore> = Arc::new(SqliteSessionStore::new(pool));
    let parent_id = SessionId::from("parent_session");
    session_store
        .create(NewSession {
            working_dir: parent_working_dir.map(str::to_string),
            model_key: Some("parent-model".to_string()),
            ..NewSession::new(parent_id.clone())
        })
        .await
        .unwrap();

    let event_bus = EventBus::new();
    let shared = Arc::new(
        AgentShared::new(
            Default::default(),
            String::new(),
            None,
            None,
            None,
            Some(Arc::clone(&session_store)),
            None,
            None,
            None,
            Vec::new(),
            None,
            None,
        )
        .with_event_bus(Arc::clone(&event_bus)),
    );
    let input_bus = InputBus::new();
    let tool = SubagentTool::new(shared, input_bus.clone(), parent_id.clone());
    (
        TemplateFixture {
            tool,
            session_store,
            event_bus,
            input_bus,
        },
        parent_id,
    )
}

fn output_text(out: &crate::types::ToolOutput) -> String {
    out.contents.iter().filter_map(|b| b.as_text()).collect()
}

/// 驱动一次 sync spawn 直到拿到 subagent id，随后用 Completed 事件收尾。
async fn run_spawn(f: TemplateFixture, parent_id: SessionId, template: &str) -> SessionId {
    let template = template.to_string();
    let mut input_subscriber = f.input_bus.subscribe_all();
    let exec = tokio::spawn(async move {
        f.tool
            .exec(
                serde_json::json!({
                    "description": "templated spawn",
                    "prompt": "do the thing",
                    "wait_for_completion": true,
                    "template": template,
                }),
                ToolExecCtx::new("call_1", ".", parent_id.as_str()),
            )
            .await
    });

    let (subagent_id, _) = input_subscriber.recv().await.unwrap();

    // 让等待中的 exec 收尾
    let event_bus = Arc::clone(&f.event_bus);
    let sid = subagent_id.clone();
    event_bus
        .publish(
            sid.clone(),
            crate::event::Envelope::new(
                sid,
                crate::event::Event::Agent(crate::event::AgentEvent::Lifecycle {
                    state: crate::event::AgentStatus::Stopped {
                        reason: crate::event::StopReason::Completed {
                            finish_reason: None,
                        },
                    },
                }),
            ),
        )
        .unwrap();
    exec.await.unwrap().unwrap();
    subagent_id
}

#[tokio::test]
async fn spawn_with_builtin_template_records_name_and_inherits_model() {
    let (f, parent_id) = template_fixture(None).await;
    let store = Arc::clone(&f.session_store);
    let subagent_id = run_spawn(f, parent_id, "verifier").await;

    let child = store.get(&subagent_id).await.unwrap().unwrap();
    assert_eq!(child.template.as_deref(), Some("verifier"));
    // verifier 不带 model_key → 继承父 session
    assert_eq!(child.model_key.as_deref(), Some("parent-model"));
}

#[tokio::test]
async fn spawn_with_workspace_template_records_name_and_inherits_model() {
    let dir = std::env::temp_dir().join(format!("yomi-subtmpl-test-{}", std::process::id()));
    let role_dir = dir.join(".yomi/agents/fast");
    std::fs::create_dir_all(&role_dir).unwrap();
    // 纯 markdown 格式：全文即角色 SP
    std::fs::write(role_dir.join("ROLE.md"), "你是快速执行者。\n").unwrap();

    let (f, parent_id) = template_fixture(Some(dir.to_str().unwrap())).await;
    let store = Arc::clone(&f.session_store);
    let subagent_id = run_spawn(f, parent_id, "fast").await;

    let child = store.get(&subagent_id).await.unwrap().unwrap();
    assert_eq!(child.template.as_deref(), Some("fast"));
    assert_eq!(child.model_key.as_deref(), Some("parent-model"));

    std::fs::remove_dir_all(&dir).unwrap();
}

#[tokio::test]
async fn unknown_template_errors_with_available_list() {
    let (f, parent_id) = template_fixture(None).await;
    let out = f
        .tool
        .exec(
            serde_json::json!({
                "description": "bad template",
                "prompt": "noop",
                "template": "no-such-role",
            }),
            ToolExecCtx::new("call_1", ".", parent_id.as_str()),
        )
        .await
        .unwrap();

    let text = output_text(&out);
    assert!(text.contains("unknown template 'no-such-role'"));
    assert!(text.contains("verifier (builtin)"));
}
