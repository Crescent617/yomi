use super::*;

fn buffer_with_run() -> RunReplyBuffer {
    let mut buf = RunReplyBuffer::new();
    buf.record_text("Let me look at the code.".to_string());
    buf.record_tool_start("t1", "read", Some(r#"{"path":"crates/kernel/src/hub.rs"}"#));
    buf.record_tool_end("t1", 120, false);
    buf.record_tool_start("t2", "shell", Some(r#"{"command":"cargo test -p kernel"}"#));
    buf.record_tool_end("t2", 65_000, false);
    buf.record_text("All tests pass.".to_string());
    buf
}

#[test]
fn into_reply_promotes_last_text_to_body() {
    let reply = buffer_with_run().into_reply();
    assert_eq!(reply.text.as_deref(), Some("All tests pass."));
    // The earlier text stays in the trace, chronologically before the tools.
    assert_eq!(reply.entries.len(), 3);
    assert!(matches!(reply.entries[0], TraceEntry::Narration(_)));
    assert!(matches!(reply.entries[1], TraceEntry::Tool(_)));
    assert!(matches!(reply.entries[2], TraceEntry::Tool(_)));
}

#[test]
fn into_reply_without_any_text_keeps_trace_only() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "read", None);
    buf.record_tool_end("t1", 10, false);
    let reply = buf.into_reply();
    assert_eq!(reply.text(), None);
    assert!(reply.has_trace());
}

#[test]
fn into_reply_after_cancel_mid_tool_uses_last_text() {
    // Cancel mid-tool: the run ends with a pending tool entry; the last
    // text still becomes the body (design: /stop still flushes).
    let mut buf = RunReplyBuffer::new();
    buf.record_text("Working on it.".to_string());
    buf.record_tool_start("t1", "shell", Some(r#"{"command":"sleep 100"}"#));
    let reply = buf.into_reply();
    assert_eq!(reply.text.as_deref(), Some("Working on it."));
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("⏳"), "pending tool gets the hourglass icon");
}

#[test]
fn tool_end_matches_by_tool_id() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "read", None);
    buf.record_tool_start("t2", "read", None);
    buf.record_tool_end("t1", 5, true);
    let reply = {
        buf.record_text("done".to_string());
        buf.into_reply()
    };
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("❌"));
    assert!(card.contains("⏳"), "t2 is still pending");
    assert!(card.contains("1 failed"));
}

