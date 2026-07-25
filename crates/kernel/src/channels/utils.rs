//! Shared utilities for platform adapters.

/// Maximum retry delay for platform connection failures (shared across adapters).
#[allow(clippy::duration_suboptimal_units)]
pub(crate) const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(300);

/// Resolve a relative path under `base`, rejecting path-traversal attempts.
///
/// Rejects absolute paths, `..` components, and paths that escape `base`.
/// Uses `tokio::fs::canonicalize` for async-safe symlink resolution.
pub async fn resolve_safe_path(base: &std::path::Path, path: &str) -> Option<std::path::PathBuf> {
    // Reject absolute paths and paths containing .. components.
    let path_obj = std::path::Path::new(path);
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

/// A file read and validated for platform upload (see [`read_upload`]).
pub(crate) struct UploadFile {
    pub bytes: Vec<u8>,
    pub file_name: String,
    pub is_image: bool,
}

/// Read `path` and validate it against platform upload caps, shared by the
/// platform adapters: empty uploads and oversize files are rejected by the
/// platforms with a generic error, so fail fast here with a precise reason.
/// `image_kind`/`file_kind` name the two classes in user-facing errors
/// (Feishu: image/file; Telegram: photo/document). The delivery file name
/// falls back to `file` when the path has no usable name.
pub(crate) async fn read_upload(
    path: &std::path::Path,
    image_max_bytes: usize,
    file_max_bytes: usize,
    image_kind: &'static str,
    file_kind: &'static str,
) -> Result<UploadFile, super::ChannelError> {
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|e| super::ChannelError::Platform(format!("read file: {e}")))?;
    let is_image = mime_guess::from_path(path).first_or_octet_stream().type_() == "image";
    let (limit, kind) = if is_image {
        (image_max_bytes, image_kind)
    } else {
        (file_max_bytes, file_kind)
    };
    if bytes.is_empty() {
        return Err(super::ChannelError::Platform(format!("empty {kind}")));
    }
    if bytes.len() > limit {
        return Err(super::ChannelError::Platform(format!(
            "{kind} exceeds the {}MB platform limit",
            limit / 1024 / 1024
        )));
    }
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("file")
        .to_string();
    Ok(UploadFile {
        bytes,
        file_name,
        is_image,
    })
}

#[cfg(test)]
#[path = "utils_test.rs"]
mod tests;
