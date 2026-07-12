use super::{extract_tool_target, tool_icon};
use kernel::tools::POST_MESSAGE_TOOL_NAME;

#[test]
fn post_message_uses_recipient_as_target() {
    let args = r#"{"agent_id":"子代理-123\n伪造标题","title":"完成","content":"结果"}"#;

    assert_eq!(
        extract_tool_target(POST_MESSAGE_TOOL_NAME, Some(args)),
        Some("子代理-123 伪造标题".to_string())
    );
}

#[test]
fn post_message_has_message_icon() {
    assert_eq!(tool_icon(POST_MESSAGE_TOOL_NAME), "󰍩 ");
}