#[test]
fn summarize_args_prefers_tool_primary_key() {
    assert_eq!(
        summarize_args("shell", Some(r#"{"command":"ls -la","timeout":30}"#)),
        "ls -la"
    );
    assert_eq!(
        summarize_args("read", Some(r#"{"path":"/tmp/a.rs","offset":2}"#)),
        "/tmp/a.rs"
    );
    assert_eq!(
        summarize_args("write", Some(r#"{"file_path":"/tmp/b.rs","content":"…"}"#)),
        "/tmp/b.rs"
    );
    assert_eq!(
        summarize_args("web_search", Some(r#"{"query":"rust async"}"#)),
        "rust async"
    );
}

#[test]
fn summarize_args_falls_back_to_known_keys_and_raw() {
    // Unknown tool with a known key.
    assert_eq!(
        summarize_args("cron", Some(r#"{"command":"echo hi","schedule":"*"}"#)),
        "echo hi"
    );
    // Unknown tool, unknown keys → empty (no meaningful summary).
    assert_eq!(summarize_args("todo", Some(r#"{"items":[]}"#)), "");
    // Non-JSON payload → truncated raw.
    assert_eq!(summarize_args("shell", Some("not json")), "not json");
    // Missing arguments.
    assert_eq!(summarize_args("shell", None), "");
    // Multi-line values are flattened for the one-line display.
    assert_eq!(
        summarize_args("shell", Some(r#"{"command":"line1\nline2"}"#)),
        "line1 line2"
    );
}

#[test]
fn summarize_args_truncates_long_values() {
    let long = "x".repeat(200);
    let args = format!(r#"{{"command":"{long}"}}"#);
    let summary = summarize_args("shell", Some(&args));
    assert_eq!(summary.chars().count(), ARG_SUMMARY_MAX_CHARS + 1); // +1 for …
    assert!(summary.ends_with('…'));
}

#[test]
fn render_card_structure() {
    let reply = buffer_with_run().into_reply();
    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();

    assert_eq!(card["schema"], "2.0");
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 2);
    assert_eq!(elements[0]["tag"], "markdown");
    assert_eq!(elements[0]["content"], "All tests pass.");

    let panel = &elements[1];
    assert_eq!(panel["tag"], "collapsible_panel");
    assert_eq!(panel["expanded"], false);
    let title = panel["header"]["title"]["content"].as_str().unwrap();
    assert!(title.contains("Run trace · 2 tools"), "title: {title}");

    let body = panel["elements"][0]["content"].as_str().unwrap();
    assert!(body.contains("💬 Let me look at the code."));
    assert!(body.contains("✅ **read** · `crates/kernel/src/hub.rs` · 120ms"));
    assert!(body.contains("✅ **shell** · `cargo test -p kernel` · 1m05s"));
}

#[test]
fn render_card_without_trace_is_a_single_markdown_element() {
    let mut buf = RunReplyBuffer::new();
    buf.record_text("Just an answer.".to_string());
    let reply = buf.into_reply();
    assert!(!reply.has_trace());

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0]["content"], "Just an answer.");
}

#[test]
fn render_card_truncates_oversized_text() {
    let mut buf = RunReplyBuffer::new();
    buf.record_text("x".repeat(FINAL_TEXT_MAX_BYTES + 100));
    let reply = buf.into_reply();
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("...(内容已截断)"));
}

#[test]
fn render_trace_caps_entries_and_notes_dropped() {
    let mut buf = RunReplyBuffer::new();
    for i in 0..30 {
        buf.record_tool_start(&format!("t{i}"), "read", None);
        buf.record_tool_end(&format!("t{i}"), 1, false);
    }
    buf.record_text("done".to_string());
    let reply = buf.into_reply();

    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("··· and 10 earlier entries"));
    let panel_body = serde_json::from_str::<serde_json::Value>(&card).unwrap();
    let body = panel_body["body"]["elements"][1]["elements"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(body.lines().count(), MAX_TRACE_ENTRIES + 1); // +1 marker line
}

#[test]
fn buffer_drops_oldest_entries_beyond_cap() {
    let mut buf = RunReplyBuffer::new();
    for i in 0..(BUFFER_MAX_ENTRIES + 20) {
        buf.record_text(format!("text {i}"));
    }
    let reply = buf.into_reply();
    // The latest text survives as the body even though old ones were dropped.
    assert_eq!(
        reply.text.as_deref(),
        Some(format!("text {}", BUFFER_MAX_ENTRIES + 19)).as_deref()
    );
    assert!(reply.entries.len() <= BUFFER_MAX_ENTRIES);
}

#[test]
fn render_plain_appends_trace_without_markup() {
    let reply = buffer_with_run().into_reply();
    let out = render_plain(&reply);
    assert!(out.starts_with("All tests pass."));
    assert!(out.contains("🐾 Run trace · 2 tools"));
    assert!(out.contains("💬 Let me look at the code."));
    assert!(out.contains("✅ shell · cargo test -p kernel · 1m05s"));
    assert!(!out.contains("<font"), "no Feishu markup in plain fallback");
    assert!(!out.contains("**"), "no markdown bold in plain fallback");
    assert!(
        !out.contains('`'),
        "no markdown backticks in plain fallback"
    );
}

#[test]
fn render_plain_without_trace_returns_text_only() {
    let mut buf = RunReplyBuffer::new();
    buf.record_text("plain answer".to_string());
    let reply = buf.into_reply();
    assert_eq!(render_plain(&reply), "plain answer");
}

#[test]
fn into_text_returns_bare_body() {
    let reply = buffer_with_run().into_reply();
    assert_eq!(reply.into_text().as_deref(), Some("All tests pass."));
}

#[test]
fn render_card_with_notice_prepends_notice_line() {
    let reply = buffer_with_run().into_reply();
    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, Some("❌ **Error**  boom")).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 3);
    assert_eq!(elements[0]["content"], "❌ **Error**  boom");
    assert_eq!(elements[1]["content"], "All tests pass.");
    // No card header — the final card is a pure content card.
    assert!(card["header"].is_null());
}

#[test]
fn render_card_returns_none_when_nothing_to_show() {
    let reply = RunReplyBuffer::new().into_reply();
    assert!(!reply.has_trace());
    assert_eq!(reply.text(), None);
    assert!(render_card(&reply, None).is_none());
    // A notice alone is enough to render (failure explanation).
    let card = render_card(&reply, Some("❌ **Error**  boom")).unwrap();
    assert!(card.contains("boom"));
}

#[test]
fn render_plain_without_text_shows_trace_only() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "read", Some(r#"{"path":"/tmp/a.rs"}"#));
    buf.record_tool_end("t1", 5, false);
    let reply = buf.into_reply();
    let out = render_plain(&reply);
    assert!(!out.is_empty());
    assert!(out.starts_with("🐾 Run trace"));
    assert!(out.contains("✅ read · /tmp/a.rs · 5ms"));
}

// ── Multi-line arg summaries ────────────────────────────────────────

#[test]
fn trace_args_preserve_line_breaks() {
    let args = r#"{"command":"cargo build\ncargo test\n cargo clippy"}"#;
    let lines = summarize_args_trace("shell", Some(args));
    assert_eq!(
        lines,
        vec!["cargo build", "cargo test", "cargo clippy"],
        "blank-stripped, per-line flattened"
    );
}

#[test]
fn trace_args_cap_lines_and_mark_dropped() {
    let args = r#"{"command":"l1\nl2\nl3\nl4\nl5"}"#;
    let lines = summarize_args_trace("shell", Some(args));
    assert_eq!(lines, vec!["l1", "l2", "l3", "…"]);
}

#[test]
fn trace_args_cap_each_line() {
    let args = format!(r#"{{"command":"{}"}}"#, "x".repeat(300));
    let lines = summarize_args_trace("shell", Some(&args));
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].chars().count(), TRACE_ARG_LINE_MAX_CHARS + 1); // +1 for …
    assert!(lines[0].ends_with('…'));
}

#[test]
fn trace_args_empty_when_no_known_key_or_args() {
    assert!(summarize_args_trace("todo", Some(r#"{"items":[]}"#)).is_empty());
    assert!(summarize_args_trace("shell", None).is_empty());
}

#[test]
fn render_trace_breaks_long_args_into_continuation_lines() {
    let mut buf = RunReplyBuffer::new();
    let long_cmd = "cargo test ".repeat(20).trim().to_string();
    let args = format!(r#"{{"command":"{long_cmd}"}}"#);
    buf.record_tool_start("t1", "shell", Some(&args));
    buf.record_tool_end("t1", 100, false);
    buf.record_text("done".to_string());
    let reply = buf.into_reply();
    let card = render_card(&reply, None).unwrap();

    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    let body = v["body"]["elements"][1]["elements"][0]["content"]
        .as_str()
        .unwrap();
    let mut iter = body.lines();
    let header = iter.next().unwrap();
    assert!(
        header.starts_with("✅ **shell** · 100ms"),
        "header: {header}"
    );
    assert!(!header.contains('`'), "long args not inline: {header}");
    let cont = iter.next().unwrap();
    assert!(cont.starts_with("↳ `cargo test cargo test"), "cont: {cont}");
    assert!(cont.ends_with("…`"), "truncated continuation: {cont}");
}

#[test]
fn render_trace_multiline_args_each_get_continuation_line() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start(
        "t1",
        "shell",
        Some(r#"{"command":"cargo build &&\n cargo test &&\n cargo clippy"}"#),
    );
    buf.record_tool_end("t1", 5, false);
    buf.record_text("done".to_string());
    let reply = buf.into_reply();
    let card = render_card(&reply, None).unwrap();

    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    let body = v["body"]["elements"][1]["elements"][0]["content"]
        .as_str()
        .unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(lines[0], "✅ **shell** · 5ms");
    assert_eq!(lines[1], "↳ `cargo build &&`");
    assert_eq!(lines[2], "↳ `cargo test &&`");
    assert_eq!(lines[3], "↳ `cargo clippy`");
    assert_eq!(lines.len(), 4);
}

#[test]
fn render_trace_short_args_stay_inline() {
    let reply = buffer_with_run().into_reply();
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("✅ **shell** · `cargo test -p kernel` · 1m05s"));
    assert!(!card.contains('↳'));
}

#[test]
fn render_plain_also_uses_continuation_lines() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "shell", Some(r#"{"command":"a\nb"}"#));
    buf.record_tool_end("t1", 5, false);
    buf.record_text("done".to_string());
    let reply = buf.into_reply();
    let out = render_plain(&reply);
    assert!(out.contains("✅ shell · 5ms\n"));
    assert!(out.contains("↳ a\n"));
    assert!(out.contains("↳ b"));
}
