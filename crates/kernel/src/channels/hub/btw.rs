//! `/btw` side questions on channel surfaces (Feishu).
//!
//! The kernel answers a btw on a read-only snapshot bypass (see
//! `kernel::btw`) — nothing enters session history, the mailbox, or the
//! run-card lifecycle. On a channel the reply message **is** the whole
//! trace: a dedicated card streams the answer and freezes as the final
//! receipt, visually self-labelled as a side question.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use tracing::{info, warn};

use crate::channels::hub_routing::command_session_key;
use crate::channels::{ChannelMessage, ChannelStore, PlatformAdapter};
use crate::comms::EventBusSubscriber;
use crate::event::{BtwEndReason, BtwEvent, Envelope, Event};
use crate::kernel::Kernel;
use crate::types::Result;
use crate::utils::strs::truncate_by_chars;

/// Throttle between card patches while the answer streams.
const PATCH_INTERVAL: Duration = Duration::from_millis(700);
/// Overall guard: a btw answer is one short completion; past this the
/// delivery freezes whatever arrived with a timeout note instead of
/// holding the subscription forever.
const BTW_MAX_WAIT: Duration = Duration::from_mins(5);
/// Listener queue for the btw stream: deltas may outpace the patch
/// throttle during a slow PATCH, so size above the default 256.
const BTW_QUEUE_CAPACITY: usize = 2048;
/// Card answer budget (chars). The wrap instruction asks for ≤10 lines;
/// this only caps a runaway model so the card payload stays well under
/// the platform limit.
const ANSWER_MAX_CHARS: usize = 6_000;
/// Header question truncation (`plain_text` header).
const QUESTION_MAX_CHARS: usize = 40;
/// Card error line budget (chars).
const ERROR_MAX_CHARS: usize = 200;

/// `handle_incoming_message` arm for `ChannelCommand::Btw`: resolve the
/// conversation's session, publish the btw, and hand the answer stream to
/// a spawned delivery task. `Ok(Some(text))` is immediate error feedback
/// (no session / publish failed); `Ok(None)` means the card is the reply.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_btw(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Arc<Kernel>,
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    reply_msg_id: Option<String>,
    reply_in_thread: bool,
    mapping_key: &str,
    question: String,
) -> Result<Option<String>> {
    let key = command_session_key(msg, reply_in_thread, &msg.external_chat_id, mapping_key);
    let Some(sid) = store.find_mapping(channel_name, key).await? else {
        return Ok(Some(
            "No session here yet — mention me to start one.".to_string(),
        ));
    };
    // Stale mapping guard: a routing row can outlive the session itself
    // (GC / manual delete); publishing to a dead session would leave the
    // pending card hanging until the timeout.
    if kernel.get_session(&sid).await.is_err() {
        return Ok(Some(
            "No session here yet — mention me to start one.".to_string(),
        ));
    }
    let Some(bus) = kernel.event_bus() else {
        return Ok(Some(
            "⚠️ Side question unavailable (no event bus).".to_string(),
        ));
    };
    // Subscribe before publishing: btw events are realtime-only (never
    // replayed from the buffer), a subscription made after `btw()` would
    // race the Start event.
    let filter_sid = sid.clone();
    let mut rx = bus
        .subscribe_all_filtered_with_capacity(BTW_QUEUE_CAPACITY, move |env: &Envelope| {
            env.session_id == filter_sid && matches!(env.event, Event::Btw(_))
        });
    let request_id = match kernel.btw(&sid, question.clone(), None).await {
        Ok(id) => id,
        Err(e) => {
            drop(rx);
            return Ok(Some(format!("⚠️ Failed to start side question: {e}")));
        }
    };
    let adapter = Arc::clone(adapter);
    let chat_id = msg.external_chat_id.clone();
    tokio::spawn(async move {
        stream_btw_answer(
            &adapter,
            &mut rx,
            chat_id,
            reply_msg_id,
            request_id,
            question,
        )
        .await;
    });
    Ok(None)
}

