//! Final-reply buffering for external channels.
//!
//! A run (see `obs.rs` for the lifecycle definition) may produce several
//! assistant texts, but only the **last** one is delivered; earlier texts
//! and tool calls are collected into a chronological trace rendered as a
//! collapsible panel (Feishu card JSON 2.0 `collapsible_panel`, requires
//! Feishu client V7.9+), or appended as plain-text lines on platforms
//! without card support. With observability enabled the run's status card
//! **morphs** into this final reply on settlement — one message per run;
//! otherwise the reply is sent as a new message bubble.

use serde_json::json;
use std::fmt::Write as _;
use std::time::{Duration, Instant};

use super::obs::fmt_elapsed;
use crate::utils::strs::truncate_by_chars;

/// Reply text budget in bytes. Feishu card payloads cap around 30KB; leave
/// headroom for the trace panel and the JSON envelope. Bytes, not chars —
/// a char budget would let ~3x that size through for CJK text.
const FINAL_TEXT_MAX_BYTES: usize = 28_000;
/// Trace entries kept in the buffer (oldest dropped beyond this). Bounds
/// memory for long goal-mode runs; dropped entries are counted and shown
/// as a marker line at render time.
const BUFFER_MAX_ENTRIES: usize = 100;
/// Intermediate-text snippet truncation inside the trace panel.
const NARRATION_MAX_CHARS: usize = 120;
/// Tool argument summary truncation (single-line displays: inline trace
/// entries). Long enough for typical shell commands and paths to stay
/// recognizable; the card payload budget has ample headroom for it.
const ARG_SUMMARY_MAX_CHARS: usize = 120;

/// Preferred argument key per tool for the one-line summary.
fn primary_arg_key(tool_name: &str) -> Option<&'static str> {
    Some(match tool_name {
        "shell" => "command",
        "read" | "edit" => "path",
        "write" => "file_path",
        "glob" | "grep" => "pattern",
        "web_fetch" => "url",
        "web_search" => "query",
        "agent" => "description",
        _ => return None,
    })
}

/// Fallback keys tried in order for tools without a dedicated mapping.
const FALLBACK_ARG_KEYS: &[&str] = &[
    "command",
    "path",
    "file_path",
    "pattern",
    "url",
    "query",
    "description",
    "prompt",
    "title",
    "message",
    "content",
    "text",
];

/// One chronological trace entry: an intermediate assistant text or a tool call.
#[derive(Debug)]
enum TraceEntry {
    /// One assistant text. Every text recorded during the run is a Narration
    /// until [`RunReplyBuffer::into_reply`] promotes the latest one to the
    /// reply body.
    Narration(String),
    Tool(ToolTrace),
}

#[derive(Debug)]
struct ToolTrace {
    tool_id: String,
    tool_name: String,
    /// One-line arg summary (whitespace flattened, capped), rendered
    /// inline after the tool name; empty when there is nothing to show.
    arg_summary: String,
    /// `None` while the tool is still running (e.g. at cancel time).
    elapsed_ms: Option<u64>,
    is_error: bool,
}

/// Per-session run buffer: chronological trace including the reply candidate.
#[derive(Debug)]
pub(crate) struct RunReplyBuffer {
    entries: Vec<TraceEntry>,
    /// Completed model responses (`ModelEvent::End` count) — the run's
    /// steps, including tool-call-only turns that produced no text.
    steps: usize,
    /// Attachment paths declared via `<yomi_attachments>` blocks in
    /// assistant texts. Blocks are stripped at record time so the XML
    /// never renders on the platform (trace snippets, live card preview,
    /// reply body).
    attachments: Vec<String>,
    /// Entries dropped at the buffer cap (oldest first) — surfaced as a
    /// "··· and N earlier entries" marker at render time.
    dropped: usize,
    started_at: Instant,
}

