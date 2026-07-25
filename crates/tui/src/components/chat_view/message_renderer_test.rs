use super::{extract_tool_target, render_message, tool_icon, tool_label, tool_verb};
use crate::components::chat_view::{HistoryMessage, ToolStatus};
use kernel::tools::POST_MESSAGE_TOOL_NAME;

fn edit_tool_msg(
    folded: bool,
    arguments: Option<String>,
    output: Option<String>,
    error: Option<String>,
) -> HistoryMessage {
    HistoryMessage::Tool {
        tool_name: "edit".to_string(),
        tool_id: "call_1".to_string(),
        status: if error.is_some() {
            ToolStatus::Failed
        } else {
            ToolStatus::Completed
        },
        output,
        error,
        folded,
        arguments,
        elapsed_ms: None,
        content_blocks: Vec::new(),
        subagent: None,
    }
}

fn rendered_line_texts(msg: &HistoryMessage) -> Vec<String> {
    render_message(msg, 80)
        .iter()
        .map(|line| {
            line.spans
                .iter()
                .map(|span| span.content.to_string())
                .collect::<String>()
        })
        .collect()
}

#[test]
fn folded_edit_shows_compact_diff_instead_of_output_peek() {
    let args = r#"{"path":"a.rs","old_str":"a\nb\nc","new_str":"a\nx\nc"}"#;
    let msg = edit_tool_msg(true, Some(args.to_string()), Some("ok".to_string()), None);
    let lines = rendered_line_texts(&msg);

    assert!(
        lines.iter().any(|l| l.ends_with("− b")),
        "del line: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l.ends_with("+ x")),
        "add line: {lines:?}"
    );
    // Successful edit: the diff replaces the noisy output peek.
    assert!(!lines.iter().any(|l| l.contains('⎿')), "peek: {lines:?}");
    assert!(!lines.iter().any(|l| l.contains("Arguments:")));
}

#[test]
fn folded_edit_caps_diff_at_ten_lines_with_expand_hint() {
    let old = (1..=8)
        .map(|i| format!("old{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let new = (1..=8)
        .map(|i| format!("new{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let args = serde_json::json!({"path":"a.rs","old_str":old,"new_str":new}).to_string();
    let msg = edit_tool_msg(true, Some(args), None, None);
    let lines = rendered_line_texts(&msg);

    // header + 10 diff lines + 1 overflow hint
    assert_eq!(lines.len(), 12, "{lines:?}");
    assert!(
        lines.last().unwrap().contains("+6 more lines"),
        "hint: {lines:?}"
    );
}

#[test]
fn unfolded_edit_shows_full_diff_without_hint() {
    let old = (1..=8)
        .map(|i| format!("old{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let new = (1..=8)
        .map(|i| format!("new{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    let args = serde_json::json!({"path":"a.rs","old_str":old,"new_str":new}).to_string();
    let msg = edit_tool_msg(false, Some(args), None, None);
    let lines = rendered_line_texts(&msg);

    assert!(!lines.iter().any(|l| l.contains("more lines")));
    assert!(lines.iter().any(|l| l.ends_with("− old8")));
    assert!(lines.iter().any(|l| l.ends_with("+ new8")));
}

#[test]
fn folded_edit_keeps_error_peek_alongside_diff() {
    let args = serde_json::json!({"path":"a.rs","old_str":"a","new_str":"b"}).to_string();
    let msg = edit_tool_msg(
        true,
        Some(args),
        None,
        Some("old_str not found".to_string()),
    );
    let lines = rendered_line_texts(&msg);

    assert!(lines.iter().any(|l| l.ends_with("+ b")));
    assert!(
        lines
            .iter()
            .any(|l| l.contains('⎿') && l.contains("old_str not found")),
        "error peek: {lines:?}"
    );
}

#[test]
fn folded_edit_without_parseable_args_falls_back_to_output_peek() {
    let msg = edit_tool_msg(
        true,
        Some("not json".to_string()),
        Some("done ok".to_string()),
        None,
    );
    let lines = rendered_line_texts(&msg);

    assert!(
        lines
            .iter()
            .any(|l| l.contains('⎿') && l.contains("done ok")),
        "peek: {lines:?}"
    );
}

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

#[test]
fn tool_verb_maps_known_tools() {
    assert_eq!(tool_verb("edit"), "Editing");
    assert_eq!(tool_verb("read"), "Reading");
    assert_eq!(tool_verb("write"), "Writing");
    assert_eq!(tool_verb("shell"), "Running");
    assert_eq!(tool_verb("grep"), "Searching");
    assert_eq!(tool_verb("web_fetch"), "Fetching");
    assert_eq!(tool_verb("agent"), "Delegating");
    assert_eq!(tool_verb("sleep"), "Sleeping");
}

#[test]
fn tool_verb_falls_back_to_calling() {
    assert_eq!(tool_verb("mcp__something"), "Calling");
    assert_eq!(tool_verb("todo"), "Calling");
}

#[test]
fn tool_label_uses_camel_case_for_multi_word_tools() {
    assert_eq!(tool_label("web_search"), "WebSearch");
    assert_eq!(tool_label("web_fetch"), "WebFetch");
    assert_eq!(tool_label("post_message"), "PostMessage");
    assert_eq!(tool_label("ask_user"), "AskUser");
    assert_eq!(tool_label("task_create"), "TaskCreate");
    assert_eq!(tool_label("update_goal"), "UpdateGoal");
    // single-word tools keep their plain label
    assert_eq!(tool_label("read"), "Read");
    // unknown tools are humanized the same way
    assert_eq!(tool_label("my_custom_tool"), "MyCustomTool");
}
