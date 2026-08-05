use super::*;

fn buffer_with_run() -> RunReplyBuffer {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("Let me look at the code.");
    buf.record_tool_start("t1", "read", Some(r#"{"path":"crates/kernel/src/hub.rs"}"#));
    buf.record_tool_end("t1", 120, false);
    buf.record_tool_start("t2", "shell", Some(r#"{"command":"cargo test -p kernel"}"#));
    buf.record_tool_end("t2", 65_000, false);
    buf.record_model_end("All tests pass.");
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
    assert!(reply.attachments().is_empty());
}

#[test]
fn into_reply_strips_attachments_block_from_body() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("report done\n\n<yomi_attachments>\nout.pdf\n</yomi_attachments>");
    let reply = buf.into_reply();
    assert_eq!(reply.text(), Some("report done"));
    assert_eq!(reply.attachments(), &["out.pdf"]);
}

#[test]
fn into_reply_attachments_only_body_becomes_textless() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("<yomi_attachments>\nout.pdf\n</yomi_attachments>");
    let reply = buf.into_reply();
    assert_eq!(reply.text(), None);
    assert_eq!(reply.attachments(), &["out.pdf"]);
}

#[test]
fn attachments_block_never_renders_anywhere() {
    // Declarations in intermediate texts are stripped at record time too —
    // the XML must not leak into the trace panel, the card, or the body.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("generated the file");
    buf.record_model_end("here you go");
    // Keep the attachment-carrying text last so the live card's current
    // step renders it (the live card shows only the latest entry).
    buf.record_model_end("mid-run note\n<yomi_attachments>\nout.pdf\n</yomi_attachments>");
    // The live card's current step renders from the buffer.
    let current_step = buf.latest_entry_line().unwrap_or_default();
    let reply = buf.into_reply();

    assert_eq!(reply.text(), Some("mid-run note"));
    assert_eq!(reply.attachments(), &["out.pdf"]);
    for rendered in [
        render_card(&reply, None).unwrap(),
        render_plain(&reply),
        current_step,
    ] {
        assert!(
            !rendered.contains("<yomi_attachments>"),
            "xml leaked: {rendered}"
        );
        assert!(!rendered.contains("out.pdf"), "path leaked: {rendered}");
    }
}

#[test]
fn push_note_appends_or_creates_text() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("done");
    let mut reply = buf.into_reply();
    reply.push_note("⚠️ first");
    assert_eq!(reply.text(), Some("done\n\n⚠️ first"));

    let mut textless = RunReplyBuffer::new().into_reply();
    textless.push_note("⚠️ only note");
    assert_eq!(textless.text(), Some("⚠️ only note"));
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
fn tool_call_only_turns_count_as_steps() {
    // Turn 1: a tool-call-only model response (no text) — still a step.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("");
    buf.record_tool_start("t1", "read", None);
    buf.record_tool_end("t1", 10, false);
    // Turn 2: the final text.
    buf.record_model_end("done");
    assert_eq!(buf.step_count(), 2);

    let reply = buf.into_reply();
    let out = render_plain(&reply);
    assert!(out.contains("Trace · 2 steps · 1 tools"), "out: {out}");
}

#[test]
fn full_trace_render_keeps_every_entry_and_empty_is_none() {
    assert!(RunReplyBuffer::new().full_trace_render().is_none());

    let buf = buffer_with_run();
    let Some((lines, title)) = buf.full_trace_render() else {
        panic!("expected a trace");
    };
    // Unlike into_reply, the final text stays a narration — the terminal
    // receipt card shows the whole run, the reply text lands separately.
    assert!(title.starts_with("🐾 Trace · 2 steps · 2 tools"), "{title}");
    let joined = lines.join("\n");
    assert!(joined.contains("💬 Let me look at the code."), "{joined}");
    assert!(joined.contains("✅ **read**"), "{joined}");
    assert!(joined.contains("✅ **shell**"), "{joined}");
    assert!(joined.contains("💬 All tests pass."), "{joined}");
}

#[test]
fn into_reply_after_cancel_mid_tool_uses_last_text() {
    // Cancel mid-tool: the run ends with a pending tool entry; the last
    // text still becomes the body (design: /stop still flushes).
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("Working on it.");
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
        buf.record_model_end("done");
        buf.into_reply()
    };
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("❌"));
    assert!(card.contains("⏳"), "t2 is still pending");
    assert!(card.contains("1 failed"));
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
    assert!(
        title.contains("Trace · 2 steps · 2 tools"),
        "title: {title}"
    );

    let body = panel["elements"][0]["content"].as_str().unwrap();
    assert!(body.contains("💬 Let me look at the code."));
    assert!(body.contains("✅ **read** · `crates/kernel/src/hub.rs` · 120ms"));
    assert!(body.contains("✅ **shell** · `cargo test -p kernel` · 1m05s"));
}

#[test]
fn render_card_without_trace_is_a_single_markdown_element() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("Just an answer.");
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
    buf.record_model_end(&"x".repeat(FINAL_TEXT_MAX_BYTES + 100));
    let reply = buf.into_reply();
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("...(内容已截断)"));
}

#[test]
fn render_trace_shows_all_entries() {
    let mut buf = RunReplyBuffer::new();
    for i in 0..30 {
        buf.record_tool_start(&format!("t{i}"), "read", None);
        buf.record_tool_end(&format!("t{i}"), 1, false);
    }
    buf.record_model_end("done");
    let reply = buf.into_reply();

    let card = render_card(&reply, None).unwrap();
    let panel_body = serde_json::from_str::<serde_json::Value>(&card).unwrap();
    let body = panel_body["body"]["elements"][1]["elements"][0]["content"]
        .as_str()
        .unwrap()
        .to_string();
    // Complete trace: every entry rendered, no dropped marker.
    assert_eq!(body.lines().count(), 30);
    assert!(!body.contains("earlier entries"));
}

