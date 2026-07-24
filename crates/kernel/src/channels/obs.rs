//! Run observability for external channels: status card + reaction state machine.
//!
//! A "run" is bracketed by `AgentEvent::Lifecycle(Running)` and
//! `AgentEvent::Lifecycle(Stopped)` (see `docs/design/feishu-channel-observability.md`).
//! Run state is tracked from `Running`, but the status card is only
//! materialized on the first `ToolEvent::Start` — short no-tool runs never
//! show a card. Once sent, the card is updated in place; on settlement it
//! freezes into a terminal style and every user message received during the
//! run has its ack reaction replaced, while the last content reply gets a
//! completion reaction.

use crate::event::{AgentEvent, AgentStatus, Event, ModelEvent, StopReason, ToolEvent};
use crate::types::SessionId;
use dashmap::DashMap;
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

use super::PlatformAdapter;

/// Minimum interval between two in-place card updates (low-frequency updates).
const PATCH_MIN_INTERVAL: Duration = Duration::from_millis(1500);
/// Error summary truncation on terminal cards.
const ERROR_MAX_CHARS: usize = 200;

const PHASE_THINKING: &str = "💭 Thinking…";
const PHASE_TYPING: &str = "🐾 Typing…";

const EMOJI_DONE: &str = "DONE";
const EMOJI_CROSS: &str = "CrossMark";

/// Per-session live status card state.
struct ObsCardState {
    status_msg_id: String,
    adapter: Arc<dyn PlatformAdapter>,
    /// Routing captured when the run started (card anchor).
    chat_id: String,
    reply_msg_id: Option<String>,
    started_at: Instant,
    /// Full card title text (icon + phase, e.g. "💭 Thinking…", "🐶 bash…").
    phase: String,
    /// Total tool executions (per-tool breakdown is not kept).
    tool_count: u32,
    token_footer: Option<String>,
    last_patch_at: Instant,
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
            phase: PHASE_THINKING.to_string(),
            tool_count: 0,
            token_footer: None,
            last_patch_at: now,
        }
    }
}

/// Receipts collected during a run: user messages that got an ack reaction,
/// plus the latest content reply message (reaction target on completion).
#[derive(Default)]
pub(crate) struct RunReceipts {
    /// (`message_id`, ack `reaction_id`) pairs of user messages.
    items: Vec<(String, String)>,
    /// Message ID of the most recent content reply sent to the platform.
    last_content_msg_id: Option<String>,
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
    /// Reaction to apply to run receipts; `None` leaves reactions untouched.
    fn receipt_emoji(&self) -> Option<&'static str> {
        match self {
            Settle::Completed => Some(EMOJI_DONE),
            Settle::Failed(_) | Settle::Cancelled | Settle::MaxIterations(_) => Some(EMOJI_CROSS),
            Settle::Timeout => None,
        }
    }
}

/// Tracks per-session observability state and drives the platform adapter.
pub(crate) struct ObsTracker {
    states: DashMap<SessionId, ObsCardState>,
    receipts: DashMap<SessionId, RunReceipts>,
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

    /// Record the ack reaction of a user message (called when the message is
    /// routed to a session, before the run starts/continues).
    pub(crate) fn record_receipt(
        &self,
        session_id: &SessionId,
        message_id: String,
        reaction_id: String,
    ) {
        self.receipts
            .entry(session_id.clone())
            .or_default()
            .items
            .push((message_id, reaction_id));
    }

    /// Record the message ID of a content reply (reaction target on completion).
    pub(crate) fn record_content_msg(&self, session_id: &SessionId, message_id: String) {
        if let Some(mut r) = self.receipts.get_mut(session_id) {
            r.last_content_msg_id = Some(message_id);
        }
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
                // start — no-tool runs never show a status card.
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
                let settle = match reason {
                    StopReason::Completed { .. } => Settle::Completed,
                    StopReason::Cancelled { .. } => Settle::Cancelled,
                    StopReason::Failed { error } => Settle::Failed(error.clone()),
                    StopReason::MaxIterations { reached } => Settle::MaxIterations(*reached),
                };
                self.settle_card(session_id, &settle).await;
                if let Some(emoji) = settle.receipt_emoji() {
                    self.settle_receipts(session_id, emoji, adapter).await;
                }
            }
            Event::Tool(ToolEvent::Start { tool_name, .. }) => {
                let tool_name = tool_name.clone();
                self.update_running(session_id, |s| {
                    s.tool_count += 1;
                    s.phase = format!("🐶 {}…", humanize_tool_name(&tool_name));
                })
                .await;
                // First tool of a run materializes the status card.
                self.materialize_card(session_id).await;
            }
            Event::Agent(AgentEvent::Retrying {
                attempt,
                max_attempts,
                reason,
            }) => {
                let phase = format!(
                    "🔁 Retrying {attempt}/{max_attempts}: {}",
                    truncate_chars(reason, 30)
                );
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Agent(AgentEvent::Error { error, .. }) => {
                let phase = format!("⚠️ Error: {}", truncate_chars(error, 30));
                // Never settles — a mid-retry error may still recover (see design §3).
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Agent(AgentEvent::GoalUpdated { status, .. }) => {
                let phase = format!("🎯 Goal: {status}");
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Model(ModelEvent::Compacting { active }) => {
                let active = *active;
                self.update_running(session_id, |s| {
                    s.phase = if active {
                        "📦 Compacting context…".to_string()
                    } else {
                        PHASE_THINKING.to_string()
                    };
                })
                .await;
            }
            Event::Model(ModelEvent::Fallback { from, to, .. }) => {
                let phase = format!("↪️ Fallback: {from} → {to}");
                self.update_running(session_id, |s| s.phase = phase).await;
            }
            Event::Model(ModelEvent::Request { .. }) => {
                // A new model call (re-)starts: thinking until chunks arrive.
                self.update_running(session_id, |s| s.phase = PHASE_THINKING.to_string())
                    .await;
            }
            Event::Model(ModelEvent::Chunk { content, .. }) => {
                let phase = match content {
                    crate::event::ContentChunk::Text(_) => PHASE_TYPING,
                    crate::event::ContentChunk::Thinking { .. }
                    | crate::event::ContentChunk::RedactedThinking => PHASE_THINKING,
                };
                self.update_running(session_id, |s| s.phase = phase.to_string())
                    .await;
            }
            Event::Model(ModelEvent::TokenUsage {
                total_tokens,
                context_window,
                ..
            }) => {
                let footer = format!(
                    "tokens: {} / {}",
                    fmt_k(*total_tokens),
                    fmt_k(*context_window)
                );
                self.update_running(session_id, |s| s.token_footer = Some(footer))
                    .await;
            }
            // Other events carry no card-visible state.
            _ => {}
        }
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
            self.settle_card(&sid, &Settle::Timeout).await;
        }
    }

