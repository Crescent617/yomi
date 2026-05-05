use crate::tools::helper::{
    get_mtime, lock_shared_timeout, FileStateStore, DEFAULT_LOCK_TIMEOUT, MAX_FILE_SIZE,
    MAX_TOOL_OUTPUT_LENGTH,
};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::image::{image_to_data_url, is_image_extension, MAX_IMAGE_SIZE};
use crate::utils::line_numbers::add_line_numbers;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

pub const READ_TOOL_NAME: &str = "read";

pub struct ReadTool {
    file_state_store: Option<Arc<FileStateStore>>,
}

impl Default for ReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl ReadTool {
    pub fn new() -> Self {
        Self {
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
            .map_err(|e| KernelError::tool(format!("Failed to acquire read lock: {e}")))?;

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
                    let mtime = get_mtime(path).await;
                    store.record(path.to_path_buf(), mtime).await;
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
        offset: usize,
        limit: Option<usize>,
        line_numbers: bool,
    ) -> Result<ToolOutput> {
        let _guard = lock_shared_timeout(path, DEFAULT_LOCK_TIMEOUT)
            .await
            .map_err(|e| KernelError::tool(format!("Failed to acquire read lock for text: {e}")))?;

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
            store
                .record(path.to_path_buf(), get_mtime(path).await)
                .await;
        }

        Ok(ToolOutput::text(maybe_truncate(output, offset)))
    }
}

/// Truncate text if it exceeds max length, adding a notice with the line number.
fn maybe_truncate(mut text: String, offset: usize) -> String {
    if text.len() <= MAX_TOOL_OUTPUT_LENGTH {
        return text;
    }

    // Truncate at a safe UTF-8 boundary near the limit
    let truncate_at = find_utf8_boundary(&text, MAX_TOOL_OUTPUT_LENGTH);
    text.truncate(truncate_at);

    // Calculate line number at truncation point
    let lines_count = text.lines().count();
    let truncation_line = offset + lines_count.saturating_sub(1);

    let notice = format!(
        "\n\n[Content truncated at line {truncation_line}. Use offset/limit to read more.]"
    );
    text.push_str(&notice);
    text
}

/// Find a valid UTF-8 boundary at or before the target byte position.
fn find_utf8_boundary(text: &str, target: usize) -> usize {
    text.char_indices()
        .rev()
        .find(|&(i, _)| i <= target)
        .map_or(0, |(i, _)| i)
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        READ_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Read a file from the local filesystem. Use this instead of cat/head/tail. Supports reading specific line ranges with offset and limit. Also supports reading image files (PNG, JPEG, GIF, WebP) which will be displayed as images."
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

        let path = ctx.working_dir.join(path_str);

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
            self.read_text(&path, offset, limit, line_numbers).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_read_basic() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Create test file
        tokio::fs::write(base_path.join("test.txt"), "Hello, World!")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let args = serde_json::json!({"path": "test.txt"});

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        assert!(result.text_content().contains("Hello, World!"));
    }

    #[tokio::test]
    async fn test_read_with_offset() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join("test.txt"), "line1\nline2\nline3")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let args = serde_json::json!({"path": "test.txt", "offset": 2});

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        let content = result.text_content();
        assert!(!content.contains("line1"));
        assert!(content.contains("line2"));
        assert!(content.contains("line3"));
    }

    #[tokio::test]
    async fn test_read_with_limit() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join("test.txt"), "line1\nline2\nline3")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let args = serde_json::json!({"path": "test.txt", "limit": 2});

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        let content = result.text_content();
        assert!(content.contains("line1"));
        assert!(content.contains("line2"));
        assert!(!content.contains("line3"));
    }

    #[tokio::test]
    async fn test_read_with_offset_and_limit() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join("test.txt"), "a\nb\nc\nd\ne")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let args = serde_json::json!({"path": "test.txt", "offset": 2, "limit": 2});

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        let content = result.text_content();
        assert!(content.contains('b'));
        assert!(content.contains('c'));
        assert!(!content.contains('a'));
        assert!(!content.contains('d'));
    }

    #[tokio::test]
    async fn test_read_with_line_numbers() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join("test.txt"), "line1\nline2")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let args = serde_json::json!({"path": "test.txt", "line_numbers": true});

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        let content = result.text_content();
        assert!(content.contains("1\tline1"));
        assert!(content.contains("2\tline2"));
    }

    #[tokio::test]
    async fn test_read_offset_with_line_numbers() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join("test.txt"), "a\nb\nc")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let args = serde_json::json!({"path": "test.txt", "offset": 2, "line_numbers": true});

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        let content = result.text_content();
        // Line numbers should start from offset
        assert!(content.contains("2\tb"));
        assert!(content.contains("3\tc"));
        assert!(!content.contains("1\ta"));
    }

    #[tokio::test]
    async fn test_read_file_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let tool = ReadTool::new();
        let args = serde_json::json!({"path": "nonexistent.txt"});

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.is_error);
        assert!(result.error_text().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_read_offset_out_of_range() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        tokio::fs::write(base_path.join("test.txt"), "line1\nline2")
            .await
            .unwrap();

        let tool = ReadTool::new();
        let args = serde_json::json!({"path": "test.txt", "offset": 10});

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.is_error);
        assert!(result.error_text().contains("out of range"));
    }

    #[tokio::test]
    async fn test_read_truncation() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Create a large file that will trigger truncation
        // Each line is about 100 chars, create enough lines to exceed limit
        let line = "x".repeat(100);
        let lines_needed = MAX_TOOL_OUTPUT_LENGTH / 100 + 10;
        let mut content = String::with_capacity(line.len() * lines_needed + lines_needed);
        for _ in 0..lines_needed {
            content.push_str(&line);
            content.push('\n');
        }
        tokio::fs::write(base_path.join("large.txt"), content)
            .await
            .unwrap();

        let tool = ReadTool::new();
        let args = serde_json::json!({"path": "large.txt"});

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        let text = result.text_content();
        // Should contain truncation notice
        assert!(text.contains("Content truncated"));
        // Should indicate line number where truncated
        assert!(text.contains("at line"));
        // Length should be close to limit (allowing for truncation notice overhead)
        assert!(text.len() <= MAX_TOOL_OUTPUT_LENGTH + 100);
    }
}
