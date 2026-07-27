//! Attachment delivery for channel replies.
//!
//! The declaration syntax and path safety rules live in
//! [`crate::utils::attachments`]; this module handles the channel-specific
//! part: resolving the reply's declared paths up front (bad declarations
//! become reply notes, never silent) and delivering the files via the
//! platform adapter right after the reply.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tracing::warn;

use super::reply::FinalReply;
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
