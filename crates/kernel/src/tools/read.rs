use crate::tools::helper::{maybe_truncate_output, FileStateStore, MAX_FILE_SIZE};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::g_lock::{g_lock_timeout, DEFAULT_LOCK_TIMEOUT};
use crate::utils::image::{
    bytes_to_data_url, detect_mime_type, gif_first_frame_to_data_url, is_image_extension,
    probe_gif_info, MAX_IMAGE_SIZE,
};
use crate::utils::line_numbers::add_line_numbers;
use crate::utils::path::expand_tilde;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

pub const READ_TOOL_NAME: &str = "read";

#[derive(Default)]
pub struct ReadTool {
    file_state_store: Option<Arc<FileStateStore>>,
}

impl ReadTool {
    /// Create a new `ReadTool` with optional file state store
    pub fn new(store: impl Into<Option<Arc<FileStateStore>>>) -> Self {
        Self {
            file_state_store: store.into(),
        }
    }
}

impl ReadTool {
    /// Read an image file and return `ToolOutput` with image content.
    ///
    /// `strict` (extension-driven routing) reports undecodable data as an
    /// explicit unsupported error; non-strict (magic-sniff routing)
    /// returns `Ok(None)` so the caller can fall back to text/binary
    /// handling — magic bytes are only a heuristic (a text file can
    /// legitimately start with `GIF87a`).
    ///
    /// Animated GIFs are flattened to their first frame — vision APIs
    /// discard every later frame anyway, and inlining multi-MB animations
    /// bloats the request body into `HTTP 413` rejections.
    async fn read_image(
        &self,
        path: &Path,
        path_str: &str,
        strict: bool,
    ) -> Result<Option<ToolOutput>> {
        // Acquire lock before reading to coordinate with writers
        let _guard = g_lock_timeout(path.to_string_lossy(), DEFAULT_LOCK_TIMEOUT).await?;

        // Check file size — independent of exec's MAX_FILE_SIZE gate: the
        // two limits are equal today but can drift.
        let metadata = tokio::fs::metadata(path).await?;
        if metadata.len() > MAX_IMAGE_SIZE {
            return Ok(Some(ToolOutput::error(format!(
                "[Unsupported image: too large | {path_str} | Size: {} bytes | limit: {MAX_IMAGE_SIZE} bytes] Shrink or inspect it with shell tools instead.",
                metadata.len()
            ))));
        }

        let data = tokio::fs::read(path).await?;
        let size = data.len();

        // Strict routing reports the failure; sniffed routing falls back.
        let unsupported = |reason: &str| {
            if strict {
                Some(ToolOutput::error(format!(
                    "[Unsupported image: {path_str} | Size: {size} bytes] {reason} Inspect it with shell tools instead (e.g. `file`, `magick identify`)."
                )))
            } else {
                None
            }
        };

        let Some(mime) = detect_mime_type(&data) else {
            return Ok(unsupported("Unrecognized or corrupt image data."));
        };

        if mime == "image/gif" {
            let info = probe_gif_info(&data);
            // Flatten multi-frame GIFs — and structure-unreadable ones,
            // which can't be trusted to be small and static.
            if info.is_none_or(|i| i.frames > 1) {
                let url = match gif_first_frame_to_data_url(&data) {
                    Ok(url) => url,
                    Err(e) => return Ok(unsupported(&e.to_string())),
                };
                // Track file mtime if store is available
                if let Some(ref store) = self.file_state_store {
                    store.refresh(path).await;
                }
                let text = match info {
                    Some(info) => {
                        let secs = info.duration_ms;
                        format!(
                            "[Animated GIF: {}x{} | {} frames | {}.{}s | Size: {size} bytes — frame 1 shown. Extract more frames with shell tools, e.g. `magick '{path_str}[N]' frame.png` or `ffmpeg -i '{path_str}' -vframes 1 frame.png`]",
                            info.width,
                            info.height,
                            info.frames,
                            secs / 1000,
                            secs % 1000 / 100,
                        )
                    }
                    None => format!(
                        "[GIF: Size: {size} bytes | structure unreadable — frame 1 shown. Extract more frames with shell tools, e.g. `magick '{path_str}[N]' frame.png`]"
                    ),
                };
                return Ok(Some(ToolOutput::with_image_and_text(url, text)));
            }
        }

        let output = match bytes_to_data_url(&data) {
            Ok(data_url) => {
                let text = format!("[Image: {path_str} | Size: {size} bytes]");
                ToolOutput::with_image_and_text(data_url, text)
            }
            Err(e) => return Ok(unsupported(&e.to_string())),
        };

        // Track file mtime if store is available
        if let Some(ref store) = self.file_state_store {
            store.refresh(path).await;
        }
        Ok(Some(output))
    }

