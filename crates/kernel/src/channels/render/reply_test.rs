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
fn into_reply_promotes_longest_earlier_text_above_last() {
    let reply = buffer_with_run().into_reply();
    assert_eq!(
        reply.body_texts(),
        &[
            "Let me look at the code.".to_string(),
            "All tests pass.".to_string()
        ]
    );
    assert_eq!(reply.text(), Some("All tests pass."));
    // Both texts left the trace; the tools stay chronological in the panel.
    assert_eq!(reply.entries.len(), 2);
    assert!(reply
        .entries
        .iter()
        .all(|e| matches!(e, TraceEntry::Tool(_))));
    assert!(reply.attachments().is_empty());
}

#[test]
fn into_reply_last_text_longest_stays_single_body() {
    // A long final answer needs no companion: the short preamble stays in
    // the process panel instead of riding along as a second body text.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("先看一下");
    buf.record_model_end("这是本轮真正的最终答案，内容比开头那句铺垫长得多。");
    let reply = buf.into_reply();
    assert_eq!(
        reply.body_texts(),
        &["这是本轮真正的最终答案，内容比开头那句铺垫长得多。".to_string()]
    );
    assert_eq!(reply.entries.len(), 1);
    assert!(
        matches!(&reply.entries[0], TraceEntry::Narration(t) if t == "先看一下"),
        "preamble stays in the trace"
    );
}

#[test]
fn into_reply_promotes_longest_text_beyond_second_to_last() {
    // The motivating case: the substantive answer sits three texts back —
    // the last two selection would have buried it in the process panel.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("这是重要结果：检索到的三条关键证据与完整分析结论。");
    buf.record_model_end("中间补了一句进展");
    buf.record_model_end("搞定");
    let reply = buf.into_reply();
    assert_eq!(
        reply.body_texts(),
        &[
            "这是重要结果：检索到的三条关键证据与完整分析结论。".to_string(),
            "搞定".to_string()
        ]
    );
    // The middle text was never promoted: it stays in the trace.
    assert_eq!(reply.entries.len(), 1);
    assert!(
        matches!(&reply.entries[0], TraceEntry::Narration(t) if t == "中间补了一句进展"),
        "middle text stays in the trace"
    );
}

#[test]
fn into_reply_tie_for_longest_keeps_last_single() {
    // Equal length: the last text already counts as the longest — no
    // second body text.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("aaaaaaaaaa");
    buf.record_model_end("bbbbbbbbbb");
    let reply = buf.into_reply();
    assert_eq!(reply.body_texts(), &["bbbbbbbbbb".to_string()]);
}

#[test]
fn into_reply_tied_earlier_texts_promote_the_most_recent() {
    // Two earlier texts tie for longest: the more recent one is promoted.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("aaaaaaaaaaaaaaaaaaaa");
    buf.record_model_end("bbbbbbbbbbbbbbbbbbbb");
    buf.record_model_end("ccccc");
    let reply = buf.into_reply();
    assert_eq!(
        reply.body_texts(),
        &["bbbbbbbbbbbbbbbbbbbb".to_string(), "ccccc".to_string()]
    );
    assert!(
        matches!(&reply.entries[0], TraceEntry::Narration(t) if t == "aaaaaaaaaaaaaaaaaaaa"),
        "the older tied text stays in the trace"
    );
}

