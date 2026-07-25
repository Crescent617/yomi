//! Run observability for external channels: status card + run receipts.
//!
//! A "run" is bracketed by `AgentEvent::Lifecycle(Running)` and
//! `AgentEvent::Lifecycle(Stopped)` (see `docs/design/feishu-channel-observability.md`).
//! Run state is tracked from `Running`, and the status card is materialized
//! on the first `ToolEvent::Start` or the first model output chunk (text or
//! thinking) — since the card morphs into the final reply on settlement,
//! every run is exactly one message (two when the user posted mid-run: the
//! card freezes as a terminal receipt and the reply lands at the bottom).
//! While running, the card body shows the stats line, the live run trace
//! (most recent tool entries), and a tail of the assistant's current text
//! ("whisper"); a rotating placeholder fills the fresh card until the
//! first tool or text arrives, and idle phase titles rotate for fun.
//! On settlement the card **morphs** into the final reply (last text +
//! run-trace panel, no header; abnormal endings get a notice line in the
//! content). With mid-run posts the card freezes as a terminal receipt
//! and the reply lands at the bottom as bare text (the trace was already
//! live on the card during the run). Runs without a reply (crash / lost
//! events) freeze into a terminal header style instead. User messages
//! received during the run are recorded as receipts — used only for the
//! mid-run post detection (morph vs. new-message settle); no reactions
//! are sent at settlement.

use crate::event::{AgentEvent, AgentStatus, Event, ModelEvent, StopReason, ToolEvent};
use crate::types::SessionId;
use crate::utils::strs::{tail_by_chars, truncate_by_chars};
use dashmap::DashMap;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

use super::reply::{self, FinalReply};
use super::PlatformAdapter;

/// Minimum interval between two in-place card updates (low-frequency updates).
const PATCH_MIN_INTERVAL: Duration = Duration::from_secs(3);
/// Error summary truncation on terminal cards.
const ERROR_MAX_CHARS: usize = 200;
/// Whisper buffer cap (tail kept); bounds memory for long streamed answers.
const WHISPER_BUFFER_CHARS: usize = 200;
/// Max length (chars, ellipsis included) for dynamic text lines on the
/// status card body (whisper).
const STATUS_TEXT_MAX_CHARS: usize = 100;
/// Trace entries shown live on the status card (most recent kept).
const STATUS_TRACE_MAX_ENTRIES: usize = 10;
/// Fresh-card placeholder lines (no tools or text yet): a random -ing
/// verb, plain text (the title already carries the fun/emoji).
const IDLE_PLACEHOLDERS: &[&str] = &[
    "Pondering…",
    "Cogitating…",
    "Mulling…",
    "Brewing…",
    "Sniffing…",
    "Scheming…",
];

/// Idle-phase card titles; a random one is picked on each render (free
/// animation, no extra API calls).
const THINKING_TITLES: &[&str] = &[
    "🐹 Chewing on it…",
    "🧠 Pondering…",
    "🤔 Mulling it over…",
    "🌩️ Brainstorming…",
    "💭 Deep in thought…",
    "💭 Thinking…",
];
const TYPING_TITLES: &[&str] = &[
    "🐾 Typing…",
    "✍️ Scribbling…",
    "📝 Drafting…",
    "⌨️ Hammering the keys…",
];

