//! Attachment delivery for channel replies.
//!
//! The declaration syntax and path safety rules live in
//! [`crate::utils::attachments`]; this module handles the channel-specific
//! part: resolving the reply's declared paths up front (bad declarations
//! become reply notes, never silent) and delivering the files via the
//! platform adapter right after the reply. Images can instead ride the
//! reply card inline: [`inline_partition`] pre-uploads them for a platform
//! image handle, and the reply renderer embeds them below the body.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::warn;

use super::reply::{FinalReply, InlineImage};
use super::{PlatformAdapter, SessionRouting};
use crate::types::ContentBlock;
use crate::utils::attachments::resolve_attachment;

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

/// Pre-upload image attachments for inline rendering on the reply card:
/// each file the platform accepts (`Ok(Some(handle))`) leaves the
/// post-reply list and rides the reply instead; non-images
/// (`Ok(None)`), platforms without an upload API, and upload failures
/// stay on the post-reply list unchanged — the regular send path then
/// re-reports the failure reason. Sequential on purpose: attachment
/// counts are small, and the upload latency rides the reply either way.
pub(crate) async fn inline_partition(
    adapter: &Arc<dyn PlatformAdapter>,
    files: Vec<PathBuf>,
) -> (Vec<InlineImage>, Vec<PathBuf>) {
    let mut inline = Vec::new();
    let mut rest = Vec::new();
    for path in files {
        match adapter.upload_image(&path).await {
            Ok(Some(key)) => {
                let alt = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("image")
                    .to_string();
                inline.push(InlineImage { key, alt, path });
            }
            Ok(None) => rest.push(path),
            Err(e) => {
                warn!(error = %e, file = %path.display(), "inline image upload failed, keeping file delivery");
                rest.push(path);
            }
        }
    }
    (inline, rest)
}

/// Move the rideable share of `files` (images the platform accepts)
/// onto the reply as inline images; the rest stay on the post-reply
/// file path. A reply without body text has no card to ride — the
/// files list passes through untouched.
pub(crate) async fn inline_partition_for_reply(
    adapter: &Arc<dyn PlatformAdapter>,
    reply: &mut FinalReply,
    files: Vec<PathBuf>,
) -> Vec<PathBuf> {
    if files.is_empty() || reply.text().is_none() {
        return files;
    }
    let (inline, rest) = inline_partition(adapter, files).await;
    if !inline.is_empty() {
        reply.set_inline_images(inline);
    }
    rest
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