#[test]
fn trace_markdown_sanitizes_structural_chars_in_dynamic_text() {
    // 未闭合的反引号/星号/尖括号会撑破飞书卡片的 markdown（整个元素按
    // 纯文本回退、标签漏成字面量）：动态文本在 markdown 渲染时全角化，
    // 纯文本路径保留原文。
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("核心是 `CronSchedule::parse` 和 `Option<Level>`，还有 `0/7 未闭合");
    buf.record_tool_start(
        "t1",
        "shell",
        Some(r#"{"command":"echo `whoami` **x** <b>"}"#),
    );
    buf.record_tool_end("t1", 10, false);

    let joined = buf.trace_preview_lines(10).join("\n");
    // 渲染方加的结构标记保持原样
    assert!(joined.contains("<font color='grey'>💬 "), "{joined}");
    assert!(joined.contains("**shell**"), "{joined}");
    // 内容里的结构字符已全角化
    assert!(joined.contains("｀CronSchedule::parse｀"), "{joined}");
    assert!(joined.contains("＜Level＞"), "{joined}");
    assert!(joined.contains("｀whoami｀"), "{joined}");
    assert!(joined.contains("＊＊x＊＊"), "{joined}");
    assert!(joined.contains("＜b＞"), "{joined}");
    // 全文只剩工具摘要那对外层行内码反引号
    assert_eq!(
        joined.matches('`').count(),
        2,
        "only the tool-summary wrapper backticks: {joined}"
    );

    // 纯文本路径保留原文
    let plain = trace_lines(&buf.entries, false).join("\n");
    assert!(plain.contains("`CronSchedule::parse`"), "{plain}");
    assert!(plain.contains("**x**"), "{plain}");
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
    buf.record_model_end("mid-run note\n<yomi_attachments>\nout.pdf\n</yomi_attachments>");
    buf.record_model_end("here you go");
    // The live card preview renders from the buffer.
    let preview = buf.trace_preview_lines(10).join("\n");
    let reply = buf.into_reply();

    assert_eq!(reply.text(), Some("here you go"));
    assert_eq!(reply.attachments(), &["out.pdf"]);
    for rendered in [
        render_card(&reply, None).unwrap(),
        render_plain(&reply),
        preview,
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
    assert!(out.contains("🐾 0s · 💬 2"), "out: {out}");
}

/// 标题段统一：model / ctx / failed / 流量设置后出现在 trace 标题
/// 上——live 卡、终态收据、回复卡三面共用同一套段落规则（仅 ~
/// 标记的输出估算是 live-only，按"缺失即省略"规则不出现在终态）。
#[test]
fn trace_title_carries_model_ctx_usage_and_failed_when_set() {
    let mut buf = buffer_with_run();
    buf.record_tool_start("t3", "shell", None);
    buf.record_tool_end("t3", 5, true);
    buf.set_model("k3-hs".to_string());
    buf.set_ctx_footer(12_345, 128_000);
    buf.add_usage(&crate::types::MessageId::new(), 12_345, 2_345);
    buf.add_usage(&crate::types::MessageId::new(), 100, 5);

    let Some(panel) = buf.terminal_trace_panel() else {
        panic!("expected a trace");
    };
    let title = panel["header"]["title"]["content"].as_str().unwrap();
    assert_eq!(
        title, "🐾 0s · 💬 2 · 12.4k↑ · 2.4k↓ · 🙀 1 · k3-hs · 10%",
        "title: {title}"
    );

    // into_reply（回复卡路径）带出同样的段。
    let card = render_card(&buf.into_reply(), None).unwrap();
    assert!(
        card.contains("🐾 0s · 💬 2 · 12.4k↑ · 2.4k↓ · 🙀 1 · k3-hs · 10%"),
        "card: {card}"
    );
}

/// tools/failed 是增量计数器：老条目被 buffer cap 挤掉后，标题总数
/// 仍然真实（按条目数统计会悄悄缩水）。
#[test]
fn title_counters_survive_buffer_cap() {
    let mut buf = RunReplyBuffer::new();
    // 100 条 cap：先 90 条 narration，再 20 个 tool（10 失败）——
    // 最终条目里只剩 10 个 tool，但标题要显示全部 20 / 10。
    for i in 0..90 {
        buf.record_model_end(&format!("text {i}"));
    }
    for i in 0..20 {
        buf.record_tool_start(&format!("t{i}"), "shell", None);
        buf.record_tool_end(&format!("t{i}"), 5, i % 2 == 0);
    }
    assert!(buf.entries.len() <= 100);

    let Some(panel) = buf.terminal_trace_panel() else {
        panic!("expected a trace");
    };
    let title = panel["header"]["title"]["content"].as_str().unwrap();
    assert!(
        title.contains("💬 90"),
        "all steps counted despite cap: {title}"
    );
    assert!(
        title.contains("🙀 10"),
        "all failures counted despite cap: {title}"
    );
}

#[test]
fn terminal_trace_panel_keeps_every_entry_and_empty_is_none() {
    assert!(RunReplyBuffer::new().terminal_trace_panel().is_none());

    let buf = buffer_with_run();
    let Some(panel) = buf.terminal_trace_panel() else {
        panic!("expected a trace");
    };
    // Unlike into_reply, the final text stays a narration — the terminal
    // receipt card shows the whole run, the reply text lands separately.
    let title = panel["header"]["title"]["content"].as_str().unwrap();
    assert!(title.starts_with("🐾 0s · 💬 2"), "{title}");
    // Collapsed tombstone with the process layout: both texts full-size,
    // the tool run folded into a nested panel.
    assert_eq!(panel["expanded"], false);
    let body = panel["elements"].as_array().unwrap();
    let tags: Vec<&str> = body.iter().map(|e| e["tag"].as_str().unwrap()).collect();
    assert_eq!(tags, ["markdown", "collapsible_panel", "markdown"]);
    assert_eq!(body[0]["content"], "Let me look at the code.");
    assert_eq!(body[2]["content"], "All tests pass.");
    let tools = body[1]["elements"][0]["content"].as_str().unwrap();
    assert!(tools.contains("✅ **read**"), "{tools}");
    assert!(tools.contains("✅ **shell**"), "{tools}");
}

#[test]
fn into_reply_after_cancel_mid_tool_uses_last_text() {
    // Cancel mid-tool: the run ends with a pending tool entry; the last
    // text still becomes the body (design: /stop still flushes).
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("Working on it.");
    buf.record_tool_start("t1", "shell", Some(r#"{"command":"sleep 100"}"#));
    let reply = buf.into_reply();
    assert_eq!(reply.text(), Some("Working on it."));
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
    assert!(card.contains("🙀"));
    assert!(card.contains("⏳"), "t2 is still pending");
    assert!(card.contains("🙀 1"));
}

#[test]
fn render_card_structure() {
    let reply = buffer_with_run().into_reply();
    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();

    assert_eq!(card["schema"], "2.0");
    let elements = card["body"]["elements"].as_array().unwrap();
    // previous text → divider → latest text → tool-trace panel.
    assert_eq!(elements.len(), 4);
    assert_eq!(elements[0]["tag"], "markdown");
    assert_eq!(elements[0]["content"], "Let me look at the code.");
    assert_eq!(elements[1]["tag"], "hr");
    assert_eq!(elements[2]["tag"], "markdown");
    assert_eq!(elements[2]["content"], "All tests pass.");

    let panel = &elements[3];
    assert_eq!(panel["tag"], "collapsible_panel");
    // Every panel on the final card starts collapsed — the process
    // narrative is one click away.
    assert_eq!(panel["expanded"], false);
    let title = panel["header"]["title"]["content"].as_str().unwrap();
    assert!(title.contains("🐾 0s · 💬 2"), "title: {title}");

    // No intermediate text left in the trace: the classic folded tool panel.
    let body = panel["elements"].as_array().unwrap();
    assert_eq!(body.len(), 1);
    let tools = body[0]["content"].as_str().unwrap();
    assert!(tools.contains("✅ **read** · `crates/kernel/src/hub.rs` · 120ms"));
    assert!(tools.contains("✅ **shell** · `cargo test -p kernel` · 1m05s"));
}

#[test]
fn process_panel_keeps_chronology_and_folds_each_tool_run() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t0", "read", None);
    buf.record_tool_end("t0", 5, false);
    buf.record_model_end("first note");
    buf.record_tool_start("t1", "shell", None);
    buf.record_tool_end("t1", 100, false);
    buf.record_tool_start("t2", "shell", None);
    buf.record_tool_end("t2", 200, false);
    buf.record_model_end("second note");
    buf.record_tool_start("t3", "edit", None);
    buf.record_tool_end("t3", 8, false);
    buf.record_model_end("done");
    let card: serde_json::Value =
        serde_json::from_str(&render_card(&buf.into_reply(), None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 4);
    assert_eq!(elements[0]["content"], "second note");
    assert_eq!(elements[1]["tag"], "hr");
    assert_eq!(elements[2]["content"], "done");

    let body = elements[3]["elements"].as_array().unwrap();
    let tags: Vec<&str> = body.iter().map(|e| e["tag"].as_str().unwrap()).collect();
    assert_eq!(
        tags,
        [
            "collapsible_panel", // leading tool run (t0)
            "markdown",          // first note stays in the process panel
            "collapsible_panel", // t1+t2+t3 became consecutive after the body split
        ]
    );
    assert_eq!(body[1]["content"], "first note");
    let t0 = body[0]["header"]["title"]["content"].as_str().unwrap();
    assert!(t0.starts_with("🔧 read"), "title: {t0}");
    let t12 = body[2]["header"]["title"]["content"].as_str().unwrap();
    assert!(t12.starts_with("🔧 shell×2 · edit"), "title: {t12}");
    let t12body = body[2]["elements"][0]["content"].as_str().unwrap();
    assert_eq!(t12body.lines().count(), 3, "three tools in one panel");
}

#[test]
fn process_panel_rewrites_mentions_in_intermediate_texts() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("cc <@ou_mid>");
    // The final text is the longest: single body, the mention text stays
    // in the process panel — this exercises the panel's rewrite path.
    buf.record_model_end("done — this final text is deliberately longer than the mention line");
    let reply = buf.into_reply();
    assert_eq!(reply.body_texts().len(), 1, "single body");
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("<at id=ou_mid></at>"), "{card}");
}

#[test]
fn tools_only_run_keeps_single_collapsed_trace_panel() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "read", None);
    buf.record_tool_end("t1", 5, false);
    buf.record_model_end("done");
    let card: serde_json::Value =
        serde_json::from_str(&render_card(&buf.into_reply(), None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 2);
    // No intermediate texts → no process panel, no nesting: the classic
    // collapsed trace panel.
    let panel = &elements[1];
    assert_eq!(panel["tag"], "collapsible_panel");
    assert_eq!(panel["expanded"], false);
    let title = panel["header"]["title"]["content"].as_str().unwrap();
    assert!(title.starts_with("🐾"), "title: {title}");
    let body = panel["elements"][0]["content"].as_str().unwrap();
    assert!(body.contains("✅ **read**"), "body: {body}");
}

#[test]
fn trace_panel_element_expanded_starts_open() {
    // The live mid-run card keeps the trace visible to the human (expanded),
    // while reading bots strip the panel regardless of `expanded`.
    let panel = super::trace_panel_element_expanded(&["l1".to_string()], "t");
    assert_eq!(panel["tag"], "collapsible_panel");
    assert_eq!(panel["expanded"], true);
    // The collapsed variant used on terminal/reply cards stays collapsed.
    let panel = super::trace_panel(&["l1".to_string()], "t", false);
    assert_eq!(panel["expanded"], false);
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
    assert!(card.contains("...(truncated)"));
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
        reply.text(),
        Some(format!("text {}", BUFFER_MAX_ENTRIES + 19)).as_deref()
    );
    assert!(reply.entries.len() <= BUFFER_MAX_ENTRIES);
    // … and the process panel leads with the dropped-count marker line.
    let card = render_card(&reply, None).unwrap();
    assert!(card.contains("··· and 20 earlier entries"));
    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    let body = v["body"]["elements"][1]["elements"].as_array().unwrap();
    assert_eq!(body[0]["tag"], "markdown");
    assert_eq!(body[0]["text_size"], "notation");
    assert_eq!(body[0]["content"], "··· and 20 earlier entries");
    // The plain fallback carries the same marker after the title line.
    let out = render_plain(&reply);
    assert!(
        out.contains("\n··· and 20 earlier entries\n"),
        "plain marker: {out}"
    );
}

