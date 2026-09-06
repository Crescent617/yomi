//! Run observability for external channels: status card + run receipts.
//!
//! A "run" is bracketed by `AgentEvent::Lifecycle(Running)` and
//! `AgentEvent::Lifecycle(Stopped)` (see `docs/archive/feishu-channel-observability.md`).
//! Run state is tracked from `Running`, and the status card is materialized
//! immediately at `Running` — the run is visible from the very start, so a
//! slow first request (long thinking, a 429 retry loop) never leaves the
//! user staring at nothing. Since the card morphs into the final reply on
//! settlement, every run is exactly one message (two when the user posted
//! mid-run: the card freezes as a terminal receipt and the reply lands at
//! the bottom).
//! While running, the card body shows the stats line, the live run trace
//! (most recent tool entries), and a tail of the assistant's current text
//! ("whisper"); a rotating placeholder fills the fresh card until the
//! first tool or text arrives, and idle phase titles rotate for fun. The
//! stats line shows real usage once reported (providers only emit it at
//! response end); a live output estimate (`out ~z`), accumulated across
//! the whole run, stands in mid-stream.
//! On settlement the card **morphs** into the final reply (last text +
//! run-trace panel, no header; abnormal endings get a notice line in the
//! content). With mid-run posts (and `mid_run_split` enabled) the reply
//! lands at the bottom as a new message carrying the run trace, and the
//! card freezes in place as a terminal receipt (header + stats); the card
//! keeps the trace panel itself whenever the reply didn't carry it (no
//! text, trace disabled, no reply, or the flush failed). Runs without a
//! reply (crash / lost events) freeze into a terminal header style
//! instead. User messages received during the run — addressed to the
//! bot or not — are recorded as receipts, used only for the mid-run
//! post detection (morph vs. new-message settle).
//!
//! Settle reaction: card patches never notify, so a run that settles
//! silently (the morph above) additionally reacts on the session's
//! **latest user message** — ✅ done / ❌ failed; the chat-list
//! "回应了你的消息" surfacing stands in for a completion ping. Runs
//! without a fresh trigger (cron-fired runs, API
//! steers) react on the last recorded user message instead of a run
//! trigger. No reaction when the reply lands as a new message (mid-run
//! posts) — that message notifies by itself. Repeated settles on the
//! same message delete the bot's previous reaction before re-adding:
//! platforms deduplicate identical reactions, so only delete-then-re-add
//! re-surfaces the signal (async runs on a silent session).
//!
//! A standalone compaction (`/compact` outside a run) has no run bracket,
//! so it gets its own minimal card: materialized on
//! `ModelEvent::Compacting { active: true }` when no run state exists,
//! settled into an outcome receipt by `ModelEvent::Compacted`. A mid-run
//! (auto) compact only flips the live card's phase instead.

use crate::event::{AgentEvent, AgentStatus, Event, ModelEvent, StopReason, ToolEvent};
use crate::types::SessionId;
use crate::utils::strs::{tail_by_chars, truncate_by_chars};
use dashmap::DashMap;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

use crate::channels::reply::{self, FinalReply};
use crate::channels::PlatformAdapter;

/// Minimum interval between two in-place card updates (low-frequency updates).
const PATCH_MIN_INTERVAL: Duration = Duration::from_secs(3);

