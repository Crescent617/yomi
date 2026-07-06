use crate::tools::helper::{g_lock_timeout, get_mtime, FileStateStore, DEFAULT_LOCK_TIMEOUT};
use crate::tools::{FileStateAwareTool, Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::path::expand_tilde;
use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

pub const WRITE_TOOL_NAME: &str = "write";

#[derive(Default)]
pub struct WriteTool {
    file_state_store: Option<Arc<FileStateStore>>,
}

impl WriteTool {
    /// Create a new `WriteTool` with optional file state store
    pub fn new(store: impl Into<Option<Arc<FileStateStore>>>) -> Self {
        Self {
            file_state_store: store.into(),
        }
    }
}

impl FileStateAwareTool for WriteTool {
    fn file_state_store(&self) -> Option<&Arc<FileStateStore>> {
        self.file_state_store.as_ref()
    }
}

#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        WRITE_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Write a file to the local filesystem. Overwrites/append existing files or creates new ones."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "Relative to the working directory or absolute path"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                },
                "mode": {
                    "type": "string",
                    "description": "Write mode: 'overwrite' (default) or 'append'",
                    "enum": ["overwrite", "append"],
                    "default": "overwrite"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let file_path_str = args["file_path"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'file_path' argument"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| KernelError::tool("Missing 'content' argument"))?;
        let mode = args["mode"].as_str().unwrap_or("overwrite");
        let is_append = mode == "append";

        // Note: file_path is expected to be absolute from the agent
        // But we also support relative paths for convenience
        let path = expand_tilde(file_path_str);
        let path = if path.is_absolute() {
            path
        } else {
            ctx.working_dir.join(path)
        };

        tracing::debug!("Write: {} (mode: {})", path.display(), mode);

        // Check if file exists
        let file_exists = tokio::fs::try_exists(&path).await?;

        // Check staleness for existing files (skip for append mode)
        if file_exists && !is_append {
            if let Some(ref store) = self.file_state_store {
                if !store.has_recorded(&path) {
                    return Ok(ToolOutput::error(format!(
                        "File has not been read yet. Read it first before writing: {file_path_str}"
                    )));
                }
                if let Err(error) = self.check_staleness(&path).await {
                    return Ok(ToolOutput::error(error));
                }
            }
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !tokio::fs::try_exists(parent).await? {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        // Determine operation type for response message
        let op = match (file_exists, is_append) {
            (false, _) => "created",
            (true, true) => "appended",
            (true, false) => "updated",
        };

        // Track file for checkpoint before modification
        // Write file: acquire lock to serialize concurrent tool calls
        let _guard = g_lock_timeout(path.to_string_lossy(), DEFAULT_LOCK_TIMEOUT).await?;

        // Track file for checkpoint before modification (under lock to avoid stale backup)
        ctx.track_edit(&path).await;

        if is_append {
            let mut file = tokio::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .await?;
            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
        } else {
            tokio::fs::write(&path, content).await?;
        }

        // Update file state store
        if let Some(ref store) = self.file_state_store {
            if let Some(mtime) = get_mtime(&path).await {
                store.record(path.clone(), mtime).await;
            }
        }

        Ok(ToolOutput::text_with_summary(
            format!("File {op}: {file_path_str}"),
            "",
        ))
    }
}

#[cfg(test)]
#[path = "write_test.rs"]
mod tests;