#[test]
fn buffer_drops_oldest_entries_beyond_cap() {
    let mut buf = RunReplyBuffer::new();
    for i in 0..(BUFFER_MAX_ENTRIES + 20) {
        buf.record_model_end(&format!("text {i}"));
    }
    let reply = buf.into_reply();
    // The latest text survives as the body even though old ones were dropped.
    assert_eq!(
        reply.text.as_deref(),
        Some(format!("text {}", BUFFER_MAX_ENTRIES + 19)).as_deref()
    );
    assert!(reply.entries.len() <= BUFFER_MAX_ENTRIES);
    // … and the final render surfaces the dropped count as a marker line.
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("··· and 20 earlier entries"));
}

#[test]
fn render_plain_appends_trace_without_markup() {
    let reply = buffer_with_run().into_reply();
    let out = render_plain(&reply);
    assert!(out.starts_with("All tests pass."));
    assert!(out.contains("🐾 Trace · 2 steps · 2 tools"));
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
    buf.record_model_end("plain answer");
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
    assert!(out.starts_with("🐾 Trace"));
    assert!(out.contains("✅ read · /tmp/a.rs · 5ms"));
}

// ── Arg summaries (single-line, flattened + truncated) ──────────────

#[test]
fn trace_arg_summary_flattens_multiline_args() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start(
        "t1",
        "shell",
        Some(r#"{"command":"cargo build &&\n cargo test &&\n cargo clippy"}"#),
    );
    buf.record_tool_end("t1", 5, false);
    buf.record_model_end("done");
    let reply = buf.into_reply();
    let card = render_card(&reply, None).unwrap();
    // Multi-line args flatten to one inline line; no continuation lines.
    assert!(card.contains("✅ **shell** · `cargo build && cargo test && cargo clippy` · 5ms"));
    assert!(!card.contains('↳'));
}

#[test]
fn trace_arg_summary_caps_long_values() {
    let args = format!(r#"{{"command":"{}"}}"#, "x".repeat(300));
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "shell", Some(&args));
    buf.record_model_end("done");
    let reply = buf.into_reply();
    let card = render_card(&reply, None).unwrap();
    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    let body = v["body"]["elements"][1]["elements"][0]["content"]
        .as_str()
        .unwrap();
    let line = body.lines().next().unwrap();
    // "⏳ **shell** · `" + ARG_SUMMARY_MAX_CHARS chars + "…`"
    assert!(line.ends_with("…`"), "line: {line}");
    assert!(
        line.chars().count() <= 20 + ARG_SUMMARY_MAX_CHARS + 2,
        "line: {line}"
    );
}

#[test]
fn trace_arg_summary_empty_when_no_known_key_or_args() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "todo", Some(r#"{"items":[]}"#));
    buf.record_tool_start("t2", "shell", None);
    buf.record_model_end("done");
    let reply = buf.into_reply();
    let card = render_card(&reply, None).unwrap();
    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    let body = v["body"]["elements"][1]["elements"][0]["content"]
        .as_str()
        .unwrap();
    // No dangling arg bullets for summary-less tools.
    assert!(body.lines().all(|l| !l.contains(" · `")), "body: {body}");
}

#[test]
fn render_trace_long_args_stay_one_truncated_line() {
    let mut buf = RunReplyBuffer::new();
    let long_cmd = "cargo test ".repeat(20).trim().to_string();
    let args = format!(r#"{{"command":"{long_cmd}"}}"#);
    buf.record_tool_start("t1", "shell", Some(&args));
    buf.record_tool_end("t1", 100, false);
    buf.record_model_end("done");
    let reply = buf.into_reply();
    let card = render_card(&reply, None).unwrap();

    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    let body = v["body"]["elements"][1]["elements"][0]["content"]
        .as_str()
        .unwrap();
    let mut iter = body.lines();
    let header = iter.next().unwrap();
    assert!(
        header.starts_with("✅ **shell** · `cargo test cargo test"),
        "header: {header}"
    );
    assert!(header.contains("…` · 100ms"), "truncated inline: {header}");
    assert!(iter.next().is_none(), "single line only: {body}");
}

#[test]
fn render_trace_multiline_args_flatten_to_one_line() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start(
        "t1",
        "shell",
        Some(r#"{"command":"cargo build &&\n cargo test &&\n cargo clippy"}"#),
    );
    buf.record_tool_end("t1", 5, false);
    buf.record_model_end("done");
    let reply = buf.into_reply();
    let card = render_card(&reply, None).unwrap();

    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    let body = v["body"]["elements"][1]["elements"][0]["content"]
        .as_str()
        .unwrap();
    let lines: Vec<&str> = body.lines().collect();
    assert_eq!(
        lines.as_slice(),
        ["✅ **shell** · `cargo build && cargo test && cargo clippy` · 5ms"]
    );
}

#[test]
fn render_trace_short_args_stay_inline() {
    let reply = buffer_with_run().into_reply();
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("✅ **shell** · `cargo test -p kernel` · 1m05s"));
    assert!(!card.contains('↳'));
}

#[test]
fn render_plain_flattens_multiline_args() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "shell", Some(r#"{"command":"a\nb"}"#));
    buf.record_tool_end("t1", 5, false);
    buf.record_model_end("done");
    let reply = buf.into_reply();
    let out = render_plain(&reply);
    assert!(out.contains("✅ shell · a b · 5ms"));
    assert!(!out.contains('↳'));
}
