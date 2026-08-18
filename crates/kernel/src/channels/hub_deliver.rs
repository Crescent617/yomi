//! Reply delivery: card morph/freeze/flush, command replies, and
//! run-completion subscription notifications.

use crate::kernel::Kernel;

use crate::types::{ContentBlock, Result, SessionId};
use std::collections::HashMap;

use std::sync::Arc;
use tracing::{error, info, warn};

use super::{
    obs::{ObsTracker, SettleOutcome},
    reply, ChannelMessage, ChannelStore, PlatformAdapter, SessionRouting,
};

/// How a run ended, for the subscription card's emoji and status word.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunEndStatus {
    Completed,
    Cancelled,
    Failed,
}

impl RunEndStatus {
    pub(crate) fn from_stop_reason(reason: &crate::event::StopReason) -> Self {
        match reason {
            crate::event::StopReason::Completed { .. } => Self::Completed,
            crate::event::StopReason::Cancelled { .. } => Self::Cancelled,
            crate::event::StopReason::Failed { .. }
            | crate::event::StopReason::MaxIterations { .. } => Self::Failed,
        }
    }

    fn emoji(self) -> &'static str {
        match self {
            Self::Completed => "✅",
            Self::Cancelled => "⏹",
            Self::Failed => "❌",
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Completed => "任务完成",
            Self::Cancelled => "任务已停止",
            Self::Failed => "任务异常结束",
        }
    }
}

/// Notify `/subscribe` subscribers that a run in `routing`'s scope has
/// delivered its reply. Each subscriber gets ONE card with a
/// jump-to-the-reply button — DM'd by default (deduplicated: holding
/// several matching subscriptions still means a single DM), or posted to
/// their chosen target chat (one card per target, mentioning all
/// subscribers routed to it). Skipped entirely when the run delivered no
/// message to point at (crash without a card), and for cancelled runs —
/// the initiator stopped it themselves, they know (mirrors the settle
/// reaction policy). Per-target failures only affect their target.
pub(crate) async fn notify_run_subscribers(
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    routing: &SessionRouting,
    reply_msg_id: Option<&str>,
    status: RunEndStatus,
    session_id: &SessionId,
    kernel: &std::sync::Weak<Kernel>,
    obs: &ObsTracker,
) {
    if status == RunEndStatus::Cancelled {
        return;
    }
    let Some(reply_msg_id) = reply_msg_id else {
        return;
    };
    let subs = match store
        .list_matching_run_subscriptions(
            &routing.channel_name,
            &routing.mapping_key,
            &routing.external_chat_id,
        )
        .await
    {
        Ok(subs) => subs,
        Err(e) => {
            warn!(error = %e, "failed to list run subscriptions");
            return;
        }
    };
    if subs.is_empty() {
        return;
    }
    let (link, chat_name, quote) = tokio::join!(
        adapter.message_link(&routing.external_chat_id, reply_msg_id),
        adapter.fetch_chat_name(&routing.external_chat_id),
        resolve_notify_quote(adapter, obs, kernel, session_id, routing),
    );
    let mut dm_users: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut chat_targets: HashMap<String, Vec<String>> = HashMap::new();
    for sub in subs {
        match sub.target_chat_id {
            Some(chat_id) => {
                let users = chat_targets.entry(chat_id).or_default();
                if !users.contains(&sub.subscriber_open_id) {
                    users.push(sub.subscriber_open_id);
                }
            }
            None => {
                dm_users.insert(sub.subscriber_open_id);
            }
        }
    }
    for user in dm_users {
        let card = subscription_notify_card(
            status,
            link.as_deref(),
            chat_name.as_deref(),
            &[],
            quote.as_deref(),
        );
        if let Err(e) = adapter.send_direct_card(&user, &card).await {
            warn!(error = %e, "run subscription DM failed");
        }
    }
    for (chat_id, users) in chat_targets {
        let card = subscription_notify_card(
            status,
            link.as_deref(),
            chat_name.as_deref(),
            &users,
            quote.as_deref(),
        );
        if let Err(e) = adapter.send_card(&chat_id, &card, None).await {
            warn!(error = %e, "run subscription notify failed");
        }
    }
}

