//! Feishu doc comments: policy filtering, content fetch, and assembly of
//! the injected user message whose meta header marks the doc provenance.
//! See `docs/design/feishu-doc-comment.md`.

use super::{
    ChannelConfig, ChannelError, ChannelMessage, DocCommentDetail, DocCommentNotice, DocCommentRef,
    PlatformAdapter,
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

/// Handle one `drive.notice.comment_add_v1` event: policy-filter, fetch
/// the comment content, assemble the user message and hand it to the
/// serial dispatch path as an allowed trigger. Filtered-out events are
/// logged and dropped. Runs off the gate loop (spawned) — the comment
/// fetch must not stall chat processing.
pub(super) async fn handle_doc_comment_added(
    channel_name: &str,
    config: &ChannelConfig,
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
        return;
    }

    let (detail, title) = tokio::join!(
        adapter.fetch_doc_comment(&notice.file_token, &notice.file_type, &notice.comment_id),
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
    let text = assemble_message(
        &notice,
        detail.as_ref().and_then(|d| d.quote.as_deref()),
        reply_text.as_deref().unwrap_or(""),
        fetch_error.as_deref(),
        title.as_deref(),
    );
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
        // The bare comment text with the @bot marker stripped: feeds the
        // session title (mirrors chat messages' `strip_bot_mention`).
        // Slash-leading comments are NOT commands (see `hub::message_command`).
        raw_text: reply_text
            .map(|t| t.replace("@bot", "").trim().to_string())
            .filter(|t| !t.is_empty()),
        content: vec![ContentBlock::Text { text }],
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