/// Delivery task: stream the answer onto one card (or one text message on
/// card-less platforms) and freeze it as the final receipt. Exits on Done
/// (any reason), bus close, or the overall guard.
async fn stream_btw_answer(
    adapter: &Arc<dyn PlatformAdapter>,
    rx: &mut EventBusSubscriber,
    chat_id: String,
    reply_msg_id: Option<String>,
    request_id: crate::types::BtwId,
    question: String,
) {
    // Card mode degrades to text mode when the initial send fails:
    // whatever the platform rejected once will keep rejecting.
    let mut card_id: Option<String> = None;
    if adapter.supports_status_card() {
        match adapter
            .send_card(
                &chat_id,
                &btw_card(&question, "", BtwPhase::Pending),
                reply_msg_id.as_deref(),
            )
            .await
        {
            Ok(id) => card_id = id,
            Err(e) => warn!(error = %e, "btw initial card failed, falling back to text"),
        }
    }
    let mut answer = String::new();
    let mut dirty = false;
    let mut tick = tokio::time::interval(PATCH_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let deadline = tokio::time::sleep(BTW_MAX_WAIT);
    tokio::pin!(deadline);
    // The Start event pins the request id; until it arrives the id
    // `kernel.btw()` generated and returned is the expectation (it is
    // what the conductor echoes back, so they agree in practice).
    let mut rid = request_id;
    loop {
        tokio::select! {
            () = &mut deadline => {
                finish(adapter, card_id.as_deref(), &chat_id, reply_msg_id.as_deref(), &question, &answer, BtwEnd::Timeout).await;
                break;
            }
            _ = tick.tick() => {
                if dirty {
                    dirty = !patch(adapter, card_id.as_deref(), &question, &answer, BtwPhase::Streaming).await;
                }
            }
            item = rx.recv() => {
                let Some((_, env)) = item else {
                    // Bus closed (daemon shutdown): freeze whatever
                    // arrived instead of abandoning a spinner card.
                    finish(adapter, card_id.as_deref(), &chat_id, reply_msg_id.as_deref(), &question, &answer, BtwEnd::Lost).await;
                    break;
                };
                let Event::Btw(ev) = env.event else { continue };
                match ev {
                    // Re-pin rid unconditionally: the conductor's
                    // per-session lock guarantees a replaced stream's
                    // Done{Replaced} is emitted BEFORE the new Start, so
                    // the previous delivery task has already exited — a
                    // Start here always belongs to the current stream.
                    BtwEvent::Start { request_id } => rid = request_id,
                    BtwEvent::Delta { request_id, text } => {
                        if request_id != rid {
                            continue;
                        }
                        answer.push_str(&text);
                        dirty = true;
                    }
                    BtwEvent::Done { request_id, reason } => {
                        if request_id != rid {
                            continue;
                        }
                        finish(adapter, card_id.as_deref(), &chat_id, reply_msg_id.as_deref(), &question, &answer, BtwEnd::Reason(reason)).await;
                        break;
                    }
                }
            }
        }
    }
}

/// How the stream ended, for the terminal render.
pub(crate) enum BtwEnd {
    Reason(BtwEndReason),
    Timeout,
    /// Event bus closed mid-answer (daemon shutdown).
    Lost,
}

/// Render phase for the live card.
#[derive(Clone, Copy)]
pub(crate) enum BtwPhase {
    Pending,
    Streaming,
}

/// One throttled card patch; `true` when the patch landed (or there is
/// no card to patch), `false` when it failed and the buffer stays dirty
/// for the next tick.
async fn patch(
    adapter: &Arc<dyn PlatformAdapter>,
    card_id: Option<&str>,
    question: &str,
    answer: &str,
    phase: BtwPhase,
) -> bool {
    let Some(id) = card_id else {
        return true;
    };
    match adapter
        .update_card(id, &btw_card(question, answer, phase))
        .await
    {
        Ok(()) => true,
        Err(e) => {
            warn!(error = %e, "btw card patch failed");
            false
        }
    }
}

/// Terminal update: freeze the card (or send the single text reply on
/// card-less platforms), then log the breadcrumb.
async fn finish(
    adapter: &Arc<dyn PlatformAdapter>,
    card_id: Option<&str>,
    chat_id: &str,
    reply_msg_id: Option<&str>,
    question: &str,
    answer: &str,
    end: BtwEnd,
) {
    match card_id {
        Some(id) => {
            // Terminal state is the user's last signal — retry a failed
            // final patch instead of leaving a spinner card behind.
            let card = btw_card_final(question, answer, &end);
            for attempt in 1..=3 {
                match adapter.update_card(id, &card).await {
                    Ok(()) => break,
                    Err(e) => {
                        warn!(attempt, error = %e, "btw final card patch failed");
                        if attempt < 3 {
                            tokio::time::sleep(Duration::from_millis(300)).await;
                        }
                    }
                }
            }
        }
        None => {
            let text = format!(
                "💭 btw · {}\n\n{}\n\n{}",
                truncate_by_chars(question, QUESTION_MAX_CHARS, "…"),
                final_body(answer, &end),
                terminal_note(&end),
            );
            if let Err(e) = adapter
                .send_message(
                    chat_id,
                    vec![crate::types::ContentBlock::Text { text }],
                    reply_msg_id,
                )
                .await
            {
                warn!(error = %e, "btw text reply failed");
            }
        }
    }
    info!("btw side question delivered");
}

/// Answer + terminal note. A `ToolUse` finish (or an empty answer) gets
/// the matching fallback body so the card never freezes bare.
fn final_body(answer: &str, end: &BtwEnd) -> String {
    if !answer.is_empty() {
        return capped(answer);
    }
    let fallback = match end {
        BtwEnd::Reason(BtwEndReason::ToolUse) => TOOL_USE_FALLBACK,
        _ => EMPTY_ANSWER_FALLBACK,
    };
    fallback.to_string()
}

/// The terminal line carrying the outcome + the side-question semantics
/// ("never enters history") so the frozen card is self-explanatory in
/// the thread. Plain text — the card wraps it in a color font, the
/// text-platform fallback sends it verbatim (HTML would leak there).
fn terminal_note(end: &BtwEnd) -> String {
    match end {
        BtwEnd::Reason(BtwEndReason::Stop | BtwEndReason::ToolUse) => {
            "💭 side question · never enters history".to_string()
        }
        BtwEnd::Reason(BtwEndReason::Replaced) => "Replaced by a newer side question".to_string(),
        BtwEnd::Reason(BtwEndReason::Cancelled) => "Cancelled".to_string(),
        BtwEnd::Reason(BtwEndReason::Error(e)) => format!(
            "🙀 {}",
            crate::channels::render::reply::md_safe(&truncate_by_chars(e, ERROR_MAX_CHARS, "…"))
        ),
        BtwEnd::Timeout => "⏰ Side question timed out — answer above is partial".to_string(),
        BtwEnd::Lost => "Event stream interrupted".to_string(),
    }
}

/// Grey for information, red for the error line.
fn terminal_note_color(end: &BtwEnd) -> &'static str {
    match end {
        BtwEnd::Reason(BtwEndReason::Error(_)) => "red",
        _ => "grey",
    }
}

