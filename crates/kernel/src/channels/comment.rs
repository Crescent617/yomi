//! Feishu doc comments: policy filtering, content fetch, and assembly of
//! the injected user message whose meta header marks the doc provenance.
//! See `docs/archive/feishu-doc-comment.md`.

use super::{
    ChannelConfig, ChannelError, ChannelMessage, ChannelStore, DocCommentDetail, DocCommentNotice,
    DocCommentRef, PlatformAdapter,
};
use crate::types::ContentBlock;
use std::fmt::Write as _;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

/// One comment reply's max length (chars). Feishu comment bodies are
/// plain text with a platform cap; longer run replies go out as
/// consecutive replies (reference implementations use the same 4000).
pub(super) const COMMENT_REPLY_CHUNK_CHARS: usize = 4000;

/// Quote snippet cap in the meta block (chars) — the quote is an anchor
/// hint, the agent can always read the full doc for more.
const QUOTE_MAX_CHARS: usize = 300;

/// Thread-history replies injected per trigger (newest win).
const THREAD_HISTORY_MAX: usize = 20;
/// Per-reply cap in the history block (chars).
const THREAD_HISTORY_REPLY_MAX_CHARS: usize = 2000;

/// Handle one `drive.notice.comment_add_v1` event: policy-filter, fetch
/// the comment content, assemble the user message and hand it to the
/// serial dispatch path as an allowed trigger. Filtered-out events are
/// logged and dropped. Runs off the gate loop (spawned) — the comment
/// fetch must not stall chat processing.
pub(super) async fn handle_doc_comment_added(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    adapter: &Arc<dyn PlatformAdapter>,
    dispatch_tx: &mpsc::Sender<(ChannelMessage, super::hub::Gate)>,
    notice: DocCommentNotice,
) {
    // Feature toggle first: disabled means zero platform API calls and no
    // session — just a debug line per event.
    if config
        .disabled_events
        .iter()
        .any(|e| e == super::EVENT_DOC_COMMENT)
    {
        debug!(
            channel = %channel_name,
            comment_id = %notice.comment_id,
            "doc comment ignored (feature disabled)"
        );
        return;
    }
    if !matches!(notice.notice_type.as_str(), "add_comment" | "add_reply") {
        debug!(
            channel = %channel_name,
            notice_type = %notice.notice_type,
            "doc comment event ignored (notice type)"
        );
        return;
    }
    // Trigger policy: only comments that @ the bot. The app also gets
    // notified e.g. as doc owner — those would amplify cost and noise.
    if !notice.is_mentioned {
        debug!(
            channel = %channel_name,
            comment_id = %notice.comment_id,
            "doc comment ignored (bot not mentioned)"
        );
        return;
    }
    if let Err(e) = check_commenter_access(config, &notice.commenter_open_id) {
        info!(channel = %channel_name, error = %e, "doc comment access denied");
        // Chat parity: an allowlist miss on an addressed comment gets the
        // soft deny reaction; blocklist hits stay silent.
        if e.is_allowlist_miss() {
            fire_reaction(adapter, &notice, config.platform.access_denied_reaction());
        }
        return;
    }

    // Accepted — ack the triggering reply (fire-and-forget, mirrors the
    // chat gate's ack reaction).
    fire_reaction(adapter, &notice, config.platform.ack_reaction());

    let (detail, title) = tokio::join!(
        fetch_detail_with_trigger(adapter.as_ref(), &notice),
        adapter.fetch_doc_title(&notice.file_token, &notice.file_type),
    );
    let (detail, fetch_error) = match detail {
        Ok(Some(d)) => (Some(d), None),
        // A deleted comment triggers nothing — there is nothing to answer.
        Ok(None) => {
            info!(
                channel = %channel_name,
                comment_id = %notice.comment_id,
                "doc comment gone (deleted?), skipped"
            );
            return;
        }
        // Content fetch failing must not silently lose the trigger:
        // inject the bare meta with a note — the agent can still act
        // (e.g. read the doc itself with its own tools).
        Err(e) => {
            warn!(
                channel = %channel_name,
                comment_id = %notice.comment_id,
                error = %e,
                "doc comment fetch failed, injecting bare meta"
            );
            (None, Some(e.to_string()))
        }
    };

    let reply_text = detail
        .as_ref()
        .map(|d| pick_triggering_reply(d, notice.reply_id.as_deref()));
    // The bare comment text (@bot stripped): session title input, and the
    // command check below.
    let bare_text = reply_text
        .as_deref()
        .map(|t| t.replace("@bot", "").trim().to_string())
        .filter(|t| !t.is_empty());
    let text = assemble_message(
        &notice,
        detail.as_ref().and_then(|d| d.quote.as_deref()),
        reply_text.as_deref().unwrap_or(""),
        fetch_error.as_deref(),
        title.as_deref(),
    );
    // The comment thread's prior replies ride as a leading context block
    // (history first, trigger last — the chat convention). Skipped for
    // commands: they bypass the session, so building (and cursor-advancing)
    // history would swallow never-injected replies.
    let mut content = Vec::new();
    let is_command = bare_text
        .as_deref()
        .is_some_and(super::hub::has_channel_command_prefix);
    if !is_command {
        if let Some(d) = &detail {
            if let Some(history) = build_thread_history(store, channel_name, &notice, d).await {
                content.push(history);
            }
        }
    }
    content.push(ContentBlock::Text { text });
    info!(
        channel = %channel_name,
        comment_id = %notice.comment_id,
        reply_id = notice.reply_id.as_deref().unwrap_or(""),
        commenter = %notice.commenter_open_id,
        "doc comment accepted"
    );
    let msg = ChannelMessage {
        external_chat_id: String::new(),
        external_user_id: notice.commenter_open_id.clone(),
        external_message_id: None,
        is_mention: true,
        // Feeds the session title; also what commands parse from.
        raw_text: bare_text,
        content,
        image_keys: Vec::new(),
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: false,
        create_time: notice.create_time,
        doc_comment: Some(DocCommentRef {
            file_token: notice.file_token.clone(),
            file_type: notice.file_type.clone(),
            comment_id: notice.comment_id.clone(),
        }),
    };
    if dispatch_tx
        .send((msg, super::hub::Gate::Allow))
        .await
        .is_err()
    {
        debug!(channel = %channel_name, "dispatch closed, doc comment dropped");
    }
}