#[test]
fn render_plain_appends_trace_without_markup() {
    let reply = buffer_with_run().into_reply();
    let out = render_plain(&reply);
    // Two body texts first (divider line between steps), then the title and
    // the chronological transcript: tools as plain lines.
    assert!(out.starts_with("Let me look at the code.\n\n---\n\nAll tests pass."));
    assert!(out.contains("🐾 0s · 💬 2"));
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
    assert_eq!(
        reply.into_text().as_deref(),
        Some("Let me look at the code.\n\n---\n\nAll tests pass.")
    );
}

#[test]
fn render_card_with_notice_prepends_notice_line() {
    let reply = buffer_with_run().into_reply();
    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, Some("🙀 **Error**  boom")).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 5);
    assert_eq!(elements[0]["content"], "🙀 **Error**  boom");
    assert_eq!(elements[1]["content"], "Let me look at the code.");
    assert_eq!(elements[2]["tag"], "hr");
    assert_eq!(elements[3]["content"], "All tests pass.");
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
    let card = render_card(&reply, Some("🙀 **Error**  boom")).unwrap();
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
    assert!(out.starts_with("🐾 "));
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
fn trace_arg_summary_unknown_tool_falls_back_to_raw_json() {
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
    // 键表全落空的工具：原始 JSON 上卡（md_safe 不影响 JSON 结构字符）
    let todo_line = body
        .lines()
        .find(|l| l.contains("**todo**"))
        .expect("todo line");
    assert!(todo_line.contains(r#"{"items":[]}"#), "line: {todo_line}");
    // 无参数调用仍然空白（没有东西可显示）
    let shell_line = body
        .lines()
        .find(|l| l.contains("**shell**"))
        .expect("shell line");
    assert!(!shell_line.contains(" · `"), "line: {shell_line}");
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

#[test]
fn render_card_rewrites_mentions() {
    // `<@USER_ID>` contract → feishu <at>；行内 code 里的不动。
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("cc <@ou_abc>，示例：`<@ou_x>`");
    let card = render_card(&buf.into_reply(), None).unwrap();
    assert!(card.contains("<at id=ou_abc></at>"), "{card}");
    assert!(card.contains("`<@ou_x>`"), "{card}");
}

#[test]
fn add_usage_folds_only_the_delta_for_repeated_events_of_one_response() {
    let mut buf = RunReplyBuffer::new();
    let mid = crate::types::MessageId::new();
    // Partial then final usage for the same response (same message id):
    // the run total tracks the final values, not the sum.
    buf.add_usage(&mid, 10_000, 1_000);
    buf.add_usage(&mid, 10_000, 2_345);
    assert_eq!(buf.usage(), (10_000, 2_345));
    // Identical repeat (some providers push the same usage twice): no-op.
    buf.add_usage(&mid, 10_000, 2_345);
    assert_eq!(buf.usage(), (10_000, 2_345));
    // A new response (new message id) accumulates on top.
    buf.add_usage(&crate::types::MessageId::new(), 500, 50);
    assert_eq!(buf.usage(), (10_500, 2_395));
}

#[test]
fn cron_tool_summary_combines_action_target_schedule() {
    // cron 的参数是动词结构：摘要 = action · 目标(name>id) · schedule
    let summary_of = |args: &str| {
        let mut buf = RunReplyBuffer::new();
        buf.record_tool_start("t1", "cron", Some(args));
        buf.trace_preview_lines(1).join("")
    };

    let line = summary_of(
        r#"{"action":"create","name":"daily","schedule":"0 9 * * *","type":"send_message","content":"hi"}"#,
    );
    // 渲染层的 md_safe 会把 * 全角化，断言分开写
    assert!(line.contains("create · daily · 0 9"), "got: {line}");

    let line = summary_of(r#"{"action":"delete","id":"cron_01ABC"}"#);
    assert!(line.contains("delete · cron_01ABC"), "got: {line}");

    // 无目标时 action 本身也要显示（此前整行空白）
    let line = summary_of(r#"{"action":"list"}"#);
    assert!(line.contains("list"), "got: {line}");
}

#[test]
fn tool_run_title_merges_consecutive_same_name_tools() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "shell", None);
    buf.record_tool_end("t1", 100, false);
    buf.record_tool_start("t2", "shell", None);
    buf.record_tool_end("t2", 200, false);
    buf.record_tool_start("t3", "read", None);
    buf.record_tool_end("t3", 5, false);
    buf.record_tool_start("t4", "shell", None);
    buf.record_tool_end("t4", 50, false);
    buf.record_model_end("done");
    let reply = buf.into_reply();
    let title = tool_run_title(&reply.entries);
    // Consecutive duplicates merge with ×N; the later shell stays
    // separate (chronology beats global counting).
    assert_eq!(title, "🔧 shell×2 · read · shell · 355ms");
}

#[test]
fn tool_run_title_caps_long_name_lists() {
    let mut buf = RunReplyBuffer::new();
    for i in 0..20 {
        buf.record_tool_start(&format!("t{i}"), &format!("toolname{i}"), None);
        buf.record_tool_end(&format!("t{i}"), 1, false);
    }
    buf.record_model_end("done");
    let reply = buf.into_reply();
    let title = tool_run_title(&reply.entries);
    assert!(title.starts_with("🔧 toolname0 · toolname1"), "{title}");
    assert!(title.contains('…'), "names capped: {title}");
    assert!(title.ends_with("· 20ms"), "elapsed kept: {title}");
}

#[test]
fn tool_run_title_omits_zero_elapsed() {
    let mut buf = RunReplyBuffer::new();
    buf.record_tool_start("t1", "read", None);
    let reply = {
        buf.record_model_end("done");
        buf.into_reply()
    };
    // Pending tool (no elapsed yet): names only, no duration segment.
    assert_eq!(tool_run_title(&reply.entries), "🔧 read");
}

#[test]
fn balance_fences_closes_unclosed_fence() {
    assert_eq!(
        balance_fences("text\n```rust\nfn main() {}").as_ref(),
        "text\n```rust\nfn main() {}\n```"
    );
    assert_eq!(balance_fences("~~~\ncode").as_ref(), "~~~\ncode\n~~~");
}

#[test]
fn balance_fences_leaves_balanced_text_borrowed() {
    let balanced = "a ``` b\n```rust\nx\n```\ndone";
    let std::borrow::Cow::Borrowed(_) = balance_fences(balanced) else {
        panic!("balanced text must not be reallocated");
    };
    // A complete open/close pair stays untouched.
    assert_eq!(balance_fences("```\nx\n```").as_ref(), "```\nx\n```");
}

#[test]
fn balance_fences_treats_the_other_marker_as_content() {
    // Tilde fences quoted inside a backtick fence (the standard way to
    // show markdown source) must not flip the state — appending a stray
    // fence here would manufacture the very degradation this prevents.
    let quoted = "```markdown\n~~~\nx\n~~~\n```";
    let std::borrow::Cow::Borrowed(_) = balance_fences(quoted) else {
        panic!("mixed but balanced text must be left untouched");
    };
    // … and a genuinely unclosed outer fence still closes with its own
    // marker, not the inner content's.
    assert_eq!(
        balance_fences("```markdown\n~~~\nx\n~~~").as_ref(),
        "```markdown\n~~~\nx\n~~~\n```"
    );
}

#[test]
fn balance_fences_ignores_info_string_closers_and_indented_lines() {
    // An inner fence line WITH an info string is content, not a closer
    // (CommonMark): the whole block is one balanced fence.
    let quoted = "```markdown\n```rust\nfn main() {}\n```";
    let std::borrow::Cow::Borrowed(_) = balance_fences(quoted) else {
        panic!("info-string inner fence must be content");
    };
    // 4+ space indent = indented code block, not a fence at all.
    let indented = "example:\n\n    ```\n    not a fence";
    let std::borrow::Cow::Borrowed(_) = balance_fences(indented) else {
        panic!("indented fence-like lines must be ignored");
    };
}

#[test]
fn balance_fences_closes_with_the_openers_run_length() {
    // A truncated 4-backtick fence needs ≥4 backticks to close.
    assert_eq!(balance_fences("````\nx").as_ref(), "````\nx\n````");
    // A shorter bare run inside a longer fence is content; the fence
    // is still open and closes at its own length.
    assert_eq!(
        balance_fences("````\nx\n```").as_ref(),
        "````\nx\n```\n````"
    );
}

#[test]
fn balance_fences_rejects_backtick_in_opener_info_string() {
    // CommonMark: a backtick fence's info string may not contain a
    // backtick — the line is a paragraph, no fence opens.
    let sloppy = "```bash echo `date`\n\ntext";
    let std::borrow::Cow::Borrowed(_) = balance_fences(sloppy) else {
        panic!("backtick-in-info line must not open a fence");
    };
    // Tilde fences may carry backticks in the info string.
    assert_eq!(
        balance_fences("~~~bash echo `date`\nx").as_ref(),
        "~~~bash echo `date`\nx\n~~~"
    );
    // Closers accept trailing spaces/tabs, nothing else.
    let tabbed = "```\nx\n```\t";
    let std::borrow::Cow::Borrowed(_) = balance_fences(tabbed) else {
        panic!("tab after closer is allowed");
    };
    assert_eq!(
        balance_fences("```\nx\n```\u{a0}").as_ref(),
        "```\nx\n```\u{a0}\n```",
        "NBSP after the run is not a closer — still open"
    );
}

#[test]
fn render_card_balances_fence_cut_by_byte_cap() {
    // No cancel needed: the byte cap can slice between a fence pair —
    // pad past the cap inside an open fence so the cut leaves it open.
    let mut text = "```\n".to_string();
    text.push_str(&"x".repeat(FINAL_TEXT_MAX_BYTES));
    text.push_str("\n```");
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(&text);
    let card = render_card(&buf.into_reply(), None).unwrap();
    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    let content = v["body"]["elements"][0]["content"].as_str().unwrap();
    assert!(content.contains("...(truncated)"), "capped: {content}");
    assert!(content.ends_with("```"), "cut fence closed: {content}");
}

#[test]
fn narration_only_process_panel_has_no_tool_panels() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("first");
    buf.record_model_end("second");
    buf.record_model_end("final");
    let card: serde_json::Value =
        serde_json::from_str(&render_card(&buf.into_reply(), None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 4);
    assert_eq!(elements[0]["content"], "second");
    assert_eq!(elements[1]["tag"], "hr");
    assert_eq!(elements[2]["content"], "final");

    let body = elements[3]["elements"].as_array().unwrap();
    assert_eq!(body.len(), 1);
    assert!(
        body.iter().all(|e| e["tag"] == "markdown"),
        "texts only, no stray empty tool panel: {body:?}"
    );
    assert_eq!(body[0]["content"], "first");
}

#[test]
fn render_card_balances_fence_cut_by_cancellation() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("看这段代码：\n```rust\nfn broken(");
    buf.record_model_end("second note — this middle text is deliberately the longest one here");
    buf.record_model_end("done");
    let card = render_card(&buf.into_reply(), None).unwrap();
    // The longest text joins the last as the body; the broken-fence one
    // stays in the trace panel, which must still close the fence.
    // (serialized JSON escapes newlines as \\n)
    assert!(
        card.contains(r"fn broken(\n```"),
        "fence closed in the process panel: {card}"
    );
}

#[test]
fn render_card_splits_budget_across_two_body_texts() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(&"a".repeat(FINAL_TEXT_MAX_BYTES));
    // Shorter than the first but still over the halved budget: the longest
    // earlier text joins the last as a second body segment.
    buf.record_model_end(&"b".repeat(FINAL_TEXT_MAX_BYTES / 2 + 100));
    let card = render_card(&buf.into_reply(), None).unwrap();
    let card: serde_json::Value = serde_json::from_str(&card).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    // [first text, hr, second text] — each capped at half the budget.
    assert_eq!(elements[1]["tag"], "hr");
    let first = elements[0]["content"].as_str().unwrap();
    let second = elements[2]["content"].as_str().unwrap();
    let per_text = FINAL_TEXT_MAX_BYTES / 2;
    assert!(
        first.len() <= per_text,
        "first body capped: {}",
        first.len()
    );
    assert!(
        second.len() <= per_text,
        "second body capped: {}",
        second.len()
    );
    assert!(first.contains("...(truncated)"));
    assert!(second.contains("...(truncated)"));
}

#[test]
fn render_card_short_body_donates_budget_to_the_long_one() {
    // [very long promoted text, short tail]: the tail's unused share
    // flows to the long text — a promoted text keeps as much of its tail
    // as possible (it left the uncapped process panel for the body).
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(&"a".repeat(25_000));
    buf.record_model_end(&"b".repeat(100));
    let card = render_card(&buf.into_reply(), None).unwrap();
    let card: serde_json::Value = serde_json::from_str(&card).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    assert_eq!(elements[1]["tag"], "hr");
    let first = elements[0]["content"].as_str().unwrap();
    let second = elements[2]["content"].as_str().unwrap();
    // 25000 ≤ 28000−100: the long text survives whole.
    assert_eq!(first.len(), 25_000, "no truncation: {}", first.len());
    assert_eq!(second.len(), 100);

    // Beyond the donated budget it still truncates — but at 27900, not 14000.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(&"a".repeat(FINAL_TEXT_MAX_BYTES + 1_000));
    buf.record_model_end(&"b".repeat(100));
    let card = render_card(&buf.into_reply(), None).unwrap();
    let card: serde_json::Value = serde_json::from_str(&card).unwrap();
    let first = card["body"]["elements"][0]["content"].as_str().unwrap();
    assert!(first.contains("...(truncated)"));
    assert!(
        first.len() > FINAL_TEXT_MAX_BYTES / 2,
        "donated budget used: {}",
        first.len()
    );
    assert!(first.len() <= FINAL_TEXT_MAX_BYTES - 100);
}

#[test]
fn into_reply_strips_end_turn_marker_from_body() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("report done __YOMI_END_TURN__");
    let reply = buf.into_reply();
    assert_eq!(reply.text(), Some("report done"));
}