/// Quote context for the notify card (design:
/// run-subscription-notify-context): the session's latest user message
/// (the very message the settle ✅ lands on), attributed when the
/// author's name resolves (`fetch_user_name` needs contact permission);
/// falls back to the thread's root message, then the session title.
/// `None` keeps the card one-line.
pub(crate) async fn resolve_notify_quote(
    adapter: &Arc<dyn PlatformAdapter>,
    obs: &ObsTracker,
    kernel: &std::sync::Weak<Kernel>,
    session_id: &SessionId,
    routing: &SessionRouting,
) -> Option<String> {
    // The reaction target first (sticky across runs; empty only after a
    // hub restart), then the thread root for thread sessions — a
    // chat-level key (mapping == chat id) is not fetchable as a message.
    let message_id = obs.last_user_msg_id(session_id).or_else(|| {
        (routing.mapping_key != routing.external_chat_id).then(|| routing.mapping_key.clone())
    });
    if let Some(message_id) = message_id {
        if let Ok(Some(msg)) = adapter.fetch_message(&message_id).await {
            let snippet = notify_quote_snippet(&msg.text);
            if !snippet.is_empty() {
                return Some(match adapter.fetch_user_name(&msg.sender_id).await {
                    Some(name) => format!("{name}：{snippet}"),
                    None => snippet,
                });
            }
        }
    }
    let title = kernel
        .upgrade()?
        .get_session(session_id)
        .await
        .ok()?
        .title?;
    let snippet = notify_quote_snippet(&title);
    (!snippet.is_empty()).then_some(snippet)
}

/// One-line snippet for the notify card's quote line: leading mention
/// placeholders (`@_user_N`) stripped, whitespace flattened, capped at 50
/// chars with an ellipsis.
pub(crate) fn notify_quote_snippet(text: &str) -> String {
    let mut rest = text.trim_start();
    while let Some(after) = rest.strip_prefix("@_user_") {
        // Placeholder form is `@_user_<digits>`; skip past it.
        let end = after.find(char::is_whitespace).unwrap_or(after.len());
        rest = after[end..].trim_start();
    }
    let flat = reply::flatten_ws(rest);
    crate::utils::strs::truncate_by_chars(&flat, NOTIFY_QUOTE_MAX_CHARS, "…")
}

/// The run-completion subscription card: a single notation-sized line
/// (mentioning subscribers when posted to a group) in a compact-width
/// card, the whole card clickable via `card_link` — no button, minimal
/// by design. The line names the source chat when known (threads have no
/// name of their own — the group name stands in). The emoji mirrors the
/// run's end status (✅/⏹/❌). Card markdown strips applink URLs, so the
/// jump rides `card_link` instead; without a link it degrades to a
/// text-only ping. An optional `quote` (trigger-message snippet, or the
/// session title) rides as a second markdown-quote line so overlapping
/// subscriptions stay distinguishable.
pub(crate) fn subscription_notify_card(
    status: RunEndStatus,
    link: Option<&str>,
    chat_name: Option<&str>,
    mentions: &[String],
    quote: Option<&str>,
) -> String {
    let mention = mentions
        .iter()
        .map(|u| format!("<at id={u}></at>"))
        .collect::<Vec<_>>()
        .join(" ");
    let scope = match chat_name {
        Some(name) => format!("的「{name}」"),
        None => "的会话".to_string(),
    };
    let text = format!(
        "{}{} 你订阅{}{}",
        if mention.is_empty() {
            String::new()
        } else {
            format!("{mention} ")
        },
        status.emoji(),
        scope,
        status.word(),
    );
    let line = match link {
        Some(_) => format!("{text} · **查看回复 →**"),
        None => text,
    };
    let mut elements =
        vec![serde_json::json!({ "tag": "markdown", "text_size": "notation", "content": line })];
    if let Some(q) = quote {
        elements.push(
            serde_json::json!({ "tag": "markdown", "text_size": "notation",
            "content": format!("> {q}") }),
        );
    }
    let card = match link {
        Some(link) => serde_json::json!({
            "schema": "2.0",
            "config": { "width_mode": "compact" },
            "card_link": { "url": link },
            "body": { "elements": elements }
        }),
        None => serde_json::json!({
            "schema": "2.0",
            "config": { "width_mode": "compact" },
            "body": { "elements": elements }
        }),
    };
    card.to_string()
}

