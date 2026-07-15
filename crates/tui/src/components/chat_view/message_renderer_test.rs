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

#[test]
fn tool_aliases_are_case_insensitive_and_compact() {
    let args = r#"{"file_path":"src/lib.rs","mode":"append"}"#;
    assert_eq!(
        extract_tool_target("WRITE_FILE", Some(args)),
        Some("src/lib.rs".to_string())
    );
    assert_eq!(tool_icon("WebSearch"), " ");
}

#[test]
fn camel_case_builtins_extract_targets() {
    assert_eq!(
        extract_tool_target("webSearch", Some(r#"{"query":"rust tui"}"#)),
        Some("rust tui".to_string())
    );
    assert_eq!(
        extract_tool_target("taskUpdate", Some(r#"{"taskId":"task-1"}"#)),
        Some("task-1".to_string())
    );
}