#[test]
fn into_reply_marker_only_body_becomes_textless() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("__YOMI_END_TURN__");
    let reply = buf.into_reply();
    assert_eq!(reply.text(), None);
}

#[test]
fn into_reply_keeps_mid_text_marker() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("__YOMI_END_TURN__ 出现在中间是惰性文本");
    let reply = buf.into_reply();
    assert_eq!(reply.text(), Some("__YOMI_END_TURN__ 出现在中间是惰性文本"));
}

#[test]
fn render_card_renders_inline_images_below_body_before_trace() {
    let mut reply = buffer_with_run().into_reply();
    reply.set_inline_images(vec![
        InlineImage {
            key: "img_k1".to_string(),
            alt: "a.png".to_string(),
            path: std::path::PathBuf::from("/tmp/a.png"),
            declared: "a.png".to_string(),
        },
        InlineImage {
            key: "img_k2".to_string(),
            alt: "b.png".to_string(),
            path: std::path::PathBuf::from("/tmp/b.png"),
            declared: "b.png".to_string(),
        },
    ]);

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();

    // buffer_with_run promotes the longest earlier text above the last:
    // two body markdown elements split by an hr, then the images in
    // declaration order, then the collapsed trace panel last.
    let tags: Vec<&str> = elements
        .iter()
        .map(|e| e["tag"].as_str().unwrap())
        .collect();
    assert_eq!(
        tags,
        vec![
            "markdown",
            "hr",
            "markdown",
            "img",
            "img",
            "collapsible_panel"
        ]
    );
    assert_eq!(
        elements[3],
        serde_json::json!({
            "tag": "img",
            "img_key": "img_k1",
            "alt": { "tag": "plain_text", "content": "a.png" }
        })
    );
    assert_eq!(elements[4]["img_key"], "img_k2");
    assert_eq!(elements[4]["alt"]["content"], "b.png");
}