/// Consecutive heartbeat-patch failures before the breaker trips
/// (`send_failed`): a PATCH that keeps failing (card deleted remotely, API
/// down) must not be stormed every heartbeat.
const PATCH_FAILURE_LIMIT: u32 = 3;
/// Trace entries shown live on the status card (most recent kept).
const STATUS_TRACE_MAX_ENTRIES: usize = 10;
/// Error summary truncation on terminal cards.
const ERROR_MAX_CHARS: usize = 200;
/// Whisper buffer cap (tail kept); bounds memory for long streamed answers.
const WHISPER_BUFFER_CHARS: usize = 200;
/// Max length (chars, ellipsis included) for dynamic text lines on the
/// status card body (whisper).
const STATUS_TEXT_MAX_CHARS: usize = 100;
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
    /// Retry/error/compact/fallback — informative text as-is.
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
    /// The full run trace shown live on the status card.
    trace: reply::RunReplyBuffer,
    /// The session's model key, shown in the stats line; set by the hub
    /// forwarder at the run's first `Running` (absent when unknown).
    model: Option<String>,
    /// Live tail of the assistant's in-progress text output ("whisper") —
    /// the one piece of not-yet-finished content on the card. Completed
    /// texts move into `trace` as narrations at `ModelEvent::End`.
    whisper: String,
    /// Any text chunk seen this run (never cleared): gates the fresh-card
    /// placeholder so it can't reappear after a Request boundary cleared
    /// the whisper.
    seen_text: bool,
    /// Live output estimate: bytes of the in-flight response
    /// (text/thinking ≈4/token, tool args ≈2/token), reset per request —
    /// a retried attempt's bytes are discarded, never double-counted.
    out_text_bytes: usize,
    out_json_bytes: usize,
    /// Estimated output tokens of completed responses this run, folded in
    /// at `ModelEvent::End`; the run total grows monotonically.
    out_run_tokens: u32,
    token_footer: Option<String>,
    last_patch_at: Instant,
    /// Set when the materialize send failed — no more attempts this run
    /// (card APIs are best-effort; don't storm a struggling API endpoint).
    send_failed: bool,
    /// Consecutive heartbeat-patch failures; reaching
    /// [`PATCH_FAILURE_LIMIT`] trips `send_failed` for the rest of the run.
    patch_failures: u32,
    /// Serializes every PATCH targeting this card (throttled event updates,
    /// heartbeat refreshes, and the terminal settle/freeze). Senders
    /// re-validate inside the lock and render fresh inside the lock, so a
    /// heartbeat PATCH can never land *after* the settle morph and paint a
    /// stale "running" render over the reply card — the settle always
    /// wins, and a late heartbeat sees the state gone and skips (M2).
    /// 终态路径统一经 [`ObsTracker::take_state_locked`] 锁内摘除。
    patch_lock: Arc<tokio::sync::Mutex<()>>,
    /// Standalone-compact card (`/compact` outside a run): materialized on
    /// `Compacting { active: true }` when no run state exists, settled by
    /// `Compacted` instead of the run's `Stopped`.
    compact_only: bool,
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
            trace: reply::RunReplyBuffer::new(),
            model: None,
            whisper: String::new(),
            seen_text: false,
            out_text_bytes: 0,
            out_json_bytes: 0,
            out_run_tokens: 0,
            token_footer: None,
            last_patch_at: now,
            send_failed: false,
            patch_failures: 0,
            patch_lock: Arc::new(tokio::sync::Mutex::new(())),
            compact_only: false,
        }
    }

    /// Append a streamed text delta to the whisper (tail kept).
    fn push_whisper(&mut self, delta: &str) {
        self.whisper.push_str(delta);
        if self.whisper.chars().count() > WHISPER_BUFFER_CHARS {
            self.whisper = tail_by_chars(&self.whisper, WHISPER_BUFFER_CHARS);
        }
    }

    /// The session's model key: shown on the live stats line and mirrored
    /// into the trace buffer so the terminal trace title carries it too.
    fn assign_model(&mut self, model: String) {
        self.trace.set_model(model.clone());
        self.model = Some(model);
    }

    /// Estimated output tokens of the in-flight response.
    fn current_out_estimate(&self) -> u32 {
        (self.out_text_bytes.div_ceil(4) + self.out_json_bytes.div_ceil(2)) as u32
    }

    /// Run-total output estimate: completed responses + in-flight.
    fn out_estimate(&self) -> u32 {
        self.out_run_tokens + self.current_out_estimate()
    }

    /// Reset the in-flight estimate (request boundary).
    fn reset_out_estimate(&mut self) {
        self.out_text_bytes = 0;
        self.out_json_bytes = 0;
    }

    /// Fold the finished response's estimate into the run total.
    fn fold_out_estimate(&mut self) {
        self.out_run_tokens += self.current_out_estimate();
        self.reset_out_estimate();
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

/// What a card settlement produced. Exactly one of the two fields is
/// meaningful: an `unsettled` reply (nothing was settled — the caller
/// falls back to a plain send) or the settled card's message id.
pub(crate) struct SettleOutcome {
    /// The reply handed back when nothing was settled (no run state, or
    /// the settle send failed).
    pub(crate) unsettled: Option<FinalReply>,
    /// Platform message id of the settled card (the reply's own id, for
    /// jump links); `None` when settlement sent nothing.
    pub(crate) message_id: Option<String>,
}

impl SettleOutcome {
    fn unsettled(reply: Option<FinalReply>) -> Self {
        Self {
            unsettled: reply,
            message_id: None,
        }
    }

    fn settled(message_id: Option<String>) -> Self {
        Self {
            unsettled: None,
            message_id,
        }
    }
}

/// Map a `Stopped` reason to its settlement kind.
fn settle_from_reason(reason: &StopReason) -> Settle {
    match reason {
        StopReason::Completed { .. } => Settle::Completed,
        StopReason::Cancelled { .. } => Settle::Cancelled,
        StopReason::Failed { error } => Settle::Failed(error.clone()),
        StopReason::MaxIterations { reached } => Settle::MaxIterations(*reached),
    }
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

    /// Settle-reaction emoji (Feishu `emoji_type`): the completion signal
    /// for a silently-settled card. `None` for cancelled runs — the user
    /// stopped it themselves, they know.
    fn reaction_emoji(&self) -> Option<&'static str> {
        match self {
            Settle::Completed => Some("DONE"),
            Settle::Failed(_) | Settle::MaxIterations(_) | Settle::Timeout => Some("CrossMark"),
            Settle::Cancelled => None,
        }
    }
}

/// Truncated one-line error summary for card bodies — the single truncation
/// budget shared by the morphed-card notice and the terminal card.
fn error_line(error: &str) -> String {
    // 动态错误文本全角化（md_safe）：错误内容常带命令/反引号，截断也可能
    // 切断成对标记——未闭合会撑破整张卡的 markdown。
    format!(
        "**Error**  {}",
        reply::md_safe(&truncate_by_chars(error, ERROR_MAX_CHARS, "…"))
    )
}

/// Settle-reaction target: the session's latest user message plus the
/// reaction the bot added there at the previous settle. The previous
/// reaction is deleted before re-adding — platforms deduplicate an
/// identical reaction from the same operator (no new event, no
/// re-notification), so repeated settles on the same message (async runs
/// on a silent session) only re-surface via delete-then-re-add.
#[derive(Debug, Clone)]
struct ReactionTarget {
    msg_id: String,
    reaction_id: Option<String>,
}

/// Tracks per-session observability state and drives the platform adapter.
pub(crate) struct ObsTracker {
    states: DashMap<SessionId, ObsCardState>,
    /// Model names stashed before their run state exists (the forwarder
    /// learns the model at the same `Running` that later materializes the
    /// state); consumed at materialization.
    pending_models: DashMap<SessionId, String>,
    /// IDs of user messages received during each run (drives the mid-run
    /// post detection); cleared at settlement.
    receipts: DashMap<SessionId, Vec<String>>,
    /// The session's settle-reaction target (its latest user message).
    /// Sticky across runs (settlement does NOT clear it), so async runs
    /// without a fresh trigger (cron-fired runs, API
    /// steers) still have a message to react on.
    last_user_msg: DashMap<SessionId, ReactionTarget>,
    patch_interval: Duration,
}