/// Flush a run's final reply as a new message (observability off, platforms
/// without card support, or the mid-run split where the status card freezes
/// as a terminal receipt): send the final text as a single message bubble,
/// with the run trace attached (collapsible panel on card-capable
/// platforms, plain-text lines otherwise). Runs without any text are
/// skipped, matching the pre-buffering behavior.
///
/// Returns the platform message id of the delivered reply — `None` when
/// there was nothing to send or every send attempt failed (the caller
/// relies on it to decide whether the trace still needs a home, and for
/// jump-link notifications).
pub(crate) async fn flush_reply(
    adapter: &Arc<dyn PlatformAdapter>,
    routing: &SessionRouting,
    reply: reply::FinalReply,
    tool_trace: bool,
) -> Option<String> {
    reply.text()?;
    if tool_trace && adapter.supports_status_card() && reply.has_trace() {
        match reply::render_card(&reply, None) {
            Some(card) => {
                match adapter
                    .send_card(
                        &routing.external_chat_id,
                        &card,
                        routing.reply_msg_id.as_deref(),
                    )
                    .await
                {
                    // Platform skipped the card (Ok(None)) — fall through
                    // to text.
                    Ok(Some(msg_id)) => return Some(msg_id),
                    Ok(None) => {}
                    // The card was rejected (oversize payload, API error) —
                    // fall through to a plain text send so the reply content
                    // is never dropped.
                    Err(e) => {
                        warn!(error = %e, "reply card send failed, falling back to plain text");
                    }
                }
            }
            // Unreachable in practice (a text reply always renders) — skip
            // rather than panic; the run's content was already delivered by
            // the settle path or is simply absent.
            None => return None,
        }
    }
    let text = if tool_trace {
        reply::render_plain(&reply)
    } else {
        reply.into_text().unwrap_or_default()
    };
    match adapter
        .send_message(
            &routing.external_chat_id,
            vec![ContentBlock::Text { text }],
            routing.reply_msg_id.as_deref(),
        )
        .await
    {
        Ok(msg_id) => msg_id,
        Err(e) => {
            error!(error = %e, "failed to send reply to platform");
            None
        }
    }
}

/// Deliver a command reply: into the doc's comment thread for
/// doc-comment sessions (there is no chat), the platform message path
/// otherwise.
pub(crate) async fn send_command_reply(
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    reply_msg_id: Option<String>,
    text: String,
) -> Result<()> {
    if let Some(dc) = &msg.doc_comment {
        for chunk in super::comment::chunk_text(&text, super::comment::COMMENT_REPLY_CHUNK_CHARS) {
            adapter
                .reply_doc_comment(&dc.file_token, &dc.file_type, &dc.comment_id, &chunk)
                .await?;
        }
        return Ok(());
    }
    adapter
        .send_message(
            &msg.external_chat_id,
            vec![ContentBlock::Text { text }],
            reply_msg_id.as_deref(),
        )
        .await?;
    Ok(())
}

/// Info-command reply: a header-titled compact card on card-capable
/// platforms (the `/sessions` style — title lives in the blue header);
/// plain text with the title as a bold first line everywhere else.
///
/// Sent inline, so the dispatch loop waits one platform RTT (the same
/// shape `/sessions` has always had): the command's feedback lands in
/// order, and a failed send surfaces as a handler error (leaving the
/// history cursor put) instead of a fire-and-forget log line.
pub(crate) async fn send_info_reply(
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    reply_msg_id: Option<String>,
    title: &str,
    body: String,
) -> Result<()> {
    if msg.doc_comment.is_none() && adapter.supports_status_card() {
        let card = info_card(title, &body);
        adapter
            .send_card(&msg.external_chat_id, &card, reply_msg_id.as_deref())
            .await?;
        return Ok(());
    }
    send_command_reply(adapter, msg, reply_msg_id, format!("**{title}**\n\n{body}")).await
}