// ── anchored inline images (block position = image position) ────────

#[test]
fn record_model_end_leaves_anchor_token_for_images_only() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(
        "看这里\n<yomi_attachments>\noutput/a.png\noutput/b.pdf\n</yomi_attachments>\n下一段",
    );
    let reply = buf.into_reply();
    let body = reply.text().unwrap();
    // Image path anchored in place; the file path leaves no token.
    assert!(body.contains(&crate::utils::attachments::attachment_token("output/a.png")));
    assert!(!body.contains("b.pdf"));
    // Both paths collected for delivery.
    assert_eq!(reply.attachments(), &["output/a.png", "output/b.pdf"]);
}

#[test]
fn render_card_places_image_at_its_anchor_token() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(
        "先看我\n\n<yomi_attachments>\noutput/a.png\n</yomi_attachments>\n\n再看我",
    );
    let mut reply = buf.into_reply();
    reply.set_inline_images(vec![InlineImage {
        key: "img_k1".to_string(),
        alt: "a.png".to_string(),
        path: std::path::PathBuf::from("/tmp/a.png"),
        declared: "output/a.png".to_string(),
    }]);

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();

    let tags: Vec<&str> = elements
        .iter()
        .map(|e| e["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, vec!["markdown", "img", "markdown"]);
    assert_eq!(elements[0]["content"], "先看我");
    assert_eq!(elements[1]["img_key"], "img_k1");
    assert_eq!(elements[2]["content"], "再看我");
}

#[test]
fn render_card_drops_token_naming_no_uploaded_image() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(
        "先看我\n\n<yomi_attachments>\noutput/a.png\n</yomi_attachments>\n\n再看我",
    );
    let mut reply = buf.into_reply();
    // Upload covered a different declaration; the token names nothing.
    reply.set_inline_images(vec![InlineImage {
        key: "img_k9".to_string(),
        alt: "other.png".to_string(),
        path: std::path::PathBuf::from("/tmp/other.png"),
        declared: "other.png".to_string(),
    }]);

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();

    let tags: Vec<&str> = elements
        .iter()
        .map(|e| e["tag"].as_str().unwrap())
        .collect();
    // The unmatched token vanishes; the unanchored image appends after.
    assert_eq!(tags, vec!["markdown", "markdown", "img"]);
    assert!(!card.to_string().contains("⟦"));
    assert_eq!(elements[2]["img_key"], "img_k9");
}

