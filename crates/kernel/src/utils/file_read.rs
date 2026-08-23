//! Unified daemon-side file reads for the wire protocol.
//!
//! Two file namespaces, one chunked reader:
//!
//! - [`FileSource::Asset`] — an `asset://{hash}.{ext}` URL under the
//!   daemon's data directory (tool/user images).
//! - [`FileSource::Attachment`] — a path declared in a `<yomi_attachments>`
//!   block, resolved with the same safety rules as channel delivery
//!   (`crate::utils::attachments::resolve_attachment`).
//!
//! Both resolve on the daemon's host, so GUI/TUI clients work identically
//! in local and remote mode. Bytes travel base64-encoded inside
//! [`FileBytes`] chunks sized to fit the wire frame limit
//! (`crate::transport::MAX_FRAME_SIZE`).

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

/// Default chunk size: 2 MiB raw ≈ 2.8 MiB base64 — well inside the
/// 8 MiB wire frame limit, with room for the envelope.
pub const DEFAULT_CHUNK_BYTES: u64 = 2 * 1024 * 1024;
/// Largest chunk a client may request (4 MiB raw ≈ 5.6 MiB on the wire).
pub const MAX_CHUNK_BYTES: u64 = 4 * 1024 * 1024;

/// Which daemon-side file a client wants to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileSource {
    /// An `asset://` URL under the daemon's data directory.
    Asset { url: String },
    /// A declared attachment path. Relative paths must stay inside
    /// `base_dir` (the session workspace); absolute paths are taken as-is.
    /// `base_dir` 缺省按 `<data_dir>/workspace` 解析（共享规则见
    /// `attachments::resolve_attachment_with_default_workspace`）。
    Attachment {
        base_dir: Option<String>,
        path: String,
    },
}

/// A byte range of a daemon-side file, base64-encoded for JSON transport.
///
/// `mime`/`file_size`/`mtime_ms` describe the whole file and are present
/// in every chunk, so a `limit = 0` request doubles as a stat call.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FileBytes {
    pub data_base64: String,
    pub mime: String,
    pub file_size: u64,
    pub start_offset: u64,
    pub end_offset: u64,
    /// Last-modified time in milliseconds since the Unix epoch (0 if the
    /// platform does not provide one). Lets clients key caches by content
    /// identity without a second round trip.
    pub mtime_ms: u64,
}

impl FileSource {
    /// Human-readable reference for error messages.
    fn display(&self) -> String {
        match self {
            FileSource::Asset { url } => url.clone(),
            FileSource::Attachment { path, .. } => path.clone(),
        }
    }
}

/// Resolve `source` to an existing regular file on the daemon's host.
async fn resolve(source: &FileSource, data_dir: &Path) -> Option<PathBuf> {
    match source {
        FileSource::Asset { url } => {
            crate::utils::asset::asset_path(url, data_dir).filter(|path| path.is_file())
        }
        FileSource::Attachment { base_dir, path } => {
            crate::utils::attachments::resolve_attachment_with_default_workspace(
                data_dir,
                base_dir.as_deref().map(Path::new),
                path,
            )
            .await
        }
    }
}

/// Read a byte range of the file `source` refers to.
///
/// `offset` defaults to 0; `limit` defaults to [`DEFAULT_CHUNK_BYTES`] and
/// is capped at [`MAX_CHUNK_BYTES`]. `limit = Some(0)` returns metadata
/// only (empty `data_base64`). An `offset` past the end of the file yields
/// an empty chunk at `file_size`.
pub async fn read_file(
    source: &FileSource,
    data_dir: &Path,
    offset: Option<u64>,
    limit: Option<u64>,
) -> crate::types::Result<FileBytes> {
    use crate::types::KernelError;
    use base64::Engine as _;

    let path = resolve(source, data_dir).await.ok_or_else(|| {
        KernelError::config(format!(
            "file unavailable: {} (missing, not a file, or outside the workspace)",
            source.display()
        ))
    })?;

    let meta = tokio::fs::metadata(&path)
        .await
        .map_err(|e| KernelError::io(format!("stat {}: {e}", source.display())))?;
    let file_size = meta.len();
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis() as u64);

    let limit = limit.unwrap_or(DEFAULT_CHUNK_BYTES).min(MAX_CHUNK_BYTES);
    let start = offset.unwrap_or(0).min(file_size);
    let end = start.saturating_add(limit).min(file_size);

    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|e| KernelError::io(format!("open {}: {e}", source.display())))?;
    file.seek(std::io::SeekFrom::Start(start))
        .await
        .map_err(KernelError::from)?;
    let mut bytes = vec![0u8; (end - start) as usize];
    file.read_exact(&mut bytes)
        .await
        .map_err(|e| KernelError::io(format!("read {}: {e}", source.display())))?;

    Ok(FileBytes {
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        mime: mime_guess::from_path(&path)
            .first_or_octet_stream()
            .essence_str()
            .to_string(),
        file_size,
        start_offset: start,
        end_offset: end,
        mtime_ms,
    })
}

#[cfg(test)]
#[path = "file_read_test.rs"]
mod tests;