/// Pick a random title (thinking/typing phases get a fresh one per render).
fn random_title(titles: &[&'static str]) -> &'static str {
    use rand::prelude::IndexedRandom;
    titles
        .choose(&mut rand::rng())
        .expect("title list is non-empty")
}

/// Card title phase. Idle phases draw a random fun title at render time;
/// informative phases carry their payload verbatim.
enum Phase {
    Thinking,
    Typing,
    /// Humanized tool name (e.g. "Bash").
    Tool(String),
    /// Retry/error/goal/compact/fallback — informative text as-is.
    Text(String),
}

/// The card title for the current phase.
fn phase_title(s: &ObsCardState) -> String {
    match &s.phase {
        Phase::Thinking => random_title(THINKING_TITLES).to_string(),
        Phase::Typing => random_title(TYPING_TITLES).to_string(),
        Phase::Tool(name) => format!("🐹 {name}…"),
        Phase::Text(text) => text.clone(),
    }
}

/// Per-session live status card state.
struct ObsCardState {
    status_msg_id: String,
    adapter: Arc<dyn PlatformAdapter>,
    /// Routing captured when the run started (card anchor).
    chat_id: String,
    reply_msg_id: Option<String>,
    started_at: Instant,
    /// Card title phase (icon + phase, rotated for idle phases).
    phase: Phase,
    /// Total tool executions (per-tool breakdown is not kept).
    tool_count: u32,
    /// The full run trace shown live on the status card.
    trace: reply::RunReplyBuffer,
    /// Live tail of the assistant's in-progress text output ("whisper") —
    /// the one piece of not-yet-finished content on the card. Completed
    /// texts move into `trace` as narrations at `ModelEvent::End`.
    whisper: String,
    /// Any text chunk seen this run (never cleared): gates the fresh-card
    /// placeholder so it can't reappear after a Request boundary cleared
    /// the whisper.
    seen_text: bool,
    token_footer: Option<String>,
    last_patch_at: Instant,
    /// Set when the materialize send failed — no more attempts this run
    /// (card APIs are best-effort; don't storm a struggling API endpoint).
    send_failed: bool,
}

impl ObsCardState {
    fn new(adapter: Arc<dyn PlatformAdapter>, chat_id: &str, reply_msg_id: Option<&str>) -> Self {
        let now = Instant::now();
        Self {
            status_msg_id: String::new(),
            adapter,
            chat_id: chat_id.to_string(),
            reply_msg_id: reply_msg_id.map(str::to_string),
            started_at: now,
            phase: Phase::Thinking,
            tool_count: 0,
            trace: reply::RunReplyBuffer::new(),
            whisper: String::new(),
            seen_text: false,
            token_footer: None,
            last_patch_at: now,
            send_failed: false,
        }
    }

    /// Append a streamed text delta to the whisper (tail kept).
    fn push_whisper(&mut self, delta: &str) {
        self.whisper.push_str(delta);
        if self.whisper.chars().count() > WHISPER_BUFFER_CHARS {
            self.whisper = tail_by_chars(&self.whisper, WHISPER_BUFFER_CHARS);
        }
    }
}

/// Terminal settlement kind.
enum Settle {
    Completed,
    Failed(String),
    Cancelled,
    MaxIterations(usize),
    /// Watchdog settlement — session agent is no longer alive
    /// (crash / panic / lost `Stopped`).
    Timeout,
}

impl Settle {
    /// Body notice line for abnormal endings, shown in the morphed card
    /// content (the card has no header; errors go into the content).
    /// `None` for completed/cancelled runs.
    fn notice(&self) -> Option<String> {
        match self {
            Settle::Completed | Settle::Cancelled => None,
            Settle::Failed(error) => Some(format!("❌ {}", error_line(error))),
            Settle::MaxIterations(reached) => {
                Some(format!("❌ Max iterations reached ({reached})"))
            }
            Settle::Timeout => Some("⏰ Session lost (timed out)".to_string()),
        }
    }
}

/// Truncated one-line error summary for card bodies — the single truncation
/// budget shared by the morphed-card notice and the terminal card.
fn error_line(error: &str) -> String {
    format!(
        "**Error**  {}",
        truncate_by_chars(error, ERROR_MAX_CHARS, "…")
    )
}

/// Tracks per-session observability state and drives the platform adapter.
pub(crate) struct ObsTracker {
    states: DashMap<SessionId, ObsCardState>,
    /// IDs of user messages received during each run (drives the mid-run
    /// post detection); cleared at settlement.
    receipts: DashMap<SessionId, Vec<String>>,
    patch_interval: Duration,
}

impl ObsTracker {
    pub(crate) fn new() -> Self {
        Self {
            states: DashMap::new(),
            receipts: DashMap::new(),
            patch_interval: PATCH_MIN_INTERVAL,
        }
    }

    #[cfg(test)]
    fn with_patch_interval(patch_interval: Duration) -> Self {
        Self {
            patch_interval,
            ..Self::new()
        }
    }

    /// Record a user message posted while the session's agent is running
    /// (hub only records then), for the mid-run post detection.
    pub(crate) fn record_receipt(&self, session_id: &SessionId, message_id: String) {
        self.receipts
            .entry(session_id.clone())
            .or_default()
            .push(message_id);
    }

    /// Whether the user posted messages mid-run — receipts hold only
    /// messages recorded while the agent was running (run triggers are never
    /// recorded, and commands never record receipts), so any receipt means
    /// the reply should land below their messages as a new message instead
    /// of morphing the status card.
    pub(crate) fn has_mid_run_posts(&self, session_id: &SessionId) -> bool {
        self.receipts.get(session_id).is_some_and(|r| !r.is_empty())
    }

    /// Whether the run's status card never materialized (send failed or
    /// never sent): the trace never went live, so the final delivery
    /// should keep it.
    pub(crate) fn card_missing(&self, session_id: &SessionId) -> bool {
        self.states
            .get(session_id)
            .is_none_or(|s| s.status_msg_id.is_empty() || s.send_failed)
    }

    /// Feed one session event. Cheap state updates happen on every event;
    /// card updates are throttled to `patch_interval`.
    pub(crate) async fn handle_event(
        &self,
        adapter: &Arc<dyn PlatformAdapter>,
        session_id: &SessionId,
        chat_id: &str,
        reply_msg_id: Option<&str>,
        event: &Event,
    ) {
        match event {
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Running,
            }) => {
                // Running fires per turn; only the first one starts tracking.
                // The card itself is materialized lazily on the first tool
                // start or the first model output chunk.
                if self.states.contains_key(session_id) {
                    return;
                }
                self.states.insert(
                    session_id.clone(),
                    ObsCardState::new(Arc::clone(adapter), chat_id, reply_msg_id),
                );
            }
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Stopped { reason },
            }) => {
                // Degenerate path: settle without a reply (crash/lost
                // events). The hub forwarder calls `handle_stopped` with
                // the buffered reply instead.
                self.handle_stopped(session_id, reason, None).await;
            }
            Event::Tool(ToolEvent::Start {
                tool_id,
                tool_name,
                arguments,
                ..
            }) => {
                let tool_id = tool_id.clone();
                let tool_name = tool_name.clone();
                let arguments = arguments.clone();
                self.update_running(session_id, |s| {
                    s.tool_count += 1;
                    s.phase = Phase::Tool(humanize_tool_name(&tool_name));
                    s.trace
                        .record_tool_start(&tool_id, &tool_name, arguments.as_deref());
                })
                .await;
                // First tool of a run materializes the status card.
                self.materialize_card(session_id).await;
            }
            Event::Tool(ToolEvent::End {
                tool_id,
                elapsed_ms,
                is_error,
                ..
            }) => {
                self.update_running(session_id, |s| {
                    s.trace.record_tool_end(tool_id, *elapsed_ms, *is_error);
                    // Back to thinking until the next model request.
                    s.phase = Phase::Thinking;
                })
                .await;
            }
            Event::Agent(AgentEvent::Retrying {
                attempt,
                max_attempts,
                reason,
            }) => {
                let phase = Phase::Text(format!(
                    "🔁 Retrying {attempt}/{max_attempts}: {}",
                    truncate_by_chars(reason, 30, "…")
                ));
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Agent(AgentEvent::Error { error, .. }) => {
                let phase = Phase::Text(format!("⚠️ Error: {}", truncate_by_chars(error, 30, "…")));
                // Never settles — a mid-retry error may still recover (see design §3).
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Agent(AgentEvent::GoalUpdated { status, .. }) => {
                let phase = Phase::Text(format!("🎯 Goal: {status}"));
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Model(ModelEvent::Compacting { active }) => {
                let active = *active;
                self.update_running(session_id, |s| {
                    s.phase = if active {
                        Phase::Text("📦 Compacting context…".to_string())
                    } else {
                        Phase::Thinking
                    };
                })
                .await;
            }
            Event::Model(ModelEvent::Fallback { from, to, .. }) => {
                let phase = Phase::Text(format!("↪️ Fallback: {from} → {to}"));
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Model(ModelEvent::Request { .. }) => {
                // A new model call (re-)starts: thinking until chunks arrive.
                self.update_running(session_id, |s| {
                    s.phase = Phase::Thinking;
                    s.whisper.clear();
                })
                .await;
            }
            Event::Model(ModelEvent::Chunk { content, .. }) => {
                let (phase, delta) = match content {
                    crate::event::ContentChunk::Text(text) => (Phase::Typing, Some(text.clone())),
                    crate::event::ContentChunk::Thinking { .. }
                    | crate::event::ContentChunk::RedactedThinking => (Phase::Thinking, None),
                };
                self.update_running(session_id, |s| {
                    s.phase = phase;
                    if let Some(delta) = delta {
                        s.seen_text = true;
                        s.push_whisper(&delta);
                    }
                })
                .await;
                // Materialize on the first model output of any kind (text or
                // thinking): the card shows up as soon as the model starts
                // responding and later morphs into the final reply — one
                // message per run.
                self.materialize_card(session_id).await;
            }
            Event::Model(ModelEvent::End { content, .. }) => {
                // The completed text joins the trace as a narration entry
                // (self-heals chunk loss on the bus — the full text is
                // authoritative); the whisper always clears for the next
                // turn instead of duplicating or going stale.
                let text = super::blocks_to_text(content);
                self.update_running(session_id, |s| {
                    if !text.is_empty() {
                        s.trace.record_text(&text);
                    }
                    s.whisper.clear();
                })
                .await;
            }
            Event::Model(ModelEvent::TokenUsage {
                total_tokens,
                context_window,
                ..
            }) => {
                let footer = format!("ctx: {} / {}", fmt_k(*total_tokens), fmt_k(*context_window));
                self.update_running(session_id, |s| s.token_footer = Some(footer))
                    .await;
            }
            // Other events carry no card-visible state.
            _ => {}
        }
    }

    /// Settle a run on `Lifecycle(Stopped)`: morph the status card into the
    /// final reply (one message per run). Returns the reply back when
    /// nothing was settled (no run state, or the settle send failed), so
    /// the caller can fall back to a plain send.
    pub(crate) async fn handle_stopped(
        &self,
        session_id: &SessionId,
        reason: &StopReason,
        reply: Option<FinalReply>,
    ) -> Option<FinalReply> {
        let settle = match reason {
            StopReason::Completed { .. } => Settle::Completed,
            StopReason::Cancelled { .. } => Settle::Cancelled,
            StopReason::Failed { error } => Settle::Failed(error.clone()),
            StopReason::MaxIterations { reached } => Settle::MaxIterations(*reached),
        };
        self.settle_card(session_id, &settle, reply).await
    }

    /// Watchdog settlement for a session whose agent died (crash / lost
    /// `Stopped`): settle the card with whatever reply state remains.
    /// Returns the reply back when nothing was settled.
    pub(crate) async fn handle_timeout(
        &self,
        session_id: &SessionId,
        reply: Option<FinalReply>,
    ) -> Option<FinalReply> {
        self.settle_card(session_id, &Settle::Timeout, reply).await
    }

    /// Settle cards whose session no longer has a live, non-idle agent
    /// (called periodically from the event forwarder; covers agent crash /
    /// lost events where no `Stopped` ever arrives). Liveness is queried,
    /// not inferred from event gaps, so long tool calls never false-positive.
    ///
    /// Race note: a `Stopped` already queued in the event bus can lose to
    /// this sweep (card settles as timed-out a beat early); receipts are
    /// still settled correctly when the real `Stopped` arrives.
    pub(crate) async fn sweep_dead_sessions(&self, is_alive: impl Fn(&SessionId) -> bool) {
        let dead: Vec<SessionId> = self
            .states
            .iter()
            .filter(|e| !is_alive(e.key()))
            .map(|e| e.key().clone())
            .collect();
        for sid in dead {
            self.settle_card(&sid, &Settle::Timeout, None).await;
        }
    }

    // ── internals ─────────────────────────────────────────────────────

    /// Send the status card if not yet materialized (triggered by the first
    /// tool start or the first model output chunk). State is tracked from
    /// `Running`, so the initial render already carries earlier phases,
    /// token usage, and the whisper.
    async fn materialize_card(&self, session_id: &SessionId) {
        let (card, chat_id, reply_msg_id, adapter) = {
            let Some(entry) = self.states.get(session_id) else {
                return;
            };
            let s = entry.value();
            if !s.status_msg_id.is_empty() || s.send_failed {
                return;
            }
            (
                render_running(s),
                s.chat_id.clone(),
                s.reply_msg_id.clone(),
                Arc::clone(&s.adapter),
            )
        };
        match adapter
            .send_card(&chat_id, &card, reply_msg_id.as_deref())
            .await
        {
            Ok(Some(msg_id)) => {
                if let Some(mut entry) = self.states.get_mut(session_id) {
                    let s = entry.value_mut();
                    s.status_msg_id = msg_id;
                    s.last_patch_at = Instant::now();
                }
            }
            // Platform without card support — silently skip.
            Ok(None) => {}
            Err(e) => {
                warn!(error = %e, "obs status card send failed");
                // Don't retry on every subsequent chunk — one attempt per run.
                if let Some(mut entry) = self.states.get_mut(session_id) {
                    entry.value_mut().send_failed = true;
                }
            }
        }
    }

    /// Mutate a running card's state and PATCH if outside the throttle window.
    /// Before the card is materialized this is memory-only.
    async fn update_running(&self, session_id: &SessionId, mutate: impl FnOnce(&mut ObsCardState)) {
        let patch = if let Some(mut entry) = self.states.get_mut(session_id) {
            let s = entry.value_mut();
            mutate(s);
            if s.status_msg_id.is_empty() || s.last_patch_at.elapsed() < self.patch_interval {
                None
            } else {
                s.last_patch_at = Instant::now();
                Some((
                    render_running(s),
                    s.status_msg_id.clone(),
                    Arc::clone(&s.adapter),
                ))
            }
        } else {
            None
        };
        if let Some((card, msg_id, adapter)) = patch {
            send_card_patch(&*adapter, &msg_id, &card).await;
        }
    }

    /// Settle the status card and clear the run's receipts. Returns the
    /// reply back when nothing was settled (no run state existed, or the
    /// settle send failed), so the caller can fall back to a plain send.
    ///
    /// With a reply, the card **morphs** into the final answer (no header;
    /// abnormal endings get a notice line in the body) — one message per
    /// run. Without a reply (crash/lost events), it freezes into the
    /// terminal header style. Cards never materialized (no-tool runs) send
    /// a new message only when there is something to show: a reply, or a
    /// failure notice (failures always get an explanation).
    async fn settle_card(
        &self,
        session_id: &SessionId,
        settle: &Settle,
        reply: Option<FinalReply>,
    ) -> Option<FinalReply> {
        // Receipts serve only the mid-run post detection, which the caller
        // evaluates before settling — always drop them at run end (covers
        // stopped/timeout/sweep, including truly-dead sessions whose real
        // `Stopped` never arrives).
        self.receipts.remove(session_id);
        let Some((_, state)) = self.states.remove(session_id) else {
            return reply;
        };
        let notice = settle.notice();
        let morphed = reply
            .as_ref()
            .and_then(|r| reply::render_card(r, notice.as_deref()));
        let (card, is_reply) = match morphed {
            Some(card) => (card, true),
            None => (render_terminal(&state, settle), false),
        };

        if state.status_msg_id.is_empty() {
            if is_reply || matches!(settle, Settle::Failed(_)) {
                if let Err(e) = state
                    .adapter
                    .send_card(&state.chat_id, &card, state.reply_msg_id.as_deref())
                    .await
                {
                    warn!(error = %e, "obs settle card send failed");
                    // The rich send failed (API error, card rejected) — the
                    // caller can still deliver the reply as a plain message.
                    return reply;
                }
            }
            return None;
        }
        if let Err(e) = state.adapter.update_card(&state.status_msg_id, &card).await {
            warn!(error = %e, "obs settle card patch failed");
        }
        None
    }
}

impl Default for ObsTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Rendering ───────────────────────────────────────────────────────

/// PATCH a card message in place; failures only warn (the next PATCH heals).
async fn send_card_patch(adapter: &dyn PlatformAdapter, message_id: &str, card_json: &str) {
    if let Err(e) = adapter.update_card(message_id, card_json).await {
        warn!(error = %e, "obs status card patch failed");
    }
}

fn render_running(s: &ObsCardState) -> String {
    let trace = s.trace.trace_preview_lines(STATUS_TRACE_MAX_ENTRIES);
    if trace.is_empty() && s.whisper.is_empty() && !s.seen_text {
        // Brand-new card: a light random placeholder, no stats line yet
        // (it would be a lone timer). Completed texts live in the trace and
        // the current one lives in the whisper, so this can't reappear
        // mid-run.
        return card_json(
            "blue",
            &phase_title(s),
            &format!(
                "<font color='grey'>{}</font>",
                random_title(IDLE_PLACEHOLDERS)
            ),
        );
    }
    let whisper_line = || {
        (!s.whisper.is_empty()).then(|| {
            format!(
                "<font color='grey'>💬 {}</font>",
                whisper_snippet(&s.whisper)
            )
        })
    };
    let elements = if trace.is_empty() {
        // No tools yet but text is flowing: stats + whisper in one block.
        let mut body = stats_line(s);
        if let Some(w) = whisper_line() {
            body.push('\n');
            body.push_str(&w);
        }
        vec![json!({ "tag": "markdown", "text_size": "notation", "content": body })]
    } else {
        // Stats line, a divider, then the live trace (+ whisper tail).
        let mut body = trace.join("\n");
        if let Some(w) = whisper_line() {
            body.push('\n');
            body.push_str(&w);
        }
        vec![
            json!({ "tag": "markdown", "text_size": "notation", "content": stats_line(s) }),
            json!({ "tag": "hr" }),
            json!({ "tag": "markdown", "text_size": "notation", "content": body }),
        ]
    };
    card_json_elements("blue", &phase_title(s), &elements)
}

/// Single-line whisper tail for the card body (≤ [`STATUS_TEXT_MAX_CHARS`]).
fn whisper_snippet(whisper: &str) -> String {
    let flat = reply::flatten_ws(whisper);
    if flat.chars().count() > STATUS_TEXT_MAX_CHARS {
        format!("…{}", tail_by_chars(&flat, STATUS_TEXT_MAX_CHARS - 1))
    } else {
        flat
    }
}

fn render_terminal(s: &ObsCardState, settle: &Settle) -> String {
    let elapsed = fmt_elapsed(s.started_at.elapsed());
    let (template, title) = match settle {
        Settle::Completed => (
            "green",
            format!("✅ Done · {} tools · {elapsed}", s.tool_count),
        ),
        Settle::Failed(_) => ("red", "❌ Failed".to_string()),
        Settle::Cancelled => ("grey", "⏹ Stopped".to_string()),
        Settle::MaxIterations(reached) => ("red", format!("❌ Max iterations ({reached})")),
        Settle::Timeout => ("grey", "⏰ Timed out".to_string()),
    };
    let mut lines = vec![stats_line(s)];
    if let Settle::Failed(error) = settle {
        lines.push(error_line(error));
    }
    card_json(template, &title, &lines.join("\n"))
}

fn card_json(template: &str, title: &str, body_md: &str) -> String {
    card_json_elements(
        template,
        title,
        &[json!({ "tag": "markdown", "text_size": "notation", "content": body_md })],
    )
}

fn card_json_elements(template: &str, title: &str, elements: &[serde_json::Value]) -> String {
    // Compact layout: 400px width, slim header/body padding, 12px notation text.
    json!({
        "schema": "2.0",
        "config": { "width_mode": "compact" },
        "header": {
            "title": { "tag": "plain_text", "content": title },
            "template": template,
            "padding": "4px 12px 4px 12px",
        },
        "body": {
            "padding": "8px 12px 8px 12px",
            "elements": elements,
        },
    })
    .to_string()
}

/// One-line run stats: elapsed · steps · tool total · tokens (greyed).
fn stats_line(s: &ObsCardState) -> String {
    use std::fmt::Write as _;
    let mut line = format!("⏱ {}", fmt_elapsed(s.started_at.elapsed()));
    let steps = s.trace.step_count();
    if steps > 0 {
        let _ = write!(line, " · {steps} steps");
    }
    if s.tool_count > 0 {
        let _ = write!(line, " · {} tools", s.tool_count);
    }
    if let Some(footer) = &s.token_footer {
        let _ = write!(line, " · <font color='grey'>{footer}</font>");
    }
    line
}

pub(crate) fn fmt_elapsed(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m{:02}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Format a token count as `12.3k` (thousands) for compact footers.
fn fmt_k(tokens: u32) -> String {
    if tokens < 1000 {
        tokens.to_string()
    } else {
        format!("{:.1}k", f64::from(tokens) / 1000.0)
    }
}

/// Humanize a `snake_case` tool name for the card title: `web_fetch` → `WebFetch`.
fn humanize_tool_name(name: &str) -> String {
    name.split('_')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            let mut chars = seg.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "obs_test.rs"]
mod tests;