#[test]
fn render_card_truncated_token_head_is_stripped_and_image_appended() {
    // Body sized so the 28KB budget cut lands inside the token: the
    // block sits after 27_970 bytes, so its minted token spans the cut.
    let mut buf = RunReplyBuffer::new();
    let text = format!(
        "{}\n\n<yomi_attachments>\noutput/aaa.png\n</yomi_attachments>\n\nTAIL",
        "a".repeat(27_968)
    );
    buf.record_model_end(&text);
    let mut reply = buf.into_reply();
    reply.set_inline_images(vec![InlineImage {
        key: "img_k1".to_string(),
        alt: "aaa.png".to_string(),
        path: std::path::PathBuf::from("/tmp/aaa.png"),
        declared: "output/aaa.png".to_string(),
    }]);

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();

    // No partial token leaks; the image falls back to the after-body slot.
    assert!(!card.to_string().contains("⟦"));
    let tags: Vec<&str> = elements
        .iter()
        .map(|e| e["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, vec!["markdown", "img"]);
    assert_eq!(elements[1]["img_key"], "img_k1");
}

#[test]
fn render_plain_strips_anchor_tokens() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("前文\n<yomi_attachments>\noutput/a.png\n</yomi_attachments>\n后文");
    let reply = buf.into_reply();

    let plain = render_plain(&reply);
    assert!(!plain.contains("⟦"));
    assert!(plain.contains("前文"));
    assert!(plain.contains("后文"));
}

#[test]
fn process_panel_strips_anchor_tokens_from_narrations() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("中间\n<yomi_attachments>\noutput/mid.png\n</yomi_attachments>");
    buf.record_model_end("这是更长一些的最终答案内容，覆盖前一条成为正文。");
    let reply = buf.into_reply();

    let card = render_card(&reply, None).unwrap();
    // The mid-run narration's token never renders (its image, when
    // uploaded, appends after the body instead).
    assert!(!card.contains("⟦"));
    assert_eq!(reply.attachments(), &["output/mid.png"]);
}