impl RunReplyBuffer {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            steps: 0,
            attachments: Vec::new(),
            dropped: 0,
            started_at: Instant::now(),
        }
    }

    /// Record a completed model response (`ModelEvent::End`) — one step of
    /// the run, whether or not it produced text (tool-call-only turns
    /// count too). A non-empty text joins the trace: the most recent one
    /// becomes the reply body at flush time, all earlier ones stay as
    /// narrations. Each `<yomi_attachments>` block outside a fenced code
    /// block is stripped from the text and its paths collected for file
    /// delivery; a text that held only blocks leaves no narration.
    pub(crate) fn record_model_end(&mut self, text: &str) {
        self.steps += 1;
        let (text, paths) = crate::utils::attachments::parse_attachments(text);
        self.attachments.extend(paths);
        if !text.is_empty() {
            self.push_entry(TraceEntry::Narration(text));
        }
    }

    pub(crate) fn record_tool_start(
        &mut self,
        tool_id: &str,
        tool_name: &str,
        arguments: Option<&str>,
    ) {
        self.push_entry(TraceEntry::Tool(ToolTrace {
            tool_id: tool_id.to_string(),
            tool_name: tool_name.to_string(),
            arg_summary: truncate_by_chars(
                &flatten_ws(&extract_arg_text(tool_name, arguments)),
                ARG_SUMMARY_MAX_CHARS,
                "…",
            ),
            elapsed_ms: None,
            is_error: false,
        }));
    }

    pub(crate) fn record_tool_end(&mut self, tool_id: &str, elapsed_ms: u64, is_error: bool) {
        if let Some(TraceEntry::Tool(tool)) = self
            .entries
            .iter_mut()
            .rev()
            .find(|e| matches!(e, TraceEntry::Tool(t) if t.tool_id == tool_id))
        {
            tool.elapsed_ms = Some(elapsed_ms);
            tool.is_error = is_error;
        }
    }

    fn push_entry(&mut self, entry: TraceEntry) {
        if self.entries.len() >= BUFFER_MAX_ENTRIES {
            let overflow = self.entries.len() + 1 - BUFFER_MAX_ENTRIES;
            self.entries.drain(..overflow);
            self.dropped += overflow;
        }
        self.entries.push(entry);
    }

    /// Split into the reply body (the most recent assistant text, if any)
    /// and the remaining trace, carrying the attachment paths collected
    /// from `<yomi_attachments>` blocks at record time.
    pub(crate) fn into_reply(self) -> FinalReply {
        let body_idx = self
            .entries
            .iter()
            .rposition(|e| matches!(e, TraceEntry::Narration(_)));
        let mut entries = self.entries;
        let text = body_idx.map(|idx| {
            let TraceEntry::Narration(text) = entries.remove(idx) else {
                unreachable!("rposition matched a Narration");
            };
            text
        });
        FinalReply {
            text,
            steps: self.steps,
            attachments: self.attachments,
            entries,
            dropped_entries: self.dropped,
            elapsed: self.started_at.elapsed(),
        }
    }

    /// Render the most recent `max` trace entries as markdown lines — the
    /// live status-card preview of the run trace.
    pub(crate) fn trace_preview_lines(&self, max: usize) -> Vec<String> {
        let capped = self.entries.len().saturating_sub(max);
        let mut lines = trace_lines(&self.entries[capped..], true);
        let dropped = self.dropped + capped;
        if dropped > 0 {
            lines.insert(0, dropped_marker(dropped));
        }
        lines
    }

    /// The full trace (title + all entry lines) as rendered on the reply
    /// card's trace panel — used by the terminal receipt card, which keeps
    /// the whole run trace (the reply text stays a narration here: only
    /// `into_reply` promotes it). `None` when nothing was recorded.
    pub(crate) fn full_trace_render(&self) -> Option<(Vec<String>, String)> {
        if self.entries.is_empty() {
            return None;
        }
        Some(render_trace_parts(
            &self.entries,
            self.steps,
            self.dropped,
            self.started_at.elapsed(),
            true,
        ))
    }

    /// Completed model responses so far this run (the run's steps; the
    /// in-progress response doesn't count until its `ModelEvent::End`).
    pub(crate) fn step_count(&self) -> usize {
        self.steps
    }
}

impl Default for RunReplyBuffer {
    fn default() -> Self {
        Self::new()
    }
}

/// The deliverable reply: optional final text + the run trace (may be empty).
pub(crate) struct FinalReply {
    text: Option<String>,
    /// Completed model responses this run (`ModelEvent::End` count) — the
    /// step count shown in the trace title.
    steps: usize,
    /// Attachment paths collected from `<yomi_attachments>` blocks in the
    /// run's assistant texts (blocks already stripped from the recorded
    /// texts).
    attachments: Vec<String>,
    entries: Vec<TraceEntry>,
    /// Trace entries dropped at the buffer cap (shown as a marker line).
    dropped_entries: usize,
    elapsed: Duration,
}