/// The shared info-card envelope: blue header + compact body — the
/// `/sessions` style, reused by every info card (single-markdown-body
/// via [`info_card`], multi-element via `/sessions`).
pub(crate) fn info_card_envelope(title: &str, elements: Vec<serde_json::Value>) -> String {
    serde_json::json!({
        "schema": "2.0",
        "config": { "width_mode": "compact" },
        "header": {
            "template": "blue",
            "title": { "tag": "plain_text", "content": title },
        },
        "body": { "elements": elements },
    })
    .to_string()
}

fn info_card(title: &str, body_md: &str) -> String {
    info_card_envelope(
        title,
        vec![serde_json::json!({ "tag": "markdown", "content": body_md })],
    )
}

/// How a run ends, for reply-delivery purposes.
#[derive(Clone, Copy)]
pub(crate) enum SettleKind<'a> {
    Stopped(&'a crate::event::StopReason),
    Timeout,
}

/// Deliver a doc-comment session's reply: there is no chat to morph or
/// flush into — the run's final text goes back to the document as comment
/// replies (chunked at the platform's comment length; a failed chunk
/// doesn't stop the rest — partial delivery beats none). Attachments
/// can't ride comment replies: they are dropped with a visible note
/// appended to the reply instead of vanishing silently.
pub(crate) async fn deliver_doc_comment_reply(
    adapter: &Arc<dyn PlatformAdapter>,
    dc: &super::DocCommentRef,
    reply: Option<reply::FinalReply>,
) -> Option<String> {
    let mut reply = reply?;
    let attachments = reply.attachments().len();
    if attachments > 0 {
        warn!(
            comment_id = %dc.comment_id,
            attachments,
            "doc comment reply cannot carry attachments, dropping"
        );
        reply.push_note(&format!(
            "（本次运行产出 {attachments} 个附件，无法投递到文档评论）"
        ));
    }
    let text = reply.text()?.to_string();
    let mut last_id = None;
    for chunk in super::comment::chunk_text(&text, super::comment::COMMENT_REPLY_CHUNK_CHARS) {
        match adapter
            .reply_doc_comment(&dc.file_token, &dc.file_type, &dc.comment_id, &chunk)
            .await
        {
            Ok(id) => last_id = id.or(last_id),
            Err(e) => {
                warn!(error = %e, comment_id = %dc.comment_id, "doc comment reply chunk failed");
            }
        }
    }
    if last_id.is_some() {
        // Doc-bound replies are otherwise invisible (no chat surface) —
        // this is the only delivery breadcrumb.
        info!(comment_id = %dc.comment_id, "doc comment reply delivered");
    }
    last_id
}

pub(crate) async fn settle_with(
    obs: &Arc<ObsTracker>,
    session_id: &SessionId,
    kind: SettleKind<'_>,
    reply: Option<reply::FinalReply>,
) -> SettleOutcome {
    match kind {
        SettleKind::Stopped(reason) => obs.handle_stopped(session_id, reason, reply).await,
        SettleKind::Timeout => obs.handle_timeout(session_id, reply).await,
    }
}

/// Flush an optional reply — shared by the mid-run-split and plain-platform
/// branches of `deliver_reply`.
pub(crate) async fn flush_optional_reply(
    adapter: &Arc<dyn PlatformAdapter>,
    routing: &SessionRouting,
    reply: Option<reply::FinalReply>,
    tool_trace: bool,
) -> Option<String> {
    match reply {
        Some(reply) => flush_reply(adapter, routing, reply, tool_trace).await,
        None => None,
    }
}

/// Freeze the status card in place as a terminal receipt (mid-run split;
/// see `deliver_reply`). `keep_trace` = the card itself carries the run
/// trace panel — false when the reply message carries it instead.
pub(crate) async fn freeze_with(
    obs: &Arc<ObsTracker>,
    session_id: &SessionId,
    kind: SettleKind<'_>,
    keep_trace: bool,
) {
    match kind {
        SettleKind::Stopped(reason) => obs.freeze_stopped(session_id, reason, keep_trace).await,
        SettleKind::Timeout => obs.freeze_timeout(session_id, keep_trace).await,
    }
}