#[test]
fn render_card_token_only_body_renders_lone_img() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("<yomi_attachments>\nonly.png\n</yomi_attachments>");
    let mut reply = buf.into_reply();
    reply.set_inline_images(vec![InlineImage {
        key: "img_k1".to_string(),
        alt: "only.png".to_string(),
        path: std::path::PathBuf::from("/tmp/only.png"),
        declared: "only.png".to_string(),
    }]);

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let tags: Vec<&str> = card["body"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, vec!["img"]);
}

#[test]
fn render_card_anchors_in_both_promoted_body_texts() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(
        "第一段比较长，占位占位占位。\n<yomi_attachments>\nout/one.png\n</yomi_attachments>\n第一段尾巴。",
    );
    buf.record_model_end("第二段短。\n<yomi_attachments>\nout/two.png\n</yomi_attachments>\n尾。");
    let mut reply = buf.into_reply();
    assert_eq!(reply.body_texts().len(), 2, "longer earlier text promoted");
    reply.set_inline_images(vec![
        InlineImage {
            key: "img_k1".to_string(),
            alt: "one.png".to_string(),
            path: std::path::PathBuf::from("/tmp/one.png"),
            declared: "out/one.png".to_string(),
        },
        InlineImage {
            key: "img_k2".to_string(),
            alt: "two.png".to_string(),
            path: std::path::PathBuf::from("/tmp/two.png"),
            declared: "out/two.png".to_string(),
        },
    ]);

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    let tags: Vec<&str> = elements
        .iter()
        .map(|e| e["tag"].as_str().unwrap())
        .collect();
    assert_eq!(
        tags,
        vec!["markdown", "img", "markdown", "hr", "markdown", "img", "markdown"]
    );
    assert_eq!(elements[1]["img_key"], "img_k1");
    assert_eq!(elements[5]["img_key"], "img_k2");
}

