use super::{agent_prefix, subagent_prompt, SubagentTool, SUBAGENT_TOOL_NAME};
use crate::agent::{AgentShared, SubAgentMode};
use crate::comms::{EventBus, InputBus};
use crate::permission::{Level, PermissionState};
use crate::storage::migrations::run_migrations;
use crate::storage::{SessionStore, SqliteSessionStore};
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
    assert!(tool.desc().contains("returned agent ID with `postMessage`"));
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
        .create(
            &parent_id,
            None,
            None,
            Some(Level::Safe.as_str()),
            None,
            None,
        )
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
    assert!(prompt.contains("Use the `postMessage` tool with this ID"));
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