/// Fire a reaction on the triggering reply (fire-and-forget, best-effort;
/// needs a reply id to target).
fn fire_reaction(
    adapter: &Arc<dyn PlatformAdapter>,
    notice: &DocCommentNotice,
    emoji: &'static str,
) {
    let Some(reply_id) = notice.reply_id.clone() else {
        return;
    };
    let adapter = Arc::clone(adapter);
    let (file_token, file_type) = (notice.file_token.clone(), notice.file_type.clone());
    tokio::spawn(async move {
        if let Err(e) = adapter
            .react_doc_comment(&file_token, &file_type, &reply_id, emoji)
            .await
        {
            warn!(error = %e, reply_id = %reply_id, "doc comment reaction failed");
        }
    });
}

/// User-dimension access control for commenters (the chat-dimension rules
/// of `ChannelConfig::check_access` don't apply — a doc comment has no
/// chat). Blocklist wins over allowlist, mirroring the chat rules.
fn check_commenter_access(config: &ChannelConfig, commenter: &str) -> Result<(), ChannelError> {
    let denied = |reason| ChannelError::AccessDenied {
        chat_id: String::new(),
        user_id: commenter.to_string(),
        reason,
    };
    if config.blocked_users.iter().any(|u| u == commenter) {
        return Err(denied(super::AccessDeniedReason::BlockedUser));
    }
    if !config.allowed_users.is_empty() && !config.allowed_users.iter().any(|u| u == commenter) {
        return Err(denied(super::AccessDeniedReason::UserNotAllowed));
    }
    Ok(())
}

/// Assemble the injected user message: the meta header (same `[k: v]`
/// convention as chat messages) carrying the doc provenance, then the
/// quote line (partial comments) and the triggering reply's text.
fn assemble_message(
    notice: &DocCommentNotice,
    quote: Option<&str>,
    reply_text: &str,
    fetch_error: Option<&str>,
    title: Option<&str>,
) -> String {
    let ts = fmt_ts(notice.create_time);
    let mut header = format!(
        "[{ts}][from_user_id: {}][platform: feishu][doc: {}:{}][comment_id: {}]",
        notice.commenter_open_id, notice.file_type, notice.file_token, notice.comment_id
    );
    if let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) {
        let _ = write!(header, "[doc_title: {title}]");
    }
    if let Some(reply_id) = notice.reply_id.as_deref() {
        let _ = write!(header, "[reply_id: {reply_id}]");
    }

    let mut body = String::new();
    if let Some(quote) = quote.map(str::trim).filter(|q| !q.is_empty()) {
        let quote = crate::utils::strs::truncate_by_chars(
            &quote.split_whitespace().collect::<Vec<_>>().join(" "),
            QUOTE_MAX_CHARS,
            "…",
        );
        let _ = write!(body, "> {quote}\n\n");
    }
    body.push_str(reply_text);
    if let Some(error) = fetch_error {
        let _ = write!(body, "[评论内容拉取失败: {error}]");
    }
    format!("{header}\n{body}")
}

/// Fetch the comment, retrying briefly until the **triggering reply**
/// shows up: `batch_query` reads lag the event by up to a few seconds
/// (E2E-verified — without the retry we once injected the *previous*
/// reply's text). Degrading to the thread's latest reply after the
/// retries is better than dropping the trigger.
async fn fetch_detail_with_trigger(
    adapter: &dyn PlatformAdapter,
    notice: &DocCommentNotice,
) -> Result<Option<DocCommentDetail>, ChannelError> {
    const RETRY_DELAYS: &[std::time::Duration] = &[
        std::time::Duration::from_millis(500),
        std::time::Duration::from_secs(1),
        std::time::Duration::from_secs(2),
    ];
    let mut attempt = 0;
    loop {
        let detail = adapter
            .fetch_doc_comment(&notice.file_token, &notice.file_type, &notice.comment_id)
            .await;
        let found = notice.reply_id.as_deref().is_none_or(
            |rid| matches!(&detail, Ok(Some(d)) if d.replies.iter().any(|r| r.reply_id == rid)),
        );
        if found || attempt >= RETRY_DELAYS.len() {
            if !found {
                warn!(
                    comment_id = %notice.comment_id,
                    reply_id = notice.reply_id.as_deref().unwrap_or(""),
                    "triggering reply still not readable after retries, using latest"
                );
            }
            return detail;
        }
        tokio::time::sleep(RETRY_DELAYS[attempt]).await;
        attempt += 1;
    }
}

