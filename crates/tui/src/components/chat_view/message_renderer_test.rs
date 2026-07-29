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

mod thinking {
    use super::super::render_thinking_lines;
    use tuirealm::ratatui::style::Modifier;
    use unicode_width::UnicodeWidthStr;

    fn line_text(line: &tuirealm::ratatui::text::Line) -> String {
        line.spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>()
    }

    fn numbered_thinking(n: usize) -> String {
        (1..=n)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
    #[test]
    fn folded_row_count_is_capped_and_matches_non_empty_lines() {
        for (n, expected) in [(1, 1), (2, 2), (4, 4), (5, 4), (7, 4), (20, 4)] {
            let streaming = render_thinking_lines(&numbered_thinking(n), true, None, true, 80);
            assert_eq!(
                streaming.len(),
                expected,
                "streaming folded rows must be min(non_empty, 3) (n={n})"
            );
            let completed =
                render_thinking_lines(&numbered_thinking(n), true, Some(8200), false, 80);
            assert_eq!(
                completed.len(),
                expected,
                "completed folded rows must match streaming (n={n})"
            );
        }
    }

    #[test]
    fn folded_stats_are_inline_on_last_row() {
        let lines = render_thinking_lines(&numbered_thinking(5), true, Some(8200), false, 80);
        let last = line_text(&lines[lines.len() - 1]);
        assert!(last.contains("line 5"), "content missing: {last}");
        assert!(last.contains("tokens"), "stats missing: {last}");
        assert!(last.contains("8.2s"), "elapsed missing: {last}");
        // Streaming shows no stats.
        let streaming = render_thinking_lines(&numbered_thinking(5), true, None, true, 80);
        assert!(!line_text(&streaming[streaming.len() - 1]).contains("tokens"));
    }

    #[test]
    fn folded_preview_shows_last_non_empty_lines() {
        let thinking = "l1\nl2\nl3\nl4\nl5\nl6\nl7\n\n";
        let lines = render_thinking_lines(thinking, true, None, true, 80);
        assert_eq!(lines.len(), 4);
        assert!(line_text(&lines[0]).contains("l4"));
        assert!(line_text(&lines[1]).contains("l5"));
        assert!(line_text(&lines[2]).contains("l6"));
        assert!(line_text(&lines[3]).contains("l7"));
    }

    #[test]
    fn folded_preview_filters_empty_lines() {
        // Interior and trailing empty lines never produce rows.
        let thinking = "one\n\n\n two \n\n";
        let lines = render_thinking_lines(thinking, true, None, true, 80);
        assert_eq!(lines.len(), 2);
        assert!(line_text(&lines[0]).contains("one"));
        assert!(line_text(&lines[1]).contains("two"));
    }

    #[test]
    fn folded_rows_never_exceed_width() {
        let long = "很长的思考内容".repeat(30);
        let thinking = format!("short\n{long}\nmedium length line");
        for width in [20, 40, 80, 120] {
            // Completed state includes inline stats — the worst case.
            let lines = render_thinking_lines(&thinking, true, Some(8200), false, width);
            for (i, line) in lines.iter().enumerate() {
                let w = UnicodeWidthStr::width(line_text(line).as_str());
                assert!(
                    w <= width,
                    "row {i} width {w} exceeds {width}: {:?}",
                    line_text(line)
                );
            }
        }
    }

    #[test]
    fn expanded_renders_all_lines() {
        let lines = render_thinking_lines("a\nb\nc\nd\ne", false, Some(100), false, 80);
        assert_eq!(lines.len(), 5);
    }

    #[test]
    fn thinking_content_is_italic() {
        let lines = render_thinking_lines("some thought", true, Some(100), false, 80);
        let last = &lines[lines.len() - 1];
        // Content span and inline stats span must both be italic.
        assert!(
            last.spans[1].style.add_modifier.contains(Modifier::ITALIC),
            "thinking content must be italic to distinguish from tool output"
        );
        assert!(last.spans[2].style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn whitespace_only_thinking() {
        // Streaming: nothing to show. Completed: stats-only row.
        assert!(render_thinking_lines("  \n\n", true, None, true, 80).is_empty());
        let lines = render_thinking_lines("  \n\n", true, Some(100), false, 80);
        assert_eq!(lines.len(), 1);
        assert!(line_text(&lines[0]).contains("tokens"));
    }

    #[test]
    fn empty_thinking_renders_nothing() {
        assert!(render_thinking_lines("", true, None, true, 80).is_empty());
        assert!(render_thinking_lines("", false, None, false, 80).is_empty());
    }
}

#[test]
fn user_message_pads_background_to_full_width() {
    use kernel::types::ContentBlock;
    use unicode_width::UnicodeWidthStr;

    let msg = HistoryMessage::User(vec![ContentBlock::Text {
        text: "hello world".to_string(),
    }]);
    let lines = render_message(&msg, 80);
    assert_eq!(lines.len(), 1);

    let line = &lines[0];
    let total: usize = line
        .spans
        .iter()
        .map(|s| UnicodeWidthStr::width(s.content.as_ref()))
        .sum();
    assert_eq!(total, 80, "user line must fill the full row width");

    let last = line.spans.last().unwrap();
    assert!(
        last.content.chars().all(|c| c == ' '),
        "padding span must be spaces: {:?}",
        last.content
    );
    assert_eq!(
        last.style.bg,
        Some(crate::theme::colors::user_msg_bg()),
        "padding must carry the user message background"
    );
}

#[test]
fn folded_peek_shows_first_two_real_output_lines() {
    let msg = HistoryMessage::Tool {
        tool_name: "shell".to_string(),
        tool_id: "call_1".to_string(),
        status: ToolStatus::Completed,
        output: Some("\nfirst line\nsecond line\nthird line\nfourth line\n".to_string()),
        error: None,
        folded: true,
        arguments: Some(r#"{"command":"ls"}"#.to_string()),
        elapsed_ms: None,
        content_blocks: Vec::new(),
        subagent: None,
    };
    let lines = rendered_line_texts(&msg);

    assert!(
        lines
            .iter()
            .any(|l| l.contains('⎿') && l.contains("first line")),
        "first peek line: {lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("second line") && l.contains("+2")),
        "second peek line with overflow hint: {lines:?}"
    );
    // Structure is preserved: no whitespace-collapsed single line.
    assert!(!lines.iter().any(|l| l.contains("first line second line")));
}

#[test]
fn folded_peek_expands_tabs_to_spaces() {
    let msg = HistoryMessage::Tool {
        tool_name: "shell".to_string(),
        tool_id: "call_1".to_string(),
        status: ToolStatus::Completed,
        output: Some("col1\tcol2".to_string()),
        error: None,
        folded: true,
        arguments: Some(r#"{"command":"ls"}"#.to_string()),
        elapsed_ms: None,
        content_blocks: Vec::new(),
        subagent: None,
    };
    let lines = rendered_line_texts(&msg);

    assert!(
        lines
            .iter()
            .any(|l| l.contains('⎿') && l.contains("col1  col2")),
        "peek line expands tabs: {lines:?}"
    );
    assert!(
        !lines.iter().any(|l| l.contains('\t')),
        "no raw tab in rendered lines: {lines:?}"
    );
}

#[test]
fn edit_header_shows_diff_stats() {
    let args = r#"{"path":"a.rs","old_str":"a\nb\nc","new_str":"a\nx\nc"}"#;
    let msg = edit_tool_msg(true, Some(args.to_string()), Some("ok".to_string()), None);
    let lines = rendered_line_texts(&msg);
    let header = &lines[0];

    assert!(header.contains("+1"), "add stats: {header}");
    assert!(header.contains("\u{2212}1"), "del stats: {header}");
}