/// Fallback body when the model produced no text at all.
const EMPTY_ANSWER_FALLBACK: &str = "(no text answer)";

/// Pure-tool_use answers carry no text by design; the kernel signals it
/// via the `ToolUse` reason so the client can say "needs a real prompt".
const TOOL_USE_FALLBACK: &str = "This needs a real prompt (side questions can't use tools).";

fn capped(answer: &str) -> String {
    // Truncation can cut inside a ``` fence — Feishu degrades the whole
    // element to plain text with raw tags leaking; close it.
    crate::channels::render::reply::balance_fences(&truncate_by_chars(
        answer,
        ANSWER_MAX_CHARS,
        "…",
    ))
    .into_owned()
}

/// Live card: header carries the question, the body streams the answer
/// with a quiet status line underneath.
pub(crate) fn btw_card(question: &str, answer: &str, phase: BtwPhase) -> String {
    let status = match phase {
        BtwPhase::Pending => "<font color='grey'>Thinking…</font>",
        BtwPhase::Streaming => "<font color='grey'>Answering…</font>",
    };
    let mut elements =
        vec![json!({ "tag": "markdown", "text_size": "notation", "content": status })];
    if !answer.is_empty() {
        elements.insert(0, json!({ "tag": "markdown", "content": capped(answer) }));
    }
    btw_card_envelope(question, &elements)
}

/// Terminal card: answer on top, terminal note underneath.
pub(crate) fn btw_card_final(question: &str, answer: &str, end: &BtwEnd) -> String {
    let note = format!(
        "<font color='{}'>{}</font>",
        terminal_note_color(end),
        terminal_note(end)
    );
    let body = format!("{}\n\n{}", final_body(answer, end), note);
    btw_card_envelope(question, &[json!({ "tag": "markdown", "content": body })])
}

fn btw_card_envelope(question: &str, elements: &[serde_json::Value]) -> String {
    json!({
        "schema": "2.0",
        "header": {
            "template": "wathet",
            "title": {
                "tag": "plain_text",
                "content": format!("💭 btw · {}", truncate_by_chars(question, QUESTION_MAX_CHARS, "…")),
            },
        },
        "body": { "elements": elements },
    })
    .to_string()
}
