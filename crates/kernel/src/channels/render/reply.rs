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

use crate::channels::obs::{fmt_elapsed, fmt_tokens};
use crate::utils::strs::truncate_by_chars;

/// Reply text budget in bytes. Feishu card payloads cap around 30KB; leave
/// headroom for the trace panel and the JSON envelope. Bytes, not chars —
/// a char budget would let ~3x that size through for CJK text.
const FINAL_TEXT_MAX_BYTES: usize = 28_000;
/// Trace entries kept in the buffer (oldest dropped beyond this). Bounds
/// memory for long runs; dropped entries are counted and shown
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
    /// Failed tool calls so far, counted incrementally so the title total
    /// stays true even after old entries hit the buffer cap (entry-derived
    /// counts would silently shrink). Tool totals themselves were dropped
    /// from the title in the traffic redesign (`↓`/`↑` segments).
    failed: usize,
    /// Session model / latest real token usage, mirrored into the trace
    /// title so every surface (live card, terminal receipt, reply card)
    /// renders the same summary segments.
    model: Option<String>,
    ctx_footer: Option<String>,
    /// Run-cumulative real token usage (Σ per-response prompt/completion
    /// at each `TokenUsage`): the title's `↓`/`↑` traffic segments.
    usage_in: u64,
    usage_out: u64,
    /// Last `TokenUsage` seen (message id + values): providers may emit
    /// usage multiple times per response (partial then final, same
    /// message id) — only the delta folds into the run totals.
    last_usage: Option<(crate::types::MessageId, u64, u64)>,
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
            failed: 0,
            model: None,
            ctx_footer: None,
            usage_in: 0,
            usage_out: 0,
            last_usage: None,
            attachments: Vec::new(),
            dropped: 0,
            started_at: Instant::now(),
        }
    }

    /// The session's model key, shown in the trace title.
    pub(crate) fn set_model(&mut self, model: String) {
        self.model = Some(model);
    }

    /// Real token usage at response end (title's ctx segment).
    pub(crate) fn set_ctx_footer(&mut self, total_tokens: u32, context_window: u32) {
        self.ctx_footer = Some(ctx_footer(total_tokens, context_window));
    }

    /// Fold one response's real usage into the run totals. `TokenUsage`
    /// may fire multiple times per response (same message id, partial
    /// then final values) — a repeat folds only the delta.
    pub(crate) fn add_usage(
        &mut self,
        message_id: &crate::types::MessageId,
        prompt_tokens: u32,
        completion_tokens: u32,
    ) {
        let (p, c) = (u64::from(prompt_tokens), u64::from(completion_tokens));
        if let Some((id, last_p, last_c)) = &self.last_usage {
            if id == message_id {
                self.usage_in += p.saturating_sub(*last_p);
                self.usage_out += c.saturating_sub(*last_c);
                self.last_usage = Some((message_id.clone(), p, c));
                return;
            }
        }
        self.usage_in += p;
        self.usage_out += c;
        self.last_usage = Some((message_id.clone(), p, c));
    }

    /// Run-cumulative real usage (title's `↓`/`↑` segments).
    pub(crate) fn usage(&self) -> (u64, u64) {
        (self.usage_in, self.usage_out)
    }

    /// Failed tool calls so far (title's ❌ segment).
    pub(crate) fn failed_count(&self) -> usize {
        self.failed
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
        if is_error {
            self.failed += 1;
        }
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
            failed: self.failed,
            model: self.model,
            ctx_footer: self.ctx_footer,
            usage_in: self.usage_in,
            usage_out: self.usage_out,
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
            &TraceTitle {
                steps: self.steps,
                failed: self.failed,
                elapsed: self.started_at.elapsed(),
                model: self.model.as_deref(),
                ctx_footer: self.ctx_footer.as_deref(),
                usage_in: self.usage_in,
                usage_out: self.usage_out,
                ..Default::default()
            },
            self.dropped,
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
    /// Failed tool calls this run (title ❌ counter, survives the buffer
    /// cap). Tool totals were dropped in the traffic redesign.
    failed: usize,
    /// Title tail segments mirrored from the run state (absent when the
    /// forwarder never learned them — e.g. plain platforms).
    model: Option<String>,
    ctx_footer: Option<String>,
    /// Run-cumulative real usage (title's `↓`/`↑` traffic segments).
    usage_in: u64,
    usage_out: u64,
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
        // Platform-neutral `<@USER_ID>` contract → feishu <at> syntax.
        let text =
            crate::channels::utils::rewrite_mentions(text, &|id| format!("<at id={id}></at>"));
        let text = crate::utils::strs::truncate_with_suffix(
            &text,
            FINAL_TEXT_MAX_BYTES,
            "\n\n...(truncated)",
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
    trace_panel(lines, title, false)
}

/// Collapsible run-trace panel that starts **expanded** — used on the live
/// mid-run card so the human still sees the trace streaming, while reading
/// bots skip it (collapsible panels are stripped from card text on every
/// yomi read path regardless of `expanded`).
pub(crate) fn trace_panel_element_expanded(lines: &[String], title: &str) -> serde_json::Value {
    trace_panel(lines, title, true)
}

fn trace_panel(lines: &[String], title: &str, expanded: bool) -> serde_json::Value {
    json!({
        "tag": "collapsible_panel",
        "expanded": expanded,
        "header": {
            // notation 小字标题：统计行是辅助信息，视觉上退到 trace
            // 内容行同一档（panel header title 支持 text_size，已实物
            // 验证安卓/桌面均生效）；小字已够弱化，不再叠灰色。
            "title": { "tag": "markdown", "text_size": "notation", "content": title },
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
        &TraceTitle {
            steps: reply.steps,
            failed: reply.failed,
            elapsed: reply.elapsed,
            model: reply.model.as_deref(),
            ctx_footer: reply.ctx_footer.as_deref(),
            usage_in: reply.usage_in,
            usage_out: reply.usage_out,
            ..Default::default()
        },
        reply.dropped_entries,
        markdown,
    )
}

/// Fields for the trace/stats summary line, shared by the terminal receipt
/// card, the live mid-run card, and the run stats line so all render the
/// same summary segments. Every part is optional and omitted when
/// zero/absent.
#[derive(Default)]
pub(crate) struct TraceTitle<'a> {
    pub steps: usize,
    pub failed: usize,
    pub elapsed: std::time::Duration,
    pub model: Option<&'a str>,
    pub ctx_footer: Option<&'a str>,
    /// Run-cumulative real usage (`↓`/`↑` segments; 0 = not yet reported).
    pub usage_in: u64,
    pub usage_out: u64,
    pub out_estimate: u32,
}

/// Build the ordered summary segments, split into the always-dark head
/// (💬 steps, traffic totals, ❌ failed) and the technical tail
/// (model, ctx) that callers may grey out. Zero/absent parts omitted;
/// the elapsed prefix is left to the caller (its icon differs per
/// surface). Tool totals are deliberately not shown — the traffic
/// segments (`12.3k↑` prompt sent / `636↓` completion received, `~`
/// marking a live estimate until the first response's real usage lands)
/// carry the run's cost shape. Arrows are user-centric, speedtest-style:
/// ↑ = up to the model, ↓ = back down from it.
pub(crate) fn summary_segments(t: &TraceTitle<'_>) -> (Vec<String>, Vec<String>) {
    let mut head = Vec::new();
    if t.steps > 0 {
        head.push(format!("💬 {}", t.steps));
    }
    if t.usage_in > 0 {
        head.push(format!("{}↑", fmt_tokens(t.usage_in)));
    }
    // ↓ shows real totals plus the in-flight estimate (the estimate's
    // folded run part is zeroed when real usage lands, so no double
    // counting); `~` marks "all estimate, no real usage yet".
    let out = t.usage_out + u64::from(t.out_estimate);
    if out > 0 {
        if t.usage_out == 0 {
            head.push(format!("~{}↓", fmt_tokens(out)));
        } else {
            head.push(format!("{}↓", fmt_tokens(out)));
        }
    }
    if t.failed > 0 {
        head.push(format!("❌ {}", t.failed));
    }
    let mut tail = Vec::new();
    if let Some(m) = t.model {
        tail.push(m.to_string());
    }
    if let Some(c) = t.ctx_footer {
        tail.push(c.to_string());
    }
    (head, tail)
}

/// Render the shared trace panel title: `🐾 Xs` plus the summary segments.
/// No `<font>` markup — the panel wraps its own title in grey, so callers
/// must not pre-wrap (nesting breaks it).
pub(crate) fn render_trace_title(t: &TraceTitle<'_>) -> String {
    let (head, tail) = summary_segments(t);
    let mut parts = vec![format!("🐾 {}", fmt_elapsed(t.elapsed))];
    parts.extend(head);
    parts.extend(tail);
    parts.join(" · ")
}

/// Shared trace renderer behind [`render_trace`] and
/// [`RunReplyBuffer::full_trace_render`].
fn render_trace_parts(
    entries: &[TraceEntry],
    title: &TraceTitle<'_>,
    dropped_entries: usize,
    markdown: bool,
) -> (Vec<String>, String) {
    let mut lines = trace_lines(entries, markdown);
    if dropped_entries > 0 {
        lines.insert(0, dropped_marker(dropped_entries));
    }
    (lines, render_trace_title(title))
}

/// The marker line noting trace entries dropped at the buffer/display cap.
fn dropped_marker(dropped: usize) -> String {
    format!("··· and {dropped} earlier entries")
}

/// Real token usage as the title's ctx segment (`35%`), shared by the
/// live stats line and the trace title on every surface.
pub(crate) fn ctx_footer(total_tokens: u32, context_window: u32) -> String {
    // Percentage only: the window size varies across models, and the
    // ratio is what fits the compact trace title.
    let pct = (f64::from(total_tokens) * 100.0 / f64::from(context_window.max(1))).round() as u32;
    format!("{pct}%")
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
    // cron 的参数是"动词 + 目标"结构（action/name/id/schedule），单一 key
    // 摘要不出信息：组合 action · 目标(name 优先，id 兜底) · schedule。
    if tool_name == "cron" {
        let action = pick("action").unwrap_or_default();
        let target = ["name", "id"]
            .iter()
            .find_map(|k| pick(k))
            .unwrap_or_default();
        let schedule = pick("schedule").unwrap_or_default();
        return [action, target, schedule]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
    }
    primary_arg_key(tool_name)
        .and_then(pick)
        .or_else(|| FALLBACK_ARG_KEYS.iter().find_map(|key| pick(key)))
        // 键表全落空的不认识工具：原始 JSON 直接上卡（调用方统一扁平化
        // 并按 ARG_SUMMARY_MAX_CHARS 截断），好过整行空白。
        .unwrap_or(raw)
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