impl ObsTracker {
    pub(crate) fn new() -> Self {
        Self {
            states: DashMap::new(),
            pending_models: DashMap::new(),
            receipts: DashMap::new(),
            last_user_msg: DashMap::new(),
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

    /// Whether a live run state exists for the session — the forwarder
    /// uses it to do one-shot setup (the model lookup) only at a run's
    /// first `Running`.
    pub(crate) fn has_state(&self, session_id: &SessionId) -> bool {
        self.states.contains_key(session_id)
    }

    /// Set the model name shown in the status card's stats line. With a
    /// live state the field updates in place (the next PATCH picks it
    /// up); before materialization the name is stashed and consumed when
    /// the first `Running` materializes the state.
    pub(crate) fn set_model(&self, session_id: &SessionId, model: String) {
        if let Some(mut entry) = self.states.get_mut(session_id) {
            entry.value_mut().assign_model(model);
        } else {
            self.pending_models.insert(session_id.clone(), model);
        }
    }

    /// Record the session's latest user message as the settle-reaction
    /// target. Called for every accepted user message (trigger / steer /
    /// queue), so runs without a fresh trigger still have somewhere to
    /// land the reaction.
    pub(crate) fn record_user_msg(&self, session_id: &SessionId, message_id: String) {
        self.last_user_msg.insert(
            session_id.clone(),
            ReactionTarget {
                msg_id: message_id,
                reaction_id: None,
            },
        );
    }

    /// The session's latest user message id — the settle reaction's
    /// target; the subscription notify quotes the same message so the
    /// card's context line and the ✅ always point at one message.
    pub(crate) fn last_user_msg_id(&self, session_id: &SessionId) -> Option<String> {
        self.last_user_msg.get(session_id).map(|t| t.msg_id.clone())
    }

    /// Whether the user posted messages mid-run — receipts hold only
    /// messages recorded while the agent was running (run triggers are never
    /// recorded, and commands never record receipts), so any receipt means
    /// the reply should land below their messages as a new message instead
    /// of morphing the status card.
    pub(crate) fn has_mid_run_posts(&self, session_id: &SessionId) -> bool {
        self.receipts.get(session_id).is_some_and(|r| !r.is_empty())
    }

    /// Drop the run's recorded receipts without settling. Receipts only
    /// drive the mid-run split decision — when the split is disabled they
    /// must not suppress the settle reaction of the in-place morph (see
    /// `deliver_reply`); settlement clears them anyway.
    pub(crate) fn clear_receipts(&self, session_id: &SessionId) {
        self.receipts.remove(session_id);
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
                // Running fires per turn; only the first one starts tracking
                // and materializes the card — the run is visible from the
                // very start (a rotating placeholder fills the card until
                // the first tool or text arrives).
                if self.states.contains_key(session_id) {
                    return;
                }
                let mut state = ObsCardState::new(Arc::clone(adapter), chat_id, reply_msg_id);
                if let Some((_, model)) = self.pending_models.remove(session_id) {
                    state.assign_model(model);
                }
                self.states.insert(session_id.clone(), state);
                self.materialize_card(session_id).await;
            }
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Stopped { reason },
            }) => {
                // Degenerate path: settle without a reply (crash/lost
                // events). The hub forwarder calls `handle_stopped` with
                // the buffered reply instead.
                let _ = self.handle_stopped(session_id, reason, None).await;
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
                    s.phase = Phase::Tool(humanize_tool_name(&tool_name));
                    s.trace
                        .record_tool_start(&tool_id, &tool_name, arguments.as_deref());
                })
                .await;
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
                wait_ms,
            }) => {
                let retry_of = format!("🔁 Retrying {attempt}/{max_attempts}");
                let reason = truncate_by_chars(reason, 30, "…");
                let phase = Phase::Text(match crate::event::format_retry_delay(*wait_ms) {
                    Some(delay) => format!("{retry_of} {delay}: {reason}"),
                    None => format!("{retry_of}: {reason}"),
                });
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Agent(AgentEvent::Error { error, .. }) => {
                let phase = Phase::Text(format!("⚠️ Error: {}", truncate_by_chars(error, 30, "…")));
                // Never settles — a mid-retry error may still recover (see design §3).
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Model(ModelEvent::Compacting { active }) => {
                let active = *active;
                if active && !self.states.contains_key(session_id) {
                    // Standalone compaction (manual `/compact` outside a
                    // run): no run brackets it, so it gets its own status
                    // card — materialized immediately, settled by
                    // `Compacted`.
                    let mut state = ObsCardState::new(Arc::clone(adapter), chat_id, reply_msg_id);
                    state.phase = Phase::Text("📦 Compacting context…".to_string());
                    state.compact_only = true;
                    self.states.insert(session_id.clone(), state);
                    self.materialize_card(session_id).await;
                    return;
                }
                self.update_running(session_id, |s| {
                    // A standalone compact's card keeps its phase until
                    // `Compacted` settles it; a mid-run (auto) compact
                    // only flips the live card's phase.
                    if s.compact_only {
                        return;
                    }
                    s.phase = if active {
                        Phase::Text("📦 Compacting context…".to_string())
                    } else {
                        Phase::Thinking
                    };
                })
                .await;
            }
            Event::Model(ModelEvent::Compacted { summary, is_error }) => {
                // Only a standalone compact's own card settles here; a
                // mid-run (auto) compact rides the live run's card, which
                // the run's `Stopped` settles.
                let compact_only = self.states.get(session_id).is_some_and(|s| s.compact_only);
                if compact_only {
                    self.settle_compact(session_id, summary, *is_error).await;
                }
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
                    s.reset_out_estimate();
                })
                .await;
            }
            Event::Model(ModelEvent::Chunk { content, .. }) => {
                let (phase, delta, text_bytes) = match content {
                    crate::event::ContentChunk::Text(text) => {
                        (Phase::Typing, Some(text.clone()), text.len())
                    }
                    crate::event::ContentChunk::Thinking { thinking, .. } => {
                        (Phase::Thinking, None, thinking.len())
                    }
                    crate::event::ContentChunk::RedactedThinking => (Phase::Thinking, None, 0),
                };
                self.update_running(session_id, |s| {
                    s.phase = phase;
                    s.out_text_bytes += text_bytes;
                    if let Some(delta) = delta {
                        s.seen_text = true;
                        s.push_whisper(&delta);
                    }
                })
                .await;
            }
            Event::Model(ModelEvent::ToolCallDelta {
                arguments_delta, ..
            }) => {
                let bytes = arguments_delta.len();
                self.update_running(session_id, |s| s.out_json_bytes += bytes)
                    .await;
            }
            Event::Model(ModelEvent::End { content, .. }) => {
                // One completed model response = one step, text or not
                // (tool-call-only turns count too). A non-empty text also
                // joins the trace as a narration entry (self-heals chunk
                // loss on the bus — the full text is authoritative); the
                // whisper always clears for the next turn instead of
                // duplicating or going stale.
                let text = crate::channels::blocks_to_text(content);
                self.update_running(session_id, |s| {
                    s.trace.record_model_end(&text);
                    s.whisper.clear();
                    s.fold_out_estimate();
                })
                .await;
            }
            Event::Model(ModelEvent::TokenUsage {
                message_id,
                prompt_tokens,
                completion_tokens,
                total_tokens,
                context_window,
                ..
            }) => {
                let (msg_id, prompt, completion, total, window) = (
                    message_id.clone(),
                    *prompt_tokens,
                    *completion_tokens,
                    *total_tokens,
                    *context_window,
                );
                self.update_running(session_id, |s| {
                    s.token_footer = Some(reply::ctx_footer(total, window));
                    s.trace.set_ctx_footer(total, window);
                    s.trace.add_usage(&msg_id, prompt, completion);
                    // Real usage now covers this response's output: zero
                    // the whole estimate (run-folded + in-flight) so the
                    // following End fold can't re-add the same response's
                    // estimate on top of the true count.
                    s.out_run_tokens = 0;
                    s.reset_out_estimate();
                })
                .await;
            }
            // Other events carry no card-visible state.
            _ => {}
        }
    }

    /// Settle a run on `Lifecycle(Stopped)`: morph the status card into the
    /// final reply (one message per run). The outcome hands the reply back
    /// when nothing was settled (no run state, or the settle send failed),
    /// so the caller can fall back to a plain send; on success it carries
    /// the settled card's message id (the reply's own id, for jump links).
    pub(crate) async fn handle_stopped(
        &self,
        session_id: &SessionId,
        reason: &StopReason,
        reply: Option<FinalReply>,
    ) -> SettleOutcome {
        self.settle_card(session_id, &settle_from_reason(reason), reply)
            .await
    }

    /// Watchdog settlement for a session whose agent died (crash / lost
    /// `Stopped`): settle the card with whatever reply state remains.
    pub(crate) async fn handle_timeout(
        &self,
        session_id: &SessionId,
        reply: Option<FinalReply>,
    ) -> SettleOutcome {
        self.settle_card(session_id, &Settle::Timeout, reply).await
    }

    /// Mid-run split settlement (`Stopped`): freeze the status card in
    /// place as a terminal receipt — the reply lands as a NEW message
    /// below the user's mid-run posts, so the card must not morph into
    /// it. `keep_trace` = the card carries the run trace panel itself;
    /// false when the reply message carries it instead. Never sends the
    /// settle reaction: the reply message notifies by itself.
    pub(crate) async fn freeze_stopped(
        &self,
        session_id: &SessionId,
        reason: &StopReason,
        keep_trace: bool,
    ) {
        self.freeze_card(session_id, &settle_from_reason(reason), keep_trace)
            .await;
    }

    /// Mid-run split settlement for the watchdog path (see
    /// [`ObsTracker::freeze_stopped`]).
    pub(crate) async fn freeze_timeout(&self, session_id: &SessionId, keep_trace: bool) {
        self.freeze_card(session_id, &Settle::Timeout, keep_trace)
            .await;
    }

    /// Settle cards whose session no longer has a live, non-idle agent
    /// (called periodically from the event forwarder; covers agent crash /
    /// lost events where no `Stopped` ever arrives). Liveness is queried,
    /// not inferred from event gaps, so long tool calls never false-positive.
    ///
    /// The forwarder's tick arms yield to queued events (entry guard plus
    /// an in-handler re-check), so terminal events (`Stopped`/`Compacted`)
    /// already delivered to this listener are always drained before this
    /// sweep runs; a card is swept here only when its terminal event never
    /// arrived (crash / lost), the sub-ms bus-forwarder hop aside.
    pub(crate) async fn sweep_dead_sessions(&self, is_alive: impl Fn(&SessionId) -> bool) {
        let dead: Vec<SessionId> = self
            .states
            .iter()
            .filter(|e| !is_alive(e.key()))
            .map(|e| e.key().clone())
            .collect();
        for sid in dead {
            // 收集到结算之间会话可能已复活（新 run 复用了孤儿卡）——
            // 逐卡复核判活，别把活卡冻成 ⏰（S1：收集与逐卡结算的
            // TOCTOU 收口；settle 在飞与新 run materialize 的残余窗口
            // 为 actor 化前既有语义，由 is_quiet 谓词收窄）。
            if is_alive(&sid) {
                continue;
            }
            let _ = self.settle_card(&sid, &Settle::Timeout, None).await;
        }
    }

    // ── internals ─────────────────────────────────────────────────────

    /// Send the status card if not yet materialized (triggered by the first
    /// `Lifecycle(Running)` of a run). The fresh card renders from the live
    /// state, so it already carries the phase, token usage, and the whisper.
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
                render_running(s, session_id),
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
        // 所有落在本卡上的 PATCH 都经 patch_lock 串行（M2）：与心跳、
        // 与 settle 的终态 PATCH 互斥——锁内渲染锁内发，本路径永远是
        // 最新内容，也绝不会盖过已结算的终态卡（settle 摘除状态后本
        // 路径在锁内看到 `get_mut` 为空即跳过重渲染）。
        let Some(lock) = self
            .states
            .get(session_id)
            .map(|e| Arc::clone(&e.patch_lock))
        else {
            return;
        };
        let _guard = lock.lock().await;
        let patch = if let Some(mut entry) = self.states.get_mut(session_id) {
            let s = entry.value_mut();
            mutate(s);
            if s.status_msg_id.is_empty() || s.last_patch_at.elapsed() < self.patch_interval {
                None
            } else {
                s.last_patch_at = Instant::now();
                Some((
                    render_running(s, session_id),
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

    /// Heartbeat refresh for live cards: re-render and PATCH any running
    /// card whose last update is older than `max_age`. Event-driven updates
    /// stop during long tool calls (no events between tool start and end),
    /// freezing the card — elapsed time and idle titles stuck at the last
    /// patch. Cheap state scan per call; only stale cards get patched.
    pub(crate) async fn refresh_stale(&self, max_age: Duration) {
        // Collect first: never hold a DashMap shard lock across an await.
        let mut candidates = Vec::new();
        for entry in &self.states {
            let s = entry.value();
            if s.status_msg_id.is_empty() || s.send_failed {
                continue;
            }
            if s.last_patch_at.elapsed() < max_age {
                continue;
            }
            candidates.push((
                entry.key().clone(),
                s.status_msg_id.clone(),
                s.last_patch_at,
                Arc::clone(&s.patch_lock),
            ));
        }
        for (sid, msg_id, marked_at, lock) in candidates {
            // 心跳在独立任务里与每会话 actor 并发——候选到发送之间会话
            // 可能已结算（卡已 morph 成回复）或刚被事件路径 PATCH 过；
            // 把过期"运行中"渲染盖到已结算的回复卡上不可自愈（obs 状态
            // 已删，再无人重 PATCH），回复会永久不可见。因此本卡的 PATCH
            // 全部经 patch_lock 串行（M2）：锁内重校验（状态消失、卡片
            // 换代 status_msg_id 变、期间有更新 last_patch_at 变一律跳
            // 过）+ 锁内新鲜渲染 + 锁内发送——settle 若先到，重校验即
            // 拦截；settle 若后到，它等锁后覆盖本渲染，终态必赢。
            let _guard = lock.lock().await;
            let prepared = if let Some(mut entry) = self.states.get_mut(&sid) {
                let s = entry.value_mut();
                if s.status_msg_id != msg_id || s.last_patch_at != marked_at || s.send_failed {
                    None
                } else {
                    s.last_patch_at = Instant::now();
                    Some((render_running(s, &sid), Arc::clone(&s.adapter)))
                }
            } else {
                None
            };
            let Some((card, adapter)) = prepared else {
                continue;
            };
            let ok = send_card_patch(&*adapter, &msg_id, &card).await;
            let Some(mut entry) = self.states.get_mut(&sid) else {
                // 防御性分支：当前锁序下不可达（摘除 state 必先拿本
                // 锁），保留以抵御未来锁序变更（三审 nit）。
                continue;
            };
            let s = entry.value_mut();
            if ok {
                s.patch_failures = 0;
            } else {
                s.patch_failures += 1;
                if s.patch_failures >= PATCH_FAILURE_LIMIT {
                    // Trip the breaker for the rest of the run (mirrors
                    // materialize's one-shot send_failed): the event-driven
                    // path still patches on real events, rate-limited by
                    // the throttle, and acts as the recovery probe.
                    s.send_failed = true;
                    tracing::info!(
                        "obs heartbeat disabled after {PATCH_FAILURE_LIMIT} patch failures"
                    );
                }
            }
        }
    }

    /// 三条终态路径（settle/freeze/compact）共用的**锁内摘除**
    /// prologue（M2 不变式的唯一载体）：持 `patch_lock` 期间摘除
    /// state——在飞的心跳/节流 PATCH 随后拿锁时重校验见 state 已删
    /// 即跳过，本路径发出的终态渲染永远是最后落在卡上的内容（旧
    /// 顺序：校验→发送→结算，在飞 PATCH 可晚于 morph 落地，过期
    /// "运行中"盖回复卡且无自愈）。返回的 guard 必须活到终态
    /// PATCH 发出之后。
    async fn take_state_locked(
        &self,
        session_id: &SessionId,
    ) -> Option<(ObsCardState, tokio::sync::OwnedMutexGuard<()>)> {
        let lock = self
            .states
            .get(session_id)
            .map(|e| Arc::clone(&e.patch_lock))?;
        let guard = lock.lock_owned().await;
        let state = self.states.remove(session_id).map(|(_, s)| s)?;
        Some((state, guard))
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
    /// failure notice (failures always get an explanation). A successful
    /// in-place settle (morph or freeze) is silent on the platform, so it
    /// additionally reacts on the session's latest user message — skipped
    /// when the reply lands as a new message (mid-run posts).
    async fn settle_card(
        &self,
        session_id: &SessionId,
        settle: &Settle,
        reply: Option<FinalReply>,
    ) -> SettleOutcome {
        // Evaluate the mid-run detection BEFORE clearing receipts: when
        // the reply lands as a new message it notifies by itself, making
        // the settle reaction redundant. Receipts are always dropped at
        // run end (covers stopped/timeout/sweep, including truly-dead
        // sessions whose real `Stopped` never arrives).
        let mid_run_posts = self.has_mid_run_posts(session_id);
        self.receipts.remove(session_id);
        let Some((state, _guard)) = self.take_state_locked(session_id).await else {
            return SettleOutcome::unsettled(reply);
        };
        let notice = settle.notice();
        let morphed = reply
            .as_ref()
            .and_then(|r| reply::render_card(r, notice.as_deref()));
        let (card, is_reply) = match morphed {
            Some(card) => (card, true),
            // No reply to morph into (crash / lost events): freeze into
            // the terminal header style, keeping the trace panel — with
            // no reply message to carry it, this is the only place the
            // trace survives settlement.
            None => (render_terminal(&state, settle, true), false),
        };

        if state.status_msg_id.is_empty() {
            if is_reply || matches!(settle, Settle::Failed(_)) {
                return match state
                    .adapter
                    .send_card(&state.chat_id, &card, state.reply_msg_id.as_deref())
                    .await
                {
                    Ok(msg_id) => SettleOutcome::settled(msg_id),
                    // The rich send failed (API error, card rejected) — the
                    // caller can still deliver the reply as a plain message.
                    Err(e) => {
                        warn!(error = %e, "obs settle card send failed");
                        SettleOutcome::unsettled(reply)
                    }
                };
            }
            return SettleOutcome::settled(None);
        }
        let outcome = match state.adapter.update_card(&state.status_msg_id, &card).await {
            Err(e) => {
                warn!(error = %e, "obs settle card patch failed");
                // 返回回复供调用方纯文本兜底（对齐文档承诺与 send_card
                // 失败分支——此处曾 settled(None) 吞回复：live 卡永停
                // "运行中"且回复整篇丢失，终审 #5 实锤的既有洞）。
                SettleOutcome::unsettled(reply)
            }
            Ok(()) => SettleOutcome::settled(Some(state.status_msg_id.clone())),
        };
        // 终态 PATCH 已落地即释锁（复审 nit）：reaction 作用于用户消
        // 息、与卡 PATCH 无序依赖，不再占锁多等两次平台 RTT。
        drop(_guard);
        // A silent in-place settle carries no notification — react
        // on the latest user message as the completion signal
        // instead (skipped when the reply lands as a new message).
        if outcome.message_id.is_some() && !mid_run_posts {
            self.send_settle_reaction(session_id, &state, settle).await;
        }
        outcome
    }

    /// Freeze the status card in place as a terminal receipt and clear
    /// the run's receipts (mid-run split: the reply lands as a separate
    /// message below the user's mid-run posts, so the card must not
    /// morph into it). `keep_trace` = the card carries the run trace
    /// panel itself — false when the reply message carries it instead.
    /// Never sends the settle reaction: the reply message notifies by
    /// itself. A failure whose card never materialized still sends the
    /// terminal card as a new message (failures always get an
    /// explanation), trace panel included — the flushed reply may carry
    /// it too; duplication in this rare edge is preferred over loss.
    async fn freeze_card(&self, session_id: &SessionId, settle: &Settle, keep_trace: bool) {
        self.receipts.remove(session_id);
        let Some((state, _guard)) = self.take_state_locked(session_id).await else {
            return;
        };
        if state.status_msg_id.is_empty() {
            if matches!(settle, Settle::Failed(_)) {
                let card = render_terminal(&state, settle, true);
                if let Err(e) = state
                    .adapter
                    .send_card(&state.chat_id, &card, state.reply_msg_id.as_deref())
                    .await
                {
                    warn!(error = %e, "obs freeze card send failed");
                }
            }
            return;
        }
        let card = render_terminal(&state, settle, keep_trace);
        if let Err(e) = state.adapter.update_card(&state.status_msg_id, &card).await {
            warn!(error = %e, "obs freeze card patch failed");
        }
    }

    /// Settle a standalone compact's status card (`/compact` outside a
    /// run): morph it into the outcome receipt. No settle reaction — the
    /// card's own send already notified, and the command message is never
    /// recorded as a reaction target. Run receipts are left alone: a
    /// message posted mid-compact is the next run's trigger, and that
    /// run's settlement still needs it for the morph/split decision.
    /// (Divergence, accepted: the cancel path settles through
    /// `settle_card`, which does clear receipts — a mid-compact post
    /// followed by `/stop` then morphs instead of splitting, visually
    /// identical since the new card anchors below the trigger anyway.)
    async fn settle_compact(&self, session_id: &SessionId, summary: &str, is_error: bool) {
        let Some((state, _guard)) = self.take_state_locked(session_id).await else {
            return;
        };
        let card = render_compact_terminal(&state, summary, is_error);
        if state.status_msg_id.is_empty() {
            // Never materialized (send failed): failures still get an
            // explanation as a new message; a silent success stays silent.
            if is_error {
                if let Err(e) = state
                    .adapter
                    .send_card(&state.chat_id, &card, state.reply_msg_id.as_deref())
                    .await
                {
                    warn!(error = %e, "obs compact settle card send failed");
                }
            }
            return;
        }
        if let Err(e) = state.adapter.update_card(&state.status_msg_id, &card).await {
            warn!(error = %e, "obs compact settle card patch failed");
        }
    }

    /// React on the session's latest user message as the completion
    /// signal for a silently-settled card. A reaction the bot added on a
    /// previous settle is deleted first: platforms deduplicate identical
    /// reactions, so delete-then-re-add is what makes repeated settles on
    /// the same message (async runs on a silent session) re-surface.
    /// Best-effort: no recorded target (fresh session, hub restart) or a
    /// platform failure just skips the reaction.
    async fn send_settle_reaction(
        &self,
        session_id: &SessionId,
        state: &ObsCardState,
        settle: &Settle,
    ) {
        let Some(emoji) = settle.reaction_emoji() else {
            return;
        };
        // Clone out of the map instead of holding a shard guard across await.
        let target = self.last_user_msg.get(session_id).map(|t| t.clone());
        tracing::trace!(
            session_id = %session_id.0,
            emoji,
            target = ?target.as_ref().map(|t| &t.msg_id),
            "settle reaction"
        );
        let Some(target) = target else {
            return;
        };
        if let Some(reaction_id) = &target.reaction_id {
            if let Err(e) = state
                .adapter
                .delete_reaction(&state.chat_id, &target.msg_id, reaction_id)
                .await
            {
                // A stale/gone reaction must not block the fresh add.
                warn!(error = %e, "obs settle reaction delete failed");
            }
        }
        match state
            .adapter
            .send_reaction(&state.chat_id, &target.msg_id, emoji)
            .await
        {
            Ok(reaction_id) => {
                // Remember the reaction for the next settle's re-add —
                // unless a newer user message already moved the target.
                if let Some(mut entry) = self.last_user_msg.get_mut(session_id) {
                    if entry.msg_id == target.msg_id {
                        entry.reaction_id = reaction_id;
                    }
                }
            }
            Err(e) => warn!(error = %e, "obs settle reaction failed"),
        }
    }
}

impl Default for ObsTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Rendering ───────────────────────────────────────────────────────

/// PATCH a card message in place; failures only warn (the next PATCH heals).
async fn send_card_patch(adapter: &dyn PlatformAdapter, message_id: &str, card_json: &str) -> bool {
    if let Err(e) = adapter.update_card(message_id, card_json).await {
        warn!(error = %e, "obs status card patch failed");
        return false;
    }
    true
}

fn render_running(s: &ObsCardState, sid: &SessionId) -> String {
    let trace = s.trace.trace_preview_lines(STATUS_TRACE_MAX_ENTRIES);
    if trace.is_empty() && s.whisper.is_empty() && !s.seen_text {
        // Brand-new card: a light random placeholder. Bare while the
        // timer would be its only content; once any token data exists
        // (live estimate or real usage) the stats line rides along.
        let placeholder = format!(
            "<font color='grey'>{}</font>",
            random_title(IDLE_PLACEHOLDERS)
        );
        let body = if s.token_footer.is_some() || s.out_estimate() > 0 {
            format!("{}\n{placeholder}", stats_line(s))
        } else {
            placeholder
        };
        return card_json_elements(
            "blue",
            &phase_title(s),
            &[
                json!({ "tag": "markdown", "text_size": "notation", "content": body }),
                stop_button_row(sid),
            ],
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
    // Everything live rides inside one collapsible panel that starts
    // expanded so the human watches it stream — **strictly chronological**:
    // the trace first, the live whisper tail last（与终态卡同一时序：
    // 最新内容在底部；早前 whisper 置顶会让最新文本跳到历史之上，
    // 且 End 落地时文本还要从顶部跳回时序位置——hrli 2026-08-22 定
    // 稿不特殊处理）。Reading bots skip the whole thing (yomi strips
    // collapsible panels from card text on every read path). The stats
    // line rides the panel's title instead of a top element.
    let mut panel_lines: Vec<String> = trace;
    if let Some(w) = whisper_line() {
        panel_lines.push(w);
    }
    let mut elements = if panel_lines.is_empty() {
        // Retry edge: a new Request cleared the whisper before the first
        // tool or model End — bare stats until content flows again.
        vec![json!({ "tag": "markdown", "text_size": "notation", "content": stats_line(s) })]
    } else {
        vec![reply::trace_panel_element_expanded(
            &panel_lines,
            &reply::render_trace_title(&reply::TraceTitle {
                steps: s.trace.step_count(),
                failed: s.trace.failed_count(),
                elapsed: s.started_at.elapsed(),
                model: s.model.as_deref(),
                ctx_footer: s.token_footer.as_deref(),
                usage_in: s.trace.usage().0,
                usage_out: s.trace.usage().1,
                out_estimate: s.out_estimate(),
            }),
        )]
    };
    elements.push(stop_button_row(sid));
    card_json_elements("blue", &phase_title(s), &elements)
}

/// Bottom-right `Stop` button row on the live card: a small bordered
/// button, visually quiet but one tap away — cancel is the one
/// time-sensitive action while a run burns tokens (everything else can
/// wait for idle and keeps its command entry). Terminal cards don't
/// carry it. `type: "default"` + `size: "small"` is the quietest
/// bordered variant; text-type buttons render in plain text color on
/// Android dark mode (no link-blue), which read as unclickable.
fn stop_button_row(sid: &SessionId) -> serde_json::Value {
    json!({
        "tag": "column_set",
        "columns": [
            {
                "tag": "column", "width": "weighted", "weight": 1,
                "elements": [{ "tag": "markdown", "text_size": "notation", "content": "" }],
            },
            {
                "tag": "column", "width": "auto",
                "elements": [{
                    "tag": "button",
                    "size": "small",
                    "text": { "tag": "plain_text", "content": "Stop" },
                    "type": "default",
                    "behaviors": [{ "type": "callback", "value": { "action": "act_stop", "sid": sid.0 } }],
                }],
            },
        ],
    })
}

/// `act_stop` button callback (bottom of the live status card): cancel
/// the session's current run. The user-level gate (blocked /
/// `allowed_users`) runs at the hub's card-action router, shared by
/// every button; this handler only executes. No card patch here: the
/// run's settlement morphs the card into the terminal receipt, which is
/// the click feedback.
pub(crate) fn handle_stop_action(
    kernel: &crate::kernel::Kernel,
    action: &crate::channels::CardAction,
) {
    if action.value["action"].as_str() != Some("act_stop") {
        warn!(value = %action.value, "unrecognized obs card action");
        return;
    }
    let sid = SessionId::from(action.value["sid"].as_str().unwrap_or_default().to_string());
    if sid.0.is_empty() {
        warn!(value = %action.value, "stop card action missing sid");
        return;
    }
    kernel.cancel(&sid);
}

/// Single-line whisper tail for the card body (≤ [`STATUS_TEXT_MAX_CHARS`]).
fn whisper_snippet(whisper: &str) -> String {
    // whisper 只用于 markdown 渲染（外层包 font 标签）：内容原文里的结构
    // 字符先全角化，防止撑破卡片 markdown（详见 reply::md_safe）。
    let flat = reply::md_safe(&reply::flatten_ws(whisper));
    if flat.chars().count() > STATUS_TEXT_MAX_CHARS {
        format!("…{}", tail_by_chars(&flat, STATUS_TEXT_MAX_CHARS - 1))
    } else {
        flat
    }
}

fn render_terminal(s: &ObsCardState, settle: &Settle, keep_trace: bool) -> String {
    let (emoji, verb) = match settle {
        Settle::Completed => ("✅", "Done".to_string()),
        Settle::Failed(_) => ("❌", "Failed".to_string()),
        Settle::Cancelled => ("⏹", "Stopped".to_string()),
        Settle::MaxIterations(reached) => ("❌", format!("Max iterations ({reached})")),
        Settle::Timeout => ("⏰", "Timed out".to_string()),
    };
    // One quiet line, no header/template: the reply message below carries
    // the content; this card is only the run's receipt.
    let mut lines = vec![format!("{emoji} **{verb}** — {}", stats_line(s))];
    if let Settle::Failed(error) = settle {
        lines.push(error_line(error));
    }
    let mut elements =
        vec![json!({ "tag": "markdown", "text_size": "notation", "content": lines.join("\n") })];
    // The trace that streamed live during the run stays on the frozen
    // card as a collapsed panel — the process narrative when texts were
    // recorded, the plain tool trace otherwise — unless the reply
    // message carries it instead (mid-run split with a flushable reply).
    if keep_trace {
        if let Some(panel) = s.trace.terminal_trace_panel() {
            elements.push(panel);
        }
    }
    json!({
        "schema": "2.0",
        "body": { "elements": elements }
    })
    .to_string()
}

/// Terminal receipt for a standalone compact (`/compact` outside a run):
/// one quiet line (outcome + elapsed) plus the result detail — same
/// headerless receipt style as [`render_terminal`].
fn render_compact_terminal(s: &ObsCardState, summary: &str, is_error: bool) -> String {
    let (emoji, verb) = if is_error {
        ("❌", "Compaction failed")
    } else {
        ("✅", "Compacted")
    };
    let mut lines = vec![format!(
        "{emoji} **{verb}** — ⏱ {}",
        fmt_elapsed(s.started_at.elapsed())
    )];
    if is_error {
        lines.push(error_line(summary));
    } else {
        lines.push(format!(
            "<font color='grey'>{}</font>",
            reply::md_safe(summary)
        ));
    }
    json!({
        "schema": "2.0",
        "body": {
            "elements": [
                { "tag": "markdown", "text_size": "notation", "content": lines.join("\n") },
            ],
        },
    })
    .to_string()
}

fn card_json_elements(template: &str, title: &str, elements: &[serde_json::Value]) -> String {
    // Default card width (600px, same as the reply card) — not the narrow
    // compact layout. Slim header/body padding, 12px notation text.
    json!({
        "schema": "2.0",
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

/// One-line run stats: elapsed · steps · tool total, then a greyed
/// technical tail (model · ctx · live out estimate). Real usage
/// (`ctx: x / y`) only lands at response end; `out ~z` is the
/// run-cumulative output estimate.
fn stats_line(s: &ObsCardState) -> String {
    let (head, grey) = stats_parts(s);
    if grey.is_empty() {
        head
    } else {
        format!("{head} · <font color='grey'>{grey}</font>")
    }
}

/// Split the stats line into the always-dark head (elapsed/steps/tools) and
/// the grey tail (model/ctx/out), so callers choose how to color the tail.
/// Shares the segment rules with the trace panel title via
/// [`reply::summary_segments`] — only the icon (⏱ vs 🐾) and coloring differ.
fn stats_parts(s: &ObsCardState) -> (String, String) {
    let (segs, tail) = reply::summary_segments(&reply::TraceTitle {
        steps: s.trace.step_count(),
        elapsed: s.started_at.elapsed(),
        model: s.model.as_deref(),
        ctx_footer: s.token_footer.as_deref(),
        usage_in: s.trace.usage().0,
        usage_out: s.trace.usage().1,
        out_estimate: s.out_estimate(),
        ..Default::default()
    });
    let mut head = vec![format!("⏱ {}", fmt_elapsed(s.started_at.elapsed()))];
    head.extend(segs);
    (head.join(" · "), tail.join(" · "))
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

/// Format a token count compactly: `999` → `999`, `12_345` → `12.3k`,
/// `2_345_678` → `2.3m` (prompt totals climb fast on long runs).
pub(crate) fn fmt_tokens(tokens: u64) -> String {
    if tokens < 1_000 {
        tokens.to_string()
    } else if tokens < 1_000_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        format!("{:.1}m", tokens as f64 / 1_000_000.0)
    }
}

/// Humanize a `snake_case` tool name for the card title: `post_message` → `PostMessage`.
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
