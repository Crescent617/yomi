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
//!
//! The channel reply path additionally anchors **images**: the block's
//! position in the text is where each image renders on the reply card —
//! [`parse_attachments_anchored`] leaves a placeholder token per image
//! path at the block's spot (non-image paths leave none — files cannot
//! ride a card), and the card renderer splits the body there. The GUI
//! keeps the plain strip ([`parse_attachments`]); the declaration rules
//! are identical and mirrored by its TS port.

use std::path::{Path, PathBuf};

const OPEN_TAG: &str = "<yomi_attachments>";
const CLOSE_TAG: &str = "</yomi_attachments>";

/// Placeholder token marking an inline image position in a reply text:
/// the spot where an attachments block declared it. The token carries
/// the declared path, so it survives every later text transformation
/// (body-text promotion, truncation, mention rewrite) without any
/// bookkeeping — only the two render endpoints (card / plain) need to
/// understand it.
pub const ATTACHMENT_TOKEN_PREFIX: &str = "⟦yomi-attachment:";
/// Token terminator (the prefix is ASCII; the path is free-form up to
/// this bracket).
pub const ATTACHMENT_TOKEN_SUFFIX: char = '⟧';

/// The placeholder token for one declared path.
pub fn attachment_token(path: &str) -> String {
    format!("{ATTACHMENT_TOKEN_PREFIX}{path}{ATTACHMENT_TOKEN_SUFFIX}")
}

/// One text piece after splitting at attachment tokens: plain text, or
/// an anchor carrying its declared path.
pub(crate) enum TokenSegment<'a> {
    Text(&'a str),
    Anchor(&'a str),
}

/// Split `text` at every complete attachment token, in document order —
/// the single scanner behind both the strip (plain surfaces) and the
/// card's anchored split.
pub(crate) fn token_segments(text: &str) -> Vec<TokenSegment<'_>> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(ATTACHMENT_TOKEN_PREFIX) {
        let after = &rest[start + ATTACHMENT_TOKEN_PREFIX.len()..];
        let Some(end) = after.find(ATTACHMENT_TOKEN_SUFFIX) else {
            break;
        };
        out.push(TokenSegment::Text(&rest[..start]));
        out.push(TokenSegment::Anchor(&after[..end]));
        rest = &after[end + ATTACHMENT_TOKEN_SUFFIX.len_utf8()..];
    }
    out.push(TokenSegment::Text(rest));
    out
}

/// Whether a path names an image (mime guess on the extension — the
/// same rule the platform upload path applies to the file itself).
pub fn is_image_name(path: impl AsRef<std::path::Path>) -> bool {
    mime_guess::from_path(path).first_or_octet_stream().type_() == "image"
}

/// Anchored variant of [`parse_attachments`] for the channel reply path:
/// identical declaration rules (fenced/unterminated blocks untouched,
/// paths collected in document order), but each **image** path also
/// leaves an [`attachment_token`] at the block's position in the cleaned
/// text — the card renderer later splits the body there and inserts the
/// image element. Non-image paths leave no token: they cannot ride the
/// card and always go the post-reply file route.
///
/// Two mint-time guards: pre-existing token literals in the source text
/// (the model quoting the pattern in prose) are neutralized by a space
/// after the bracket — same glyphs, dead pattern, never read as a live
/// anchor downstream; and a declared path containing a bracket never
/// mints a token (it would terminate early — the image falls back to
/// the after-body append instead).
pub fn parse_attachments_anchored(text: &str) -> (String, Vec<String>) {
    let text = &text.replace(ATTACHMENT_TOKEN_PREFIX, "⟦ yomi-attachment:");
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
                if line.is_empty() {
                    continue;
                }
                if is_image_name(line) && !line.contains(['⟦', '⟧']) {
                    out.push_str(&attachment_token(line));
                }
                paths.push(line.to_string());
            }
            removed = true;
            rest = &after_open[close + CLOSE_TAG.len()..];
        }
        out.push_str(rest);
    });
    if !removed {
        return (text.clone(), paths);
    }
    (cleaned.trim().to_string(), paths)
}

/// Strip every complete attachment token from `text` — trace snippets,
/// process-panel narrations, and plain-text surfaces never render them.
pub fn strip_attachment_tokens(text: &str) -> String {
    token_segments(text)
        .into_iter()
        .filter_map(|seg| match seg {
            TokenSegment::Text(t) => Some(t),
            TokenSegment::Anchor(_) => None,
        })
        .collect()
}

/// Cut a dangling token head at the end of `text` and append `suffix` —
/// a body-text truncation that landed inside a token (anywhere,
/// including inside the prefix itself) leaves an unterminated `⟦…`;
/// the partial token must not reach the card (the image it named falls
/// back to the appended-after-body position). Keyed on the bracket
/// alone: `⟦` is the token's private charset (pre-existing literals are
/// neutralized at mint time — a truncation-split literal loses only its
/// partial head, same as a token). Returns whether a cut was made.
pub fn strip_dangling_token(text: &mut String, suffix: &str) -> bool {
    let Some(pos) = text.rfind('⟦') else {
        return false;
    };
    if text[pos..].contains(ATTACHMENT_TOKEN_SUFFIX) {
        return false;
    }
    text.truncate(pos);
    text.push_str(suffix);
    true
}

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
