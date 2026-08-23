//! Attachment declarations in assistant texts (`<yomi_attachments>`).
//!
//! An agent attaches files to a reply with a `<yomi_attachments>` block,
//! one path per line (absolute, or relative to the session workspace).
//! A block counts as a declaration only when it stands outside a fenced
//! code block (see `crate::utils::markdown`); a fenced example (e.g. the
//! model showing the syntax to the user) renders as typed.
//!
//! Each chat surface strips recognized blocks at its own boundary and
//! presents the files its own way: channels deliver them via the platform
//! adapter (see `crate::channels::attachments`), the GUI renders clickable
//! attachment items under the message. Stored messages keep the raw text.

use std::path::{Path, PathBuf};

const OPEN_TAG: &str = "<yomi_attachments>";
const CLOSE_TAG: &str = "</yomi_attachments>";

/// Strip every `<yomi_attachments>…</yomi_attachments>` block standing
/// outside a fenced code block, returning the cleaned text and the
/// declared paths (trimmed, non-empty, in document order).
///
/// Fenced examples and unterminated blocks are left in place: they should
/// surface to the user as typed, not vanish silently into a bogus
/// declaration.
pub fn parse_attachments(text: &str) -> (String, Vec<String>) {
    let mut paths = Vec::new();
    let mut removed = false;
    let cleaned = crate::utils::markdown::map_outside_fences(text, |run, out| {
        let mut rest = run;
        while let Some(open) = rest.find(OPEN_TAG) {
            let after_open = &rest[open + OPEN_TAG.len()..];
            let Some(close) = after_open.find(CLOSE_TAG) else {
                break;
            };
            out.push_str(&rest[..open]);
            for line in after_open[..close].lines() {
                let line = line.trim();
                if !line.is_empty() {
                    paths.push(line.to_string());
                }
            }
            removed = true;
            rest = &after_open[close + CLOSE_TAG.len()..];
        }
        out.push_str(rest);
    });
    if !removed {
        return (text.to_string(), paths);
    }
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

/// Like [`resolve_attachment`], but a missing (or empty) `base_dir` falls
/// back to the default workspace (`<data_dir>/workspace`) — sessions
/// without a stored working_dir (e.g. unbound channel sessions) resolve
/// relative paths there. 全系统唯一的缺省回落点；绝对路径行为与上相同
/// （as-is）。
pub async fn resolve_attachment_with_default_workspace(
    data_dir: &Path,
    base_dir: Option<&Path>,
    path: &str,
) -> Option<PathBuf> {
    let fallback;
    let base = match base_dir {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => {
            fallback = crate::utils::path::session_workspace_dir(data_dir, None);
            &fallback
        }
    };
    resolve_attachment(Some(base), path).await
}

#[cfg(test)]
#[path = "attachments_test.rs"]
mod tests;
