//! Attachment declarations in assistant texts (`<yomi_attachments>`).
//!
//! An agent attaches files to a reply with a `<yomi_attachments>` block,
//! one path per line (absolute, or relative to the session workspace).
//! A block counts as a declaration only when it stands outside a fenced
//! code block — decided by parity: the fences after a fenced-in block are
//! odd (its enclosing fence closes after it), while an even count (zero
//! included) means the block stands outside any fence. A fenced example
//! (e.g. the model showing the syntax to the user) therefore renders as
//! typed.
//!
//! Each chat surface strips recognized blocks at its own boundary and
//! presents the files its own way: channels deliver them via the platform
//! adapter (see `crate::channels::attachments`), the GUI renders clickable
//! attachment items under the message. Stored messages keep the raw text.

use std::path::{Path, PathBuf};

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
pub fn parse_attachments(text: &str) -> (String, Vec<String>) {
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

/// Resolve a relative path under `base`, rejecting path-traversal attempts.
///
/// Rejects absolute paths, `..` components, and paths that escape `base`.
/// Uses `tokio::fs::canonicalize` for async-safe symlink resolution.
pub async fn resolve_safe_path(base: &Path, path: &str) -> Option<PathBuf> {
    // Reject absolute paths and paths containing .. components.
    let path_obj = Path::new(path);
    if path_obj.is_absolute() {
        return None;
    }
    for comp in path_obj.components() {
        if matches!(comp, std::path::Component::ParentDir) {
            return None;
        }
    }
    let joined = base.join(path);
    match tokio::fs::canonicalize(&joined).await {
        Ok(canonical) => {
            let base_canonical = tokio::fs::canonicalize(base).await.ok()?;
            if canonical.starts_with(&base_canonical) {
                Some(canonical)
            } else {
                None
            }
        }
        Err(_) => {
            // File may not exist yet; verify logically within base.
            let base_canonical = tokio::fs::canonicalize(base).await.ok()?;
            let joined = base_canonical.join(path);
            if joined.starts_with(&base_canonical) {
                Some(joined)
            } else {
                None
            }
        }
    }
}

/// Resolve a declared attachment path to an existing file.
///
/// Absolute paths are taken as-is; relative paths must stay inside `base`
/// (the session workspace) — `..` components and symlink escapes are
/// rejected. Returns `None` when the path is unsafe, missing, or not a
/// regular file.
pub async fn resolve_attachment(base: Option<&Path>, path: &str) -> Option<PathBuf> {
    let candidate = if Path::new(path).is_absolute() {
        tokio::fs::canonicalize(path).await.ok()?
    } else {
        resolve_safe_path(base?, path).await?
    };
    let meta = tokio::fs::metadata(&candidate).await.ok()?;
    meta.is_file().then_some(candidate)
}

#[cfg(test)]
#[path = "attachments_test.rs"]
mod tests;