impl FinalReply {
    /// Whether a run trace accompanies the text (drives card vs. plain send).
    pub(crate) fn has_trace(&self) -> bool {
        !self.entries.is_empty()
    }

    /// The final text, when the run produced any.
    pub(crate) fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// The bare final text (used when the trace is disabled by config).
    pub(crate) fn into_text(self) -> Option<String> {
        self.text
    }

    /// Attachment paths declared in the final text.
    pub(crate) fn attachments(&self) -> &[String] {
        &self.attachments
    }

    /// Take the declared attachment paths out of the reply (consumed by
    /// the hub's file-delivery step).
    pub(crate) fn take_attachments(&mut self) -> Vec<String> {
        std::mem::take(&mut self.attachments)
    }

    /// Append a delivery note (e.g. an attachment failure) to the reply
    /// text, so it surfaces on the platform instead of vanishing.
    pub(crate) fn push_note(&mut self, note: &str) {
        match &mut self.text {
            Some(text) => {
                text.push_str("\n\n");
                text.push_str(note);
            }
            None => self.text = Some(note.to_string()),
        }
    }

    /// Test-only constructor helper: set the attachments list directly.
    #[cfg(test)]
    pub(crate) fn set_attachments(&mut self, attachments: Vec<String>) {
        self.attachments = attachments;
    }
}

/// Render the Feishu reply card (schema 2.0, no header): an optional notice
/// line (e.g. error summary for abnormal endings), the final text, and a
/// collapsible run-trace panel (default collapsed). Returns `None` when
/// there is nothing to show.
pub(crate) fn render_card(reply: &FinalReply, notice: Option<&str>) -> Option<String> {
    let mut elements = Vec::new();
    if let Some(notice) = notice {
        elements.push(json!({ "tag": "markdown", "content": notice }));
    }

    if let Some(text) = reply.text() {
        let text = crate::utils::strs::truncate_with_suffix(
            text,
            FINAL_TEXT_MAX_BYTES,
            "\n\n...(内容已截断)",
        );
        elements.push(json!({ "tag": "markdown", "content": text }));
    }

    if !reply.entries.is_empty() {
        let (lines, title) = render_trace(reply, true);
        elements.push(trace_panel_element(&lines, &title));
    }

    if elements.is_empty() {
        return None;
    }
    Some(
        json!({
            "schema": "2.0",
            "body": { "elements": elements },
        })
        .to_string(),
    )
}

/// Render the plain-text fallback (platforms without card support): the
/// final text followed by the trace as plain lines.
pub(crate) fn render_plain(reply: &FinalReply) -> String {
    let mut out = String::new();
    if let Some(text) = reply.text() {
        out.push_str(text);
    }
    if !reply.entries.is_empty() {
        let (lines, title) = render_trace(reply, false);
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        let _ = writeln!(out, "{title}");
        out.push_str(&lines.join("\n"));
    }
    out
}

/// Collapsible run-trace panel (default collapsed) shared by the reply
/// card and the terminal receipt card.
pub(crate) fn trace_panel_element(lines: &[String], title: &str) -> serde_json::Value {
    json!({
        "tag": "collapsible_panel",
        "expanded": false,
        "header": {
            "title": { "tag": "markdown", "content": format!("<font color='grey'>{title}</font>") },
            "vertical_align": "center",
            "padding": "4px 0px 4px 8px",
        },
        "vertical_spacing": "4px",
        "padding": "0px 0px 0px 8px",
        "elements": [
            { "tag": "markdown", "text_size": "notation", "content": lines.join("\n") },
        ],
    })
}

/// Render the trace lines (all of them — entries are single-line, so the
/// full run stays compact) and the summary title.
fn render_trace(reply: &FinalReply, markdown: bool) -> (Vec<String>, String) {
    render_trace_parts(
        &reply.entries,
        reply.steps,
        reply.dropped_entries,
        reply.elapsed,
        markdown,
    )
}