    /// Read a text file and return `ToolOutput` with text content
    async fn read_text(
        &self,
        path: &Path,
        offset: usize,
        limit: Option<usize>,
        line_numbers: bool,
        max_tool_output_length: usize,
    ) -> Result<ToolOutput> {
        // Acquire lock before reading to coordinate with writers
        let _guard = g_lock_timeout(path.to_string_lossy(), DEFAULT_LOCK_TIMEOUT).await?;

        let content = tokio::fs::read_to_string(path).await?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = offset.saturating_sub(1);
        if start >= total_lines {
            return Ok(ToolOutput::error(format!(
                "File has {total_lines} lines, offset {offset} is out of range"
            )));
        }

        let end = limit.map_or(total_lines, |l| start + l).min(total_lines);
        let text = lines[start..end].join("\n");

        let output = if line_numbers {
            add_line_numbers(&text, offset)
        } else {
            text
        };

        if let Some(ref store) = self.file_state_store {
            store.refresh(path).await;
        }

        let output = if end < total_lines {
            let notice = format!(
                "\n\n[Stopped at line {end} of {total_lines}. Use offset/limit to read more.]"
            );
            if output.len() + notice.len() <= max_tool_output_length {
                output + &notice
            } else {
                maybe_truncate_output(output, max_tool_output_length, offset)
            }
        } else {
            maybe_truncate_output(output, max_tool_output_length, offset)
        };

        Ok(ToolOutput::text(output))
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        READ_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Read a file from the local filesystem. Use this instead of cat/head/tail. Supports reading specific line ranges with offset and limit. Also supports reading image files (PNG, JPEG, GIF, WebP) which will be displayed as images; animated GIFs are flattened to their first frame (extract more frames with shell tools). Other binary files are unsupported - inspect them with shell tools instead."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file. Can be a text file or an image (png, jpg, jpeg, gif, webp)."
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based). Default: 1. Only applies to text files.",
                    "default": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read. Default: read all. Only applies to text files.",
                },
                "line_numbers": {
                    "type": "boolean",
                    "description": "Whether to include line numbers in the output. Default: false.",
                    "default": false
                }
            },
            "required": ["path"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'path' argument"))?;
        let offset = args["offset"].as_u64().map_or(1, |n| n as usize);
        let limit = args["limit"].as_u64().map(|n| n as usize);
        let line_numbers = args["line_numbers"].as_bool().unwrap_or(false);

        let path = expand_tilde(path_str);
        let path = if path.is_absolute() {
            path
        } else {
            ctx.working_dir.join(path)
        };

        tracing::debug!("Read: {}", path.display());

        // Check if file exists
        if !tokio::fs::try_exists(&path).await? {
            return Ok(ToolOutput::error(format!(
                "File does not exist: {path_str}"
            )));
        }

        let metadata = tokio::fs::metadata(&path).await?;

        // Allow only regular files to avoid blocking reads on special files
        // (character devices, FIFOs, sockets, directories, block devices, etc.).
        if !metadata.file_type().is_file() {
            return Ok(ToolOutput::error(format!(
                "Not a regular file: {path_str}. Only regular files can be read."
            )));
        }

        // Check file size
        let file_size = metadata.len();
        if file_size > MAX_FILE_SIZE {
            return Ok(ToolOutput::error(format!(
                "[Unsupported: file too large | {path_str} | Size: {file_size} bytes | limit: {MAX_FILE_SIZE} bytes] Shrink or inspect it with shell tools instead."
            )));
        }

        // Route by content, not extension alone: sniff the head for image
        // magic bytes, and refuse other binary content up front — reading
        // it as text would dump garbage into context.
        let head = read_sniff_head(&path).await?;
        let ext_is_image = is_image_extension(&path);
        if ext_is_image || detect_mime_type(&head).is_some() {
            // Sniffed (non-strict) routing is a heuristic: data that turns
            // out not to be a decodable image falls through to text/binary
            // handling instead of erroring.
            if let Some(output) = self.read_image(&path, path_str, ext_is_image).await? {
                return Ok(output);
            }
        }
        if looks_binary(&head) {
            Ok(ToolOutput::error(format!(
                "[Unsupported binary file: {path_str} | Size: {file_size} bytes] Inspect it with shell tools instead (e.g. `file`, `xxd`)."
            )))
        } else {
            self.read_text(
                &path,
                offset,
                limit,
                line_numbers,
                ctx.max_tool_output_length,
            )
            .await
        }
    }
}

/// Head bytes sniffed for content-based routing (image magic, NUL check).
const SNIFF_LEN: u64 = 8 * 1024;

async fn read_sniff_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt as _;
    let file = tokio::fs::File::open(path).await?;
    let mut buf = Vec::new();
    file.take(SNIFF_LEN).read_to_end(&mut buf).await?;
    Ok(buf)
}

/// Cheap binary heuristic: NUL bytes never appear in the UTF-8 text this
/// tool is meant for. (UTF-16 text already fails `read_to_string` today,
/// so flagging it here only improves the error message.)
fn looks_binary(data: &[u8]) -> bool {
    data.contains(&0)
}

#[cfg(test)]
#[path = "read_test.rs"]
mod tests;
