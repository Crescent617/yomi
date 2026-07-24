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
fn cron_uses_action_as_target_and_clock_icon() {
    let args = r#"{"action":"create","name":"daily","schedule":"0 9 * * 1-5"}"#;
    assert_eq!(
        extract_tool_target("cron", Some(args)),
        Some("create".to_string())
    );
    assert_eq!(tool_icon("cron"), "󰥔 ");
}

#[test]
fn cron_metadata_summarizes_args() {
    let summary = super::tool_header_summary(
        "cron",
        Some(
            r#"{"action":"create","name":"daily","schedule":"0 9 * * 1-5","type":"shell","command":"make report","max_runs":5}"#,
        ),
    );
    assert_eq!(summary.label, "Cron");
    assert_eq!(
        summary.metadata.as_deref(),
        Some("daily · 0 9 * * 1-5 · shell · max 5")
    );

    let update = super::tool_header_summary(
        "cron",
        Some(r#"{"action":"update","id":"cron_1","status":"paused"}"#),
    );
    assert_eq!(update.metadata.as_deref(), Some("cron_1 · → paused"));
}

#[test]
fn snake_case_builtins_extract_targets() {
    assert_eq!(
        extract_tool_target("web_search", Some(r#"{"query":"rust tui"}"#)),
        Some("rust tui".to_string())
    );
    assert_eq!(
        extract_tool_target("task_update", Some(r#"{"taskId":"task-1"}"#)),
        Some("task-1".to_string())
    );
}
