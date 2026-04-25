use crate::tools::base::{FileTool, MAX_FILE_SIZE};
use crate::tools::file_lock::{lock_shared_timeout, DEFAULT_LOCK_TIMEOUT};
use crate::tools::file_state::FileStateStore;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use crate::utils::image::{image_to_data_url, is_image_extension, MAX_IMAGE_SIZE};
use crate::utils::line_numbers::format_file_lines;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const READ_TOOL_NAME: &str = "read";

pub struct ReadTool {
    base_dir: PathBuf,
    file_state_store: Option<Arc<FileStateStore>>,
}

impl ReadTool {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
            file_state_store: None,
        }
    }

    /// Set the file state store for tracking reads
    #[must_use]
    pub fn with_file_state_store(mut self, store: Arc<FileStateStore>) -> Self {
        self.file_state_store = Some(store);
        self
    }

    /// Read an image file and return `ToolOutput` with image content
    async fn read_image(&self, path: &Path, path_str: &str) -> Result<ToolOutput> {
        // Acquire shared lock before reading to coordinate with writers
        let _guard = lock_shared_timeout(path, DEFAULT_LOCK_TIMEOUT)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire read lock: {e}"))?;

        // Check file size
        let metadata = tokio::fs::metadata(path).await?;
        if metadata.len() > MAX_IMAGE_SIZE {
            return Ok(ToolOutput::error(format!(
                "Image file too large: {} bytes (max: {MAX_IMAGE_SIZE})",
                metadata.len()
            )));
        }

        // Convert to data URL
        match image_to_data_url(path).await? {
            Some(data_url) => {
                // Track file mtime if store is available
                if let Some(ref store) = self.file_state_store {
                    let mtime = self.get_mtime(path);
                    store.record(path.to_path_buf(), mtime.await);
                }

                // Create output with image and metadata text
                let metadata_text =
                    format!("[Image: {} | Size: {} bytes]", path_str, metadata.len());
                Ok(ToolOutput::with_image_and_text(data_url, metadata_text))
            }
            None => Ok(ToolOutput::error(format!(
                "Failed to read image file: {path_str}"
            ))),
        }
    }

    /// Read a text file and return `ToolOutput` with text content
    async fn read_text(
        &self,
        path: &Path,
        path_str: &str,
        offset: usize,
        limit: Option<usize>,
    ) -> Result<ToolOutput> {
        // Acquire shared lock before reading to coordinate with writers
        let _guard = lock_shared_timeout(path, DEFAULT_LOCK_TIMEOUT)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire read lock: {e}"))?;

        let content = tokio::fs::read_to_string(path).await?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = offset.saturating_sub(1); // Convert to 0-based
        let end = limit.map_or(total_lines, |l| start + l).min(total_lines);

        if start >= total_lines {
            return Ok(ToolOutput::error(format!(
                "File has {total_lines} lines, offset {offset} is out of range"
            )));
        }

        let result_content = if start == 0 && end == total_lines {
            // Reading whole file
            content.clone()
        } else {
            // Reading partial content
            lines[start..end].join("\n")
        };

        // Add line numbers to the result
        let formatted_result = format_file_lines(&result_content, offset);

        // Track file mtime if store is available
        if let Some(ref store) = self.file_state_store {
            let mtime = self.get_mtime(path);
            store.record(path.to_path_buf(), mtime.await);
        }

        // Build response with file info
        let response = if start == 0 && end == total_lines {
            format!("{formatted_result}\n\n[File: {path_str} | Lines: {total_lines}]")
        } else {
            format!(
                "{formatted_result}\n\n[File: {path_str} | Lines: {offset}-{end} of {total_lines}]"
            )
        };

        Ok(ToolOutput::text(response))
    }
}

impl FileTool for ReadTool {
    fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        READ_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Read a file from the local filesystem. Use this instead of cat/head/tail. Supports reading specific line ranges with offset and limit. Also supports reading image files (PNG, JPEG, GIF, WebP) which will be displayed as images."
    }

    fn params(&self) -> Value {
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
                }
            },
            "required": ["path"]
        })
    }

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
        let offset = args["offset"].as_u64().map_or(1, |n| n as usize);
        let limit = args["limit"].as_u64().map(|n| n as usize);

        let path = self.resolve_path(path_str);

        tracing::debug!("Read: {}", path.display());

        // Check if file exists
        if !tokio::fs::try_exists(&path).await? {
            return Ok(ToolOutput::error(format!(
                "File does not exist: {path_str}"
            )));
        }

        // Check file size
        let file_size = tokio::fs::metadata(&path).await?.len();
        if file_size > MAX_FILE_SIZE {
            return Ok(ToolOutput::error(format!(
                "File is too large to read: {path_str}"
            )));
        }

        // Check if this is an image file
        if is_image_extension(&path) {
            self.read_image(&path, path_str).await
        } else {
            self.read_text(&path, path_str, offset, limit).await
        }
    }
}
