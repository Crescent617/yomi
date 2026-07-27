//! Shared utilities for platform adapters.

/// Maximum retry delay for platform connection failures (shared across adapters).
#[allow(clippy::duration_suboptimal_units)]
pub(crate) const MAX_RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(300);

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
