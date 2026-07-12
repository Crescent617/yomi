use serde_json::json;

use super::{format_message, PostMessageTool, POST_MESSAGE_TOOL_NAME};
use crate::agent::AgentInput;
use crate::comms::InputBus;
use crate::storage::migrations::run_migrations;
use crate::storage::{SessionStore, SqliteSessionStore};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{ContentBlock, SessionId, ToolOutputBlock};

async fn session_store_with(id: &SessionId) -> std::sync::Arc<dyn SessionStore> {
    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let store = SqliteSessionStore::new(pool);
    store
        .create(id, None, None, None, None, None)
        .await
        .unwrap();
    std::sync::Arc::new(store)
}

#[tokio::test]
async fn test_definition() {
    let tool = PostMessageTool::new(InputBus::new(), None);

    assert_eq!(tool.name(), POST_MESSAGE_TOOL_NAME);
    assert!(tool.desc().contains("another agent by its ID"));
    assert!(tool
        .desc()
        .contains("current session ID identified as the sender"));
    assert_eq!(
        tool.schema()["required"],
        json!(["agent_id", "title", "content"])
    );
    assert_eq!(tool.schema()["additionalProperties"], false);
}

#[test]
fn test_format_message() {
    assert_eq!(
        format_message("parent_session", "Review complete", "Found two issues."),
        "[From Agent: parent_session] Review complete\nFound two issues."
    );
}

#[tokio::test]
async fn test_exec_publishes_steer_to_target_agent() {
    let bus = InputBus::new();
    let mut subscriber = bus.subscribe(SessionId::from("sub_123"));
    let tool = PostMessageTool::new(bus, None);

    let output = tool
        .exec(
            json!({
                "agent_id": "sub_123",
                "title": "Review complete",
                "content": "Found two issues."
            }),
            ToolExecCtx::new("call_1", ".", "parent_session"),
        )
        .await
        .unwrap();

    assert!(matches!(
        output.contents.as_slice(),
        [ToolOutputBlock::Text { text }] if text == "Message sent to agent sub_123"
    ));

    let (session_id, input) = subscriber.recv().await.unwrap();
    assert_eq!(session_id, SessionId::from("sub_123"));
    assert!(matches!(
        input,
        AgentInput::Steer(content)
            if content == vec![ContentBlock::Text {
                text: "[From Agent: parent_session] Review complete\nFound two issues.".to_string(),
            }]
    ));
}

#[tokio::test]
async fn test_exec_rejects_empty_agent_id() {
    let tool = PostMessageTool::new(InputBus::new(), None);

    let error = tool
        .exec(
            json!({
                "agent_id": "  ",
                "title": "No recipient",
                "content": "This must not be delivered."
            }),
            ToolExecCtx::new("call_1", ".", "parent_session"),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("agent_id must not be empty"));
}

#[tokio::test]
async fn test_exec_rejects_unknown_agent_when_session_store_is_available() {
    let existing = SessionId::from("sub_existing");
    let store = session_store_with(&existing).await;
    let tool = PostMessageTool::new(InputBus::new(), Some(store));

    let error = tool
        .exec(
            json!({
                "agent_id": "sub_unknown",
                "title": "No recipient",
                "content": "This must not be delivered."
            }),
            ToolExecCtx::new("call_1", ".", "parent_session"),
        )
        .await
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("Agent 'sub_unknown' does not exist"));
}

#[tokio::test]
async fn test_exec_rejects_missing_required_argument() {
    let tool = PostMessageTool::new(InputBus::new(), None);

    let error = tool
        .exec(
            json!({
                "agent_id": "sub_123",
                "title": "Missing content"
            }),
            ToolExecCtx::new("call_1", ".", "parent_session"),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("Invalid post_message arguments"));
}
