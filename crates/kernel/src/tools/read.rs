use crate::tools::file_state::FileStateStore;
use crate::tools::line_numbers::format_file_lines;
use crate::tools::Tool;
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024; // 10 MB

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

    fn resolve_path(&self, relative: &str) -> PathBuf {
        let path = self.base_dir.join(relative);

        path.canonicalize().unwrap_or(path)
    }

    /// Get file modification time in milliseconds since epoch
    async fn get_mtime(&self, path: &PathBuf) -> Result<u64> {
        let metadata = tokio::fs::metadata(path).await?;
        let mtime = metadata.modified()?;
        let duration = mtime
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        Ok(duration.as_millis() as u64)
    }
}

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn desc(&self) -> &'static str {
        "Read a file from the local filesystem. Use this instead of cat/head/tail. Supports reading specific line ranges with offset and limit."
    }

    fn params(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file"
                },
                "offset": {
                    "type": "integer",
                    "description": "Line number to start reading from (1-based). Default: 1",
                    "default": 1
                },
                "limit": {
                    "type": "integer",
                    "description": "Number of lines to read. Default: read all",
                }
            },
            "required": ["path"]
        })
    }

    async fn exec(&self, args: Value) -> Result<ToolOutput> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
        let offset = args["offset"].as_u64().map_or(1, |n| n as usize);
        let limit = args["limit"].as_u64().map(|n| n as usize);

        let path = self.resolve_path(path_str);

        tracing::debug!("Read: {}", path.display());

        // Check if file exists
        if !tokio::fs::try_exists(&path).await? {
            return Ok(ToolOutput::new_err(format!(
                "File does not exist: {path_str}"
            )));
        }
        if tokio::fs::metadata(&path).await?.len() > MAX_FILE_SIZE {
            return Ok(ToolOutput::new_err(format!(
                "File is too large to read: {path_str}"
            )));
        }

        let content = tokio::fs::read_to_string(&path).await?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = offset.saturating_sub(1); // Convert to 0-based
        let end = limit.map_or(total_lines, |l| start + l).min(total_lines);

        if start >= total_lines {
            return Ok(ToolOutput::new_err(format!(
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
            let mtime = self.get_mtime(&path).await.unwrap_or(0);
            store.record(path.clone(), mtime);
        }

        // Build response with file info
        let response = if start == 0 && end == total_lines {
            format!("{formatted_result}\n\n[File: {path_str} | Lines: {total_lines}]")
        } else {
            format!(
                "{formatted_result}\n\n[File: {path_str} | Lines: {offset}-{end} of {total_lines}]"
            )
        };

        Ok(ToolOutput::new(response, ""))
    }
}
