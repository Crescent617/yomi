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
/// memory for long goal-mode runs; the render cap is much lower, so this
/// only needs to keep the recent tail intact.
const BUFFER_MAX_ENTRIES: usize = 100;
/// Trace entries rendered in the panel (most recent kept).
const MAX_TRACE_ENTRIES: usize = 20;
/// Intermediate-text snippet truncation inside the trace panel.
const NARRATION_MAX_CHARS: usize = 80;
/// Tool argument summary truncation (single-line displays: inline trace
/// entries and the status-card last-tool line).
const ARG_SUMMARY_MAX_CHARS: usize = 60;
/// Arg lines shown per tool in the trace panel (long/multi-line args).
const TRACE_ARG_MAX_LINES: usize = 3;
/// Per-line arg budget in the trace panel.
const TRACE_ARG_LINE_MAX_CHARS: usize = 100;

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
    /// Arg summary for display: a short single line renders inline after the
    /// tool name; long or multi-line args render as `↳` continuation lines.
    arg_lines: Vec<String>,
    /// `None` while the tool is still running (e.g. at cancel time).
    elapsed_ms: Option<u64>,
    is_error: bool,
}

/// Per-session run buffer: chronological trace including the reply candidate.
#[derive(Debug)]
pub(crate) struct RunReplyBuffer {
    entries: Vec<TraceEntry>,
    started_at: Instant,
}

impl RunReplyBuffer {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            started_at: Instant::now(),
        }
    }

    /// Record a completed assistant text. The most recent one becomes the
    /// reply body at flush time; all earlier ones stay in the trace.
    pub(crate) fn record_text(&mut self, text: String) {
        self.push_entry(TraceEntry::Narration(text));
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
            arg_lines: summarize_args_trace(tool_name, arguments),
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
        }
        self.entries.push(entry);
    }

    /// Split into the reply body (the most recent assistant text, if any)
    /// and the remaining trace.
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
            entries,
            elapsed: self.started_at.elapsed(),
        }
    }

    /// Render the most recent `max` trace entries as markdown lines — the
    /// live status-card preview of the run trace.
    pub(crate) fn trace_preview_lines(&self, max: usize) -> Vec<String> {
        let (lines, _) = trace_lines(&self.entries, true);
        cap_trace_lines(lines, max)
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
    entries: Vec<TraceEntry>,
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
        elements.push(json!({
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
        }));
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

/// Render the trace lines (capped to the most recent) and the summary title.
fn render_trace(reply: &FinalReply, markdown: bool) -> (Vec<String>, String) {
    let (lines, stats) = trace_lines(&reply.entries, markdown);
    let lines = cap_trace_lines(lines, MAX_TRACE_ENTRIES);

    let mut title = format!("🐾 Run trace · {} tools", stats.tools);
    if stats.failed > 0 {
        let _ = write!(title, " · {} failed", stats.failed);
    }
    let _ = write!(title, " · {}", fmt_elapsed(reply.elapsed));
    (lines, title)
}

/// Cap trace lines to the most recent `max`, noting the dropped count.
fn cap_trace_lines(mut lines: Vec<String>, max: usize) -> Vec<String> {
    if lines.len() > max {
        let dropped = lines.len() - max;
        lines.drain(..dropped);
        lines.insert(0, format!("··· and {dropped} earlier entries"));
    }
    lines
}

#[derive(Default)]
struct TraceStats {
    tools: usize,
    failed: usize,
}

fn trace_lines(entries: &[TraceEntry], markdown: bool) -> (Vec<String>, TraceStats) {
    let mut stats = TraceStats::default();
    let mut lines = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            TraceEntry::Narration(text) => {
                let snippet = truncate_by_chars(&flatten_ws(text), NARRATION_MAX_CHARS, "…");
                if markdown {
                    lines.push(format!("<font color='grey'>💬 {snippet}</font>"));
                } else {
                    lines.push(format!("💬 {snippet}"));
                }
            }
            TraceEntry::Tool(tool) => {
                stats.tools += 1;
                let icon = if tool.elapsed_ms.is_none() {
                    "⏳"
                } else if tool.is_error {
                    stats.failed += 1;
                    "❌"
                } else {
                    "✅"
                };
                let mut line = if markdown {
                    format!("{icon} **{}**", tool.tool_name)
                } else {
                    format!("{icon} {}", tool.tool_name)
                };
                // Short single-line args stay inline; long or multi-line
                // args get their own continuation lines below the header.
                let inline = match tool.arg_lines.as_slice() {
                    [only] if only.chars().count() <= ARG_SUMMARY_MAX_CHARS => Some(only.as_str()),
                    _ => None,
                };
                if let Some(summary) = inline {
                    if markdown {
                        let _ = write!(line, " · `{summary}`");
                    } else {
                        let _ = write!(line, " · {summary}");
                    }
                }
                if let Some(ms) = tool.elapsed_ms {
                    let _ = write!(line, " · {}", fmt_tool_elapsed(ms));
                }
                lines.push(line);
                if inline.is_none() {
                    lines.extend(tool.arg_lines.iter().map(|arg| {
                        if markdown {
                            format!("↳ `{arg}`")
                        } else {
                            format!("↳ {arg}")
                        }
                    }));
                }
            }
        }
    }
    (lines, stats)
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

/// Multi-line arg summary for the trace panel: the arg's own line breaks are
/// preserved (whitespace flattened within each line), capped at
/// [`TRACE_ARG_MAX_LINES`] lines of [`TRACE_ARG_LINE_MAX_CHARS`] chars; a
/// trailing `…` line marks dropped lines.
fn summarize_args_trace(tool_name: &str, arguments: Option<&str>) -> Vec<String> {
    let mut lines: Vec<String> = extract_arg_text(tool_name, arguments)
        .lines()
        .map(flatten_ws)
        .filter(|line| !line.is_empty())
        .map(|line| truncate_by_chars(&line, TRACE_ARG_LINE_MAX_CHARS, "…"))
        .collect();
    if lines.len() > TRACE_ARG_MAX_LINES {
        lines.truncate(TRACE_ARG_MAX_LINES);
        lines.push("…".to_string());
    }
    lines
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