/// Deliver a run's final reply, then its attachment files. Declared
/// attachments (`<yomi_attachments>` blocks, stripped at record time) are
/// resolved up front — resolution notes ride with the reply text — while
/// the files themselves go out AFTER the reply, landing at the bottom of
/// the chat. The reply itself: card-capable platforms with observability
/// morph the status card into it (one message per run) — or, when the user
/// posted mid-run and `mid_run_split` is enabled, flush the reply as a new
/// message at the bottom carrying the run trace, then freeze the card in
/// place as a terminal receipt (the card keeps the trace panel itself
/// whenever the reply didn't carry it). All other cases flush as a new
/// message and settle the obs state without a reply. When the rich
/// settle comes back unsettled (no run state, or the settle send failed),
/// the reply falls back to a plain flush so content is never silently lost.
///
/// Returns the platform message id of the delivered reply (the morphed
/// status card or the flushed message) for jump-link notifications —
/// `None` when nothing was delivered.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn deliver_reply(
    obs: &Arc<ObsTracker>,
    adapter: &Arc<dyn PlatformAdapter>,
    routing: &SessionRouting,
    reply: Option<reply::FinalReply>,
    tool_trace: bool,
    observability: bool,
    mid_run_split: bool,
    session_id: &SessionId,
    kind: SettleKind<'_>,
    kernel: &std::sync::Weak<Kernel>,
) -> Option<String> {
    // Doc-comment sessions have no chat surface at all — the reply goes
    // back to the document's comment thread (no cards, no receipts).
    if let Some(dc) = &routing.doc_comment {
        return deliver_doc_comment_reply(adapter, dc, reply).await;
    }
    let mut reply = reply;
    // Split files off the reply up front: resolution failures become notes
    // on the reply text; the files are sent after the reply below.
    let files = match reply.as_mut() {
        Some(reply) if !reply.attachments().is_empty() => {
            // Best-effort workspace lookup for relative paths; a gone
            // kernel still allows absolute-path attachments.
            let cwd = match kernel.upgrade() {
                Some(k) => Some(k.session_cwd(session_id).await),
                None => None,
            };
            super::attachments::resolve_attachments(cwd.as_deref(), reply).await
        }
        _ => Vec::new(),
    };
    let reply_msg_id = if observability && adapter.supports_status_card() {
        // A mention forces the split even in a quiet chat: card patches
        // never notify (feishu), so the only way an @ pings is a new
        // message carrying the reply — same landing path as mid-run posts.
        let has_mention = reply
            .as_ref()
            .and_then(|r| r.text())
            .is_some_and(super::utils::contains_mention);
        if (mid_run_split && obs.has_mid_run_posts(session_id)) || has_mention {
            // The reply lands as a new message below the user's mid-run
            // posts, carrying the run trace; the status card then freezes
            // in place as a terminal receipt. Flush first and freeze with
            // the outcome: the card keeps the trace panel itself whenever
            // the reply didn't carry it (nothing delivered — no text or
            // every send failed — or the trace is disabled), so the trace
            // is never lost.
            let delivered = flush_optional_reply(adapter, routing, reply, tool_trace).await;
            let keep_trace = !tool_trace || delivered.is_none();
            freeze_with(obs, session_id, kind, keep_trace).await;
            delivered
        } else {
            // Morph in place (no mid-run posts, or the split disabled).
            // Receipts only drive the split decision — with the split
            // disabled they must not suppress the settle reaction for
            // this silent in-place morph.
            if !mid_run_split {
                obs.clear_receipts(session_id);
            }
            let outcome = settle_with(obs, session_id, kind, reply).await;
            match outcome.unsettled {
                // Nothing settled — fall back to a plain message instead of
                // dropping the reply.
                Some(reply) => flush_reply(adapter, routing, reply, tool_trace).await,
                None => outcome.message_id,
            }
        }
    } else {
        // Platforms without card support cannot morph — the reply goes out
        // as a plain message; obs still settles its memory-only state
        // (typing fallback).
        let flushed = flush_optional_reply(adapter, routing, reply, tool_trace).await;
        if observability {
            let _ = settle_with(obs, session_id, kind, None).await;
        }
        flushed
    };

    // Attachments last: files land at the bottom of the chat, below the
    // reply text/card.
    super::attachments::send_attachments(adapter, routing, files).await;
    reply_msg_id
}

/// Max chars for the subscription notify card's quote line (ellipsis
/// included — see `notify_quote_snippet`).
pub(crate) const NOTIFY_QUOTE_MAX_CHARS: usize = 50;