    // ── internals ─────────────────────────────────────────────────────

    /// Send the status card if not yet materialized (triggered by the first
    /// tool start). State is tracked from `Running`, so the initial render
    /// already carries any pre-tool phases and token usage.
    async fn materialize_card(&self, session_id: &SessionId) {
        let (card, chat_id, reply_msg_id, adapter) = {
            let Some(entry) = self.states.get(session_id) else {
                return;
            };
            let s = entry.value();
            if !s.status_msg_id.is_empty() {
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
            Err(e) => warn!(error = %e, "obs status card send failed"),
        }
    }

    /// Mutate a running card's state and PATCH if outside the throttle window.
    /// Before the card is materialized (no tool ran yet) this is memory-only.
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
            if let Err(e) = adapter.update_card(&msg_id, &card).await {
                warn!(error = %e, "obs status card patch failed");
            }
        }
    }

    /// Freeze the status card into its terminal style and drop the state.
    /// Cards never materialized (no-tool runs) are dropped silently — except
    /// failures, which are sent as a terminal card so the user gets an
    /// explanation, not just a `CrossMark` reaction.
    async fn settle_card(&self, session_id: &SessionId, settle: &Settle) {
        let Some((_, state)) = self.states.remove(session_id) else {
            return;
        };
        let card = render_terminal(&state, settle);
        if state.status_msg_id.is_empty() {
            if matches!(settle, Settle::Failed(_)) {
                if let Err(e) = state
                    .adapter
                    .send_card(&state.chat_id, &card, state.reply_msg_id.as_deref())
                    .await
                {
                    warn!(error = %e, "obs terminal card send failed");
                }
            }
            return;
        }
        if let Err(e) = state.adapter.update_card(&state.status_msg_id, &card).await {
            warn!(error = %e, "obs terminal card patch failed");
        }
    }

    /// Replace ack reactions on all run receipts and react on the last
    /// content reply, then drop the receipts.
    async fn settle_receipts(
        &self,
        session_id: &SessionId,
        emoji: &str,
        adapter: &Arc<dyn PlatformAdapter>,
    ) {
        let Some((_, receipts)) = self.receipts.remove(session_id) else {
            return;
        };
        for (msg_id, reaction_id) in &receipts.items {
            if let Err(e) = adapter.remove_reaction(msg_id, reaction_id).await {
                warn!(error = %e, "obs remove ack reaction failed");
            }
            if let Err(e) = adapter.send_reaction("", msg_id, emoji).await {
                warn!(error = %e, "obs settle reaction failed");
            }
        }
        if let Some(content_msg_id) = receipts.last_content_msg_id {
            if let Err(e) = adapter.send_reaction("", &content_msg_id, emoji).await {
                warn!(error = %e, "obs content reaction failed");
            }
        }
    }
}

impl Default for ObsTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ── Rendering ───────────────────────────────────────────────────────

fn render_running(s: &ObsCardState) -> String {
    card_json("blue", &s.phase, &stats_line(s))
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
        lines.push(format!(
            "**Error**  {}",
            truncate_chars(error, ERROR_MAX_CHARS)
        ));
    }
    card_json(template, &title, &lines.join("\n"))
}

fn card_json(template: &str, title: &str, body_md: &str) -> String {
    // Compact layout: 400px width, slim header/body padding, 12px notation text,
    // single markdown element.
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
            "elements": [
                { "tag": "markdown", "text_size": "notation", "content": body_md },
            ],
        },
    })
    .to_string()
}

/// One-line run stats: elapsed · tool total · tokens (greyed).
fn stats_line(s: &ObsCardState) -> String {
    use std::fmt::Write as _;
    let mut line = format!("⏱ {}", fmt_elapsed(s.started_at.elapsed()));
    if s.tool_count > 0 {
        let _ = write!(line, " · {} tools", s.tool_count);
    }
    if let Some(footer) = &s.token_footer {
        let _ = write!(line, " · <font color='grey'>{footer}</font>");
    }
    line
}

fn fmt_elapsed(d: Duration) -> String {
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

fn truncate_chars(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max).collect();
    format!("{truncated}…")
}

#[cfg(test)]
#[path = "obs_test.rs"]
mod tests;