/// The comment thread's prior replies as a `<comment_thread_history>`
/// block — the doc-comment surface's "recent history". Deduped by the
/// per-thread history cursor (same store as chat); the bot's own replies
/// (already in the session as assistant turns) and the triggering reply
/// (delivered verbatim below) are excluded. The cursor advances to the
/// newest fetched reply at build time — best-effort, like chat. `None`
/// when there is nothing (left) to inject.
async fn build_thread_history(
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    notice: &DocCommentNotice,
    detail: &DocCommentDetail,
) -> Option<ContentBlock> {
    let container =
        super::doc_comment_mapping_key(&notice.file_type, &notice.file_token, &notice.comment_id);
    // Comment timestamps are seconds; cursors are stored in ms (chat
    // convention).
    let cursor = store
        .get_history_cursor(channel_name, &container)
        .await
        .ok()
        .flatten();
    if let Some(newest_ms) = detail.replies.iter().map(|r| r.create_time * 1000).max() {
        if cursor.is_none_or(|c| newest_ms > c) {
            if let Err(e) = store
                .set_history_cursor(channel_name, &container, newest_ms)
                .await
            {
                warn!(error = %e, "comment thread cursor advance failed");
            }
        }
    }
    let mut lines: Vec<String> = detail
        .replies
        .iter()
        .filter(|r| !r.is_from_bot)
        .filter(|r| Some(r.reply_id.as_str()) != notice.reply_id.as_deref())
        .filter(|r| !r.text.trim().is_empty())
        // Commands are control-plane (same rule as chat history).
        .filter(|r| !super::hub::is_command_text(r.text.trim()))
        .filter(|r| cursor.is_none_or(|c| r.create_time * 1000 > c))
        .map(|r| {
            let ts = chrono::DateTime::from_timestamp(r.create_time, 0)
                .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
                .unwrap_or_default();
            let text = crate::utils::strs::truncate_by_chars(
                r.text.trim(),
                THREAD_HISTORY_REPLY_MAX_CHARS,
                "…",
            );
            format!("[{ts}] {}: {text}", r.user_id)
        })
        .collect();
    if lines.len() > THREAD_HISTORY_MAX {
        lines.drain(..lines.len() - THREAD_HISTORY_MAX);
    }
    if lines.is_empty() {
        return None;
    }
    Some(ContentBlock::Text {
        text: format!(
            "<comment_thread_history>\n{}\n</comment_thread_history>",
            lines.join("\n")
        ),
    })
}

/// The comment thread reply that triggered this event: the one matching
/// the event's `reply_id`, else the thread's latest. Empty when the
/// thread fetched without replies (defensive — the API always returns at
/// least the first).
fn pick_triggering_reply(detail: &DocCommentDetail, reply_id: Option<&str>) -> String {
    reply_id
        .and_then(|id| detail.replies.iter().find(|r| r.reply_id == id))
        .or_else(|| detail.replies.last())
        .map(|r| r.text.clone())
        .unwrap_or_default()
}

/// `[YYYY-MM-DD HH:MM:SS]` local time from unix milliseconds (event
/// `header.create_time`); falls back to now when absent/invalid.
fn fmt_ts(create_time_ms: Option<i64>) -> String {
    let dt = create_time_ms
        .and_then(chrono::DateTime::from_timestamp_millis)
        .map_or_else(chrono::Local::now, |dt| dt.with_timezone(&chrono::Local));
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}

/// Split `text` into chunks of at most `max_chars` **chars** (UTF-8 safe),
/// preferring to break after a newline within the window. Empty input
/// yields no chunks.
pub(super) fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    // A zero budget would never advance — treat it as "no chunking".
    if max_chars == 0 {
        return if text.is_empty() {
            Vec::new()
        } else {
            vec![text.to_string()]
        };
    }
    let mut chunks = Vec::new();
    let mut rest = text;
    while !rest.is_empty() {
        let Some((hard_end, _)) = rest.char_indices().nth(max_chars) else {
            chunks.push(rest.to_string());
            break;
        };
        // Soft break: the last newline inside the window (its own char
        // rides the current chunk).
        let end = rest[..hard_end].rfind('\n').map_or(hard_end, |nl| nl + 1);
        chunks.push(rest[..end].to_string());
        rest = &rest[end..];
    }
    chunks
}

#[cfg(test)]
#[path = "comment_test.rs"]
mod tests;