/// Shared trace renderer behind [`render_trace`] and
/// [`RunReplyBuffer::full_trace_render`].
fn render_trace_parts(
    entries: &[TraceEntry],
    steps: usize,
    dropped_entries: usize,
    elapsed: Duration,
    markdown: bool,
) -> (Vec<String>, String) {
    let stats = trace_stats(entries);
    let mut lines = trace_lines(entries, markdown);
    if dropped_entries > 0 {
        lines.insert(0, dropped_marker(dropped_entries));
    }

    let mut title = format!(
        "🐾 Trace · {} steps · {} tools · {}",
        steps,
        stats.tools,
        fmt_elapsed(elapsed)
    );
    if stats.failed > 0 {
        let _ = write!(title, " · {} failed", stats.failed);
    }
    (lines, title)
}

/// The marker line noting trace entries dropped at the buffer/display cap.
fn dropped_marker(dropped: usize) -> String {
    format!("··· and {dropped} earlier entries")
}

/// Totals over the whole trace (title summary), independent of how many
/// entries end up rendered.
fn trace_stats(entries: &[TraceEntry]) -> TraceStats {
    let mut stats = TraceStats::default();
    for entry in entries {
        if let TraceEntry::Tool(tool) = entry {
            stats.tools += 1;
            if tool.is_error {
                stats.failed += 1;
            }
        }
    }
    stats
}

#[derive(Default)]
struct TraceStats {
    tools: usize,
    failed: usize,
}

fn trace_lines(entries: &[TraceEntry], markdown: bool) -> Vec<String> {
    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            TraceEntry::Narration(text) => {
                let snippet = truncate_by_chars(&flatten_ws(text), NARRATION_MAX_CHARS, "…");
                if markdown {
                    lines.push(format!(
                        "<font color='grey'>💬 {}</font>",
                        md_safe(&snippet)
                    ));
                } else {
                    lines.push(format!("💬 {snippet}"));
                }
            }
            TraceEntry::Tool(tool) => {
                let icon = if tool.elapsed_ms.is_none() {
                    "⏳"
                } else if tool.is_error {
                    "❌"
                } else {
                    "✅"
                };
                let mut line = if markdown {
                    format!("{icon} **{}**", tool.tool_name)
                } else {
                    format!("{icon} {}", tool.tool_name)
                };
                if !tool.arg_summary.is_empty() {
                    let summary = &tool.arg_summary;
                    if markdown {
                        let _ = write!(line, " · `{}`", md_safe(summary));
                    } else {
                        let _ = write!(line, " · {summary}");
                    }
                }
                if let Some(ms) = tool.elapsed_ms {
                    let _ = write!(line, " · {}", fmt_tool_elapsed(ms));
                }
                lines.push(line);
            }
        }
    }
    lines
}

/// Extract the display text for a tool call's args: the tool's primary
/// argument (command / path / pattern / …), a well-known fallback key, or
/// the raw payload. May contain newlines; no length cap is applied here —
/// callers pick a one-line or multi-line presentation budget.
fn extract_arg_text(tool_name: &str, arguments: Option<&str>) -> String {
    let Some(raw) = arguments.map(str::trim).filter(|s| !s.is_empty()) else {
        return String::new();
    };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return raw.to_string();
    };
    let pick = |key: &str| {
        map.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
    };
    primary_arg_key(tool_name)
        .and_then(pick)
        .or_else(|| FALLBACK_ARG_KEYS.iter().find_map(|key| pick(key)))
        .unwrap_or_default()
        .to_string()
}

/// Collapse all whitespace runs (including newlines) into single spaces.
pub(crate) fn flatten_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 把动态文本里的 markdown 结构字符换成全角：narration / 工具摘要 /
/// whisper 的内容原文可能带 `` ` ``、`**`、`<`，截断也可能把成对标记
/// 切成未闭合——任一情况都会让飞书把整个元素按纯文本回退（所有标签
/// 漏成字面量）。仅在 markdown 渲染时调用；结构性标记（font、加粗、
/// 行内码）由渲染方自己加，不受影响。
pub(crate) fn md_safe(text: &str) -> String {
    text.replace('<', "＜")
        .replace('>', "＞")
        .replace('`', "｀")
        .replace('*', "＊")
}

fn fmt_tool_elapsed(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        fmt_elapsed(Duration::from_millis(ms))
    }
}

#[cfg(test)]
#[path = "reply_test.rs"]
mod tests;