#[test]
fn render_card_complete_token_at_truncation_boundary_survives() {
    // Cut lands right after the token's final byte: target = 28_000 -
    // suffix(16) = 27_984; the block sits so its minted token ends
    // exactly there, and the tail beyond pushes the text past the cap.
    let token = crate::utils::attachments::attachment_token("output/aaa.png");
    assert_eq!(token.len(), 36);
    let text = format!(
        "{}\n\n<yomi_attachments>\noutput/aaa.png\n</yomi_attachments>\n\n{}",
        "a".repeat(27_984 - 36 - 2),
        "T".repeat(100)
    );
    assert!(text.len() > 28_000, "must actually truncate");
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(&text);
    let mut reply = buf.into_reply();
    reply.set_inline_images(vec![InlineImage {
        key: "img_k1".to_string(),
        alt: "aaa.png".to_string(),
        path: std::path::PathBuf::from("/tmp/aaa.png"),
        declared: "output/aaa.png".to_string(),
    }]);

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let elements = card["body"]["elements"].as_array().unwrap();
    let tags: Vec<&str> = elements
        .iter()
        .map(|e| e["tag"].as_str().unwrap())
        .collect();
    // Token intact → image in place; the suffix becomes the trailing segment.
    assert_eq!(tags, vec!["markdown", "img", "markdown"]);
    assert_eq!(elements[1]["img_key"], "img_k1");
    assert!(!card.to_string().contains("⟦"));
}

#[test]
fn process_panel_skips_token_only_narrations() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("<yomi_attachments>\nmid.png\n</yomi_attachments>");
    buf.record_model_end("最终答案内容，写得长一些、确保超过 token 的长度而留在正文位置不被提升。");
    let reply = buf.into_reply();

    let card = render_card(&reply, None).unwrap();
    assert!(
        !card.contains("\"content\":\"\""),
        "no empty markdown: {card}"
    );
    assert!(!card.contains("⟦"));
}

#[test]
fn body_text_drops_token_emptied_texts_before_join() {
    // Over-drop guard: an earlier text with real content keeps the divider.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end(
        "<yomi_attachments>\nout/a.png\n</yomi_attachments>\n凑长度占位占位占位。",
    );
    buf.record_model_end("正文短。");
    let reply = buf.into_reply();
    assert_eq!(reply.body_texts().len(), 2);
    assert!(reply.body_text().unwrap().contains("---"));

    // The orphan-`---` case: token-only earlier text (32 bytes) beats the
    // short final at promotion, then flattens to nothing on plain.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("<yomi_attachments>\nout/a.png\n</yomi_attachments>");
    buf.record_model_end("正文。");
    let reply = buf.into_reply();
    assert_eq!(
        reply.body_texts().len(),
        2,
        "token-only text promoted first"
    );
    let text = reply.body_text().unwrap();
    assert_eq!(text, "正文。", "no orphan divider: {text:?}");
}

#[test]
fn render_card_no_stray_hr_when_a_text_emits_nothing() {
    // Leading case: token-only earlier text promoted first, its image
    // never uploaded → it emits nothing; the short final emits markdown.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("<yomi_attachments>\nout/a.png\n</yomi_attachments>");
    buf.record_model_end("正文。");
    let reply = buf.into_reply();
    assert_eq!(reply.body_texts().len(), 2);

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let tags: Vec<&str> = card["body"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, vec!["markdown"], "no leading hr: {card}");

    // Trailing case: real earlier text, token-only final (unmatched) —
    // no trailing hr above whatever follows.
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("正文长一些的正文，占位占位占位占位占位占位。");
    buf.record_model_end("<yomi_attachments>\nout/a.png\n</yomi_attachments>");
    let reply = buf.into_reply();
    assert_eq!(reply.body_texts().len(), 2);

    let card: serde_json::Value =
        serde_json::from_str(&render_card(&reply, None).unwrap()).unwrap();
    let tags: Vec<&str> = card["body"]["elements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["tag"].as_str().unwrap())
        .collect();
    assert_eq!(tags, vec!["markdown"], "no trailing hr: {card}");
    assert!(!card.to_string().contains("\"tag\":\"hr\""));
}

#[test]
fn render_plain_skips_token_only_narrations() {
    let mut buf = RunReplyBuffer::new();
    buf.record_model_end("<yomi_attachments>\nmid.png\n</yomi_attachments>");
    buf.record_model_end("最终答案，比 token-only 前一条长不少的正文内容。");
    let reply = buf.into_reply();

    let plain = render_plain(&reply);
    assert!(!plain.contains("⟦"));
    // The token-only mid-run narration leaves no blank line in the
    // trace section (the blank above the title is the body separator).
    let trace = &plain[plain.find('🐾').unwrap()..];
    assert!(
        trace.lines().all(|l| !l.trim().is_empty()),
        "no blank lines in trace: {plain:?}"
    );
}
