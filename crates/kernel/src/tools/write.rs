use crate::tools::base::get_mtime;
use crate::tools::file_lock::{lock_exclusive_timeout, DEFAULT_LOCK_TIMEOUT};
use crate::tools::file_state::FileStateStore;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::AsyncWriteExt;

pub const WRITE_TOOL_NAME: &str = "write";

pub struct WriteTool {
    file_state_store: Option<Arc<FileStateStore>>,
}

impl Default for WriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl WriteTool {
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

    /// Check if the file has been modified since it was last read
    async fn check_staleness(&self, path: &Path) -> Result<(), String> {
        let store = self
            .file_state_store
            .as_ref()
            .ok_or("File state store not initialized")?;

        let current_mtime = get_mtime(path).await;
        store.check_staleness(path, current_mtime)
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
            .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' argument"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'content' argument"))?;
        let mode = args["mode"].as_str().unwrap_or("overwrite");
        let is_append = mode == "append";

        // Note: file_path is expected to be absolute from the agent
        // But we also support relative paths for convenience
        let path = if file_path_str.starts_with('/') {
            PathBuf::from(file_path_str)
        } else {
            ctx.working_dir.join(file_path_str)
        };

        tracing::debug!("Write: {} (mode: {})", path.display(), mode);

        // Check if file exists
        let file_exists = tokio::fs::try_exists(&path).await?;

        // If file exists and we have a file state store, check if it's been read
        // Skip staleness check for append mode (we're not overwriting)
        if file_exists {
            if let Some(ref store) = self.file_state_store {
                if !store.has_recorded(&path) {
                    return Ok(ToolOutput::error(format!(
                        "File has not been read yet. Read it first before writing: {file_path_str}"
                    )));
                }

                // Check for staleness
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

        // Write the file (with exclusive lock for existing files)
        let _original_content = if file_exists && !is_append {
            // For existing files in overwrite mode, acquire exclusive lock first, then read and write
            let _guard = lock_exclusive_timeout(&path, DEFAULT_LOCK_TIMEOUT)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to acquire write lock: {e}"))?;

            // Read original content for diff (while holding exclusive lock)
            let original = tokio::fs::read_to_string(&path).await.ok();

            // Write new content
            tokio::fs::write(&path, content).await?;

            original
        } else if is_append && file_exists {
            // Append mode: acquire lock, read original for diff, then append
            let _guard = lock_exclusive_timeout(&path, DEFAULT_LOCK_TIMEOUT)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to acquire write lock: {e}"))?;

            // Read original content for diff
            let original = tokio::fs::read_to_string(&path).await.ok();

            // Append content
            let mut file = tokio::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .await?;
            file.write_all(content.as_bytes()).await?;
            file.flush().await?;
            drop(file);

            original
        } else {
            // For new files, just write (no lock needed)
            tokio::fs::write(&path, content).await?;
            None
        };

        // Update file state store
        if let Some(ref store) = self.file_state_store {
            let mtime = get_mtime(&path).await;
            store.record(path.clone(), mtime);
        }

        // Build response
        let response = if is_append && file_exists {
            format!("File appended: {file_path_str}")
        } else if file_exists {
            format!("File updated: {file_path_str}")
        } else {
            format!("File created: {file_path_str}")
        };

        Ok(ToolOutput::text_with_summary(response, ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_write_tool_create_new() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let tool = WriteTool::new();
        let args = serde_json::json!({
            "file_path": "test.txt",
            "content": "Hello, World!"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("File created"));
        assert!(result.text_content().contains("test.txt"));

        // Verify file was created
        let content = tokio::fs::read_to_string(base_path.join("test.txt"))
            .await
            .unwrap();
        assert_eq!(content, "Hello, World!");
    }

    #[tokio::test]
    async fn test_write_tool_create_in_subdir() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let tool = WriteTool::new();
        let args = serde_json::json!({
            "file_path": "src/nested/test.rs",
            "content": "fn main() {}"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());

        // Verify file was created
        let content = tokio::fs::read_to_string(base_path.join("src/nested/test.rs"))
            .await
            .unwrap();
        assert_eq!(content, "fn main() {}");
    }

    #[tokio::test]
    async fn test_write_tool_update_without_read() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Create file first
        tokio::fs::write(base_path.join("existing.txt"), "original content")
            .await
            .unwrap();

        let store = Arc::new(FileStateStore::new());
        let tool = WriteTool::new().with_file_state_store(store);

        let args = serde_json::json!({
            "file_path": "existing.txt",
            "content": "new content"
        });

        // Should fail because file hasn't been read
        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.error_text().contains("not been read"));
    }

    #[tokio::test]
    async fn test_write_tool_update_after_read() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path().canonicalize().unwrap();

        // Create file first
        let file_path = base_path.join("existing.txt");
        tokio::fs::write(&file_path, "original content")
            .await
            .unwrap();

        let store = Arc::new(FileStateStore::new());

        // Record the file as read with the current mtime
        let mtime = crate::tools::base::get_mtime(&file_path).await;
        store.record(file_path.clone(), mtime);

        let tool = WriteTool::new().with_file_state_store(store);

        let args = serde_json::json!({
            "file_path": "existing.txt",
            "content": "new content"
        });

        // Should succeed because file was recorded as read
        let ctx = ToolExecCtx::new("test_tool_call", &base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.text_content().contains("File updated"));
        assert!(result.text_content().contains("existing.txt"));

        // Verify file was updated
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn test_write_tool_absolute_path() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let tool = WriteTool::new();
        let absolute_path = base_path.join("absolute.txt");
        let args = serde_json::json!({
            "file_path": absolute_path.to_str().unwrap(),
            "content": "absolute path content"
        });

        let ctx = ToolExecCtx::new("test_tool_call", base_path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());

        let content = tokio::fs::read_to_string(absolute_path).await.unwrap();
        assert_eq!(content, "absolute path content");
    }

    #[tokio::test]
    async fn test_write_then_edit_no_need_read() {
        let temp_dir = TempDir::new().unwrap();
        // Use non-canonicalized path to match real-world usage
        let base_path = temp_dir.path().to_path_buf();

        // Create a shared file state store
        let store = Arc::new(crate::tools::file_state::FileStateStore::new());

        // Create WriteTool with file state store
        let write_tool = WriteTool::new().with_file_state_store(Arc::clone(&store));

        // Write a new file
        let args = serde_json::json!({
            "file_path": "test.txt",
            "content": "Hello, World!"
        });
        let ctx = ToolExecCtx::new("test_tool_call", &base_path);
        let result = write_tool.exec(args, ctx).await.unwrap();
        assert!(result.success());

        // Now create EditTool with the same file state store
        let edit_tool = crate::tools::edit::EditTool::new().with_file_state_store(store);

        // Try to edit the file without reading first
        // This should succeed because WriteTool already recorded the file state
        let args = serde_json::json!({
            "path": "test.txt",
            "old_str": "Hello",
            "new_str": "Goodbye"
        });
        let ctx = ToolExecCtx::new("test_tool_call_2", &base_path);
        let result = edit_tool.exec(args, ctx).await.unwrap();

        // Should succeed, not fail with "not been read" error
        assert!(
            result.success(),
            "Edit after write should succeed without read first, but got: {}",
            result.error_text()
        );

        // Verify file was edited
        let content = tokio::fs::read_to_string(base_path.join("test.txt"))
            .await
            .unwrap();
        assert_eq!(content, "Goodbye, World!");
    }
}
