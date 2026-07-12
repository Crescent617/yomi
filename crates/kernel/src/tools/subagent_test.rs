use super::{subagent_prompt, SubagentTool, SUBAGENT_TOOL_NAME};
use crate::agent::{AgentShared, SubAgentMode};
use crate::comms::InputBus;
use crate::tools::Tool;
use crate::types::SessionId;

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
