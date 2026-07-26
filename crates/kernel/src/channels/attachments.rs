//! Attachment declarations in channel replies.
//!
//! A channel-routed agent attaches files to its reply with a
//! `<yomi_attachments>` block, one path per line (absolute, or relative
//! to the session workspace). The tag is project-specific, and a block
//! counts as a declaration only when it stands outside a fenced code
//! block — decided by parity: the fences after a fenced-in block are odd
//! (its enclosing fence closes after it), while an even count (zero
//! included) means the block stands outside any fence. A fenced example
//! (e.g. the model showing the syntax to the user) therefore renders as
//! typed. Recognized blocks are stripped when the text enters the reply
//! buffer (see [`super::reply`]), so a declaration never renders on the
//! platform; the hub resolves the declared paths up front
//! ([`resolve_attachments`]) and delivers the files right after the
//! reply ([`send_attachments`]).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::warn;

use super::reply::FinalReply;
use super::utils::resolve_safe_path;
use super::{PlatformAdapter, SessionRouting};
use crate::types::ContentBlock;

const OPEN_TAG: &str = "<yomi_attachments>";
const CLOSE_TAG: &str = "</yomi_attachments>";
const FENCE: &str = "```";

/// Strip every `<yomi_attachments>…</yomi_attachments>` block standing
/// outside a fenced code block, returning the cleaned text and the
/// declared paths (trimmed, non-empty, in document order).
///
/// Fence membership is decided by parity: the fences remaining after a
/// fenced-in block are odd (its enclosing fence closes after it), so an
/// even count — zero included — means the block stands outside any fence.
/// Fenced examples and unterminated blocks are left in place: they should
/// surface to the user as typed, not vanish silently into a bogus
/// declaration.
pub(crate) fn parse_attachments(text: &str) -> (String, Vec<String>) {
    let mut paths = Vec::new();
    let mut cleaned = String::with_capacity(text.len());
    let mut removed = false;
    let mut rest = text;
    while let Some(open) = rest.find(OPEN_TAG) {
        let after_open = &rest[open + OPEN_TAG.len()..];
        let Some(close) = after_open.find(CLOSE_TAG) else {
            break;
        };
        let block_end = open + OPEN_TAG.len() + close + CLOSE_TAG.len();
        if rest[block_end..].matches(FENCE).count() % 2 != 0 {
            // Fenced example: keep it verbatim, keep scanning after it.
            cleaned.push_str(&rest[..block_end]);
            rest = &rest[block_end..];
            continue;
        }
        cleaned.push_str(&rest[..open]);
        for line in after_open[..close].lines() {
            let line = line.trim();
            if !line.is_empty() {
                paths.push(line.to_string());
            }
        }
        removed = true;
        rest = &rest[block_end..];
    }
    if !removed {
        return (text.to_string(), paths);
    }
    cleaned.push_str(rest);
    (cleaned.trim().to_string(), paths)
}

/// Resolve a declared attachment path to an existing file.
///
/// Absolute paths are taken as-is; relative paths must stay inside `base`
/// (the session workspace) — `..` components and symlink escapes are
/// rejected. Returns `None` when the path is unsafe, missing, or not a
/// regular file.
pub(crate) async fn resolve_attachment(base: Option<&Path>, path: &str) -> Option<PathBuf> {
    let candidate = if Path::new(path).is_absolute() {
        tokio::fs::canonicalize(path).await.ok()?
    } else {
        resolve_safe_path(base?, path).await?
    };
    let meta = tokio::fs::metadata(&candidate).await.ok()?;
    meta.is_file().then_some(candidate)
}

/// Resolve the reply's declared attachments to existing files, consuming
/// the declaration list. Unresolvable paths are appended to the reply text
/// as notes — a bad declaration never vanishes silently. The files
/// themselves are sent later via [`send_attachments`], after the reply.
pub(crate) async fn resolve_attachments(
    cwd: Option<&Path>,
    reply: &mut FinalReply,
) -> Vec<PathBuf> {
    let declared = reply.take_attachments();
    let mut paths: Vec<PathBuf> = Vec::new();
    for path in declared {
        match resolve_attachment(cwd, &path).await {
            // Dedupe declarations resolving to the same file.
            Some(p) if paths.contains(&p) => {}
            Some(p) => paths.push(p),
            None => reply.push_note(&format!(
                "⚠️ attachment skipped: `{path}` (missing, not a file, or outside the workspace)"
            )),
        }
    }
    paths
}

/// Send resolved attachment files. A platform failure surfaces as a short
/// follow-up message so it never vanishes silently.
pub(crate) async fn send_attachments(
    adapter: &Arc<dyn PlatformAdapter>,
    routing: &SessionRouting,
    files: Vec<PathBuf>,
) {
    if files.is_empty() {
        return;
    }
    let refs: Vec<(&Path, Option<&str>)> = files.iter().map(|p| (p.as_path(), None)).collect();
    if let Err(e) = adapter
        .send_files(
            &routing.external_chat_id,
            &refs,
            routing.reply_msg_id.as_deref(),
        )
        .await
    {
        warn!(error = %e, "failed to send attachment files");
        // Platform errors carry per-file reasons (empty, oversize, …) —
        // show them bare; other variants keep the full Display.
        let text = match &e {
            super::ChannelError::Platform(msg) => format!("⚠️ {msg}"),
            _ => format!("⚠️ failed to send attachment(s): {e}"),
        };
        let _ = adapter
            .send_message(
                &routing.external_chat_id,
                vec![ContentBlock::Text { text }],
                routing.reply_msg_id.as_deref(),
            )
            .await;
    }
}

#[cfg(test)]
#[path = "attachments_test.rs"]
mod tests;
