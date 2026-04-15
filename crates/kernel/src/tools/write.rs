use crate::tools::base::FileTool;
use crate::tools::edit_utils::generate_diff;
use crate::tools::file_state::FileStateStore;
use crate::tools::line_numbers::format_file_lines;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const WRITE_TOOL_NAME: &str = "write";

pub struct WriteTool {
    base_dir: PathBuf,
    file_state_store: Option<Arc<FileStateStore>>,
}

impl WriteTool {
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

    /// Check if the file has been modified since it was last read
    async fn check_staleness(&self, path: &PathBuf) -> Option<String> {
        let store = self.file_state_store.as_ref()?;

        // Check if file has been modified (mtime changed)
        let current_mtime = self.get_mtime(path).await;
        if store.is_stale(path, current_mtime) {
            return Some(
                "File has been modified since it was read. Read the file again before writing."
                    .to_string(),
            );
        }

        None
    }
}

impl FileTool for WriteTool {
    fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}
#[async_trait]
impl Tool for WriteTool {
    fn name(&self) -> &'static str {
        WRITE_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Write a file to the local filesystem. Completely overwrites existing files or creates new ones. Must read the file first if it exists."
    }

    fn params(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to write (must be absolute, not relative)"
                },
                "content": {
                    "type": "string",
                    "description": "The content to write to the file"
                }
            },
            "required": ["file_path", "content"]
        })
    }

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let file_path_str = args["file_path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'file_path' argument"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'content' argument"))?;

        // Note: file_path is expected to be absolute from the agent
        // But we also support relative paths for convenience
        let path = if file_path_str.starts_with('/') {
            PathBuf::from(file_path_str)
        } else {
            self.resolve_path(file_path_str)
        };

        tracing::debug!("Write: {}", path.display());

        // Check if file exists
        let file_exists = tokio::fs::try_exists(&path).await?;

        // If file exists and we have a file state store, check if it's been read
        if file_exists {
            if let Some(ref store) = self.file_state_store {
                if !store.has_recorded(&path) {
                    return Ok(ToolOutput::new_err(format!(
                        "File has not been read yet. Read it first before writing: {file_path_str}"
                    )));
                }

                // Check for staleness
                if let Some(error) = self.check_staleness(&path).await {
                    return Ok(ToolOutput::new_err(error));
                }
            }
        }

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() && !tokio::fs::try_exists(parent).await? {
                tokio::fs::create_dir_all(parent).await?;
            }
        }

        // Read original content for diff if file exists
        let original_content = if file_exists {
            tokio::fs::read_to_string(&path).await.ok()
        } else {
            None
        };

        // Write the file
        tokio::fs::write(&path, content).await?;

        // Update file state store
        if let Some(ref store) = self.file_state_store {
            let mtime = self.get_mtime(&path);
            store.record(path.clone(), mtime.await);
        }

        // Build response
        let response = if let Some(ref old_content) = original_content {
            // File was updated - show diff
            let diff = generate_diff(old_content, content, 3);
            format!(
                "File updated: {file_path_str}\n\nDiff:\n{}",
                format_file_lines(&diff, 1)
            )
        } else {
            // File was created
            format!(
                "File created successfully at: {file_path_str}\n\nContent:\n{}",
                format_file_lines(content, 1)
            )
        };

        Ok(ToolOutput::new(response, ""))
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

        let tool = WriteTool::new(base_path);
        let args = serde_json::json!({
            "file_path": "test.txt",
            "content": "Hello, World!"
        });

        let ctx = ToolExecCtx::new("test_tool_call");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("created successfully"));
        assert!(result.stdout.contains("Hello, World!"));

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

        let tool = WriteTool::new(base_path);
        let args = serde_json::json!({
            "file_path": "src/nested/test.rs",
            "content": "fn main() {}"
        });

        let ctx = ToolExecCtx::new("test_tool_call");
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
        let tool = WriteTool::new(base_path).with_file_state_store(store);

        let args = serde_json::json!({
            "file_path": "existing.txt",
            "content": "new content"
        });

        // Should fail because file hasn't been read
        let ctx = ToolExecCtx::new("test_tool_call");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(!result.success());
        assert!(result.stderr.contains("not been read"));
    }

    #[tokio::test]
    async fn test_write_tool_update_after_read() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        // Create file first
        let file_path = base_path.join("existing.txt");
        tokio::fs::write(&file_path, "original content")
            .await
            .unwrap();

        let store = Arc::new(FileStateStore::new());

        // Record the file as read
        let mtime = tokio::fs::metadata(&file_path)
            .await
            .unwrap()
            .modified()
            .unwrap()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        store.record(file_path.clone(), mtime);

        let tool = WriteTool::new(base_path).with_file_state_store(store);

        let args = serde_json::json!({
            "file_path": "existing.txt",
            "content": "new content"
        });

        // Should succeed because file was recorded as read
        let ctx = ToolExecCtx::new("test_tool_call");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());
        assert!(result.stdout.contains("updated"));
        assert!(result.stdout.contains("Diff:"));

        // Verify file was updated
        let content = tokio::fs::read_to_string(&file_path).await.unwrap();
        assert_eq!(content, "new content");
    }

    #[tokio::test]
    async fn test_write_tool_absolute_path() {
        let temp_dir = TempDir::new().unwrap();
        let base_path = temp_dir.path();

        let tool = WriteTool::new(base_path);
        let absolute_path = base_path.join("absolute.txt");
        let args = serde_json::json!({
            "file_path": absolute_path.to_str().unwrap(),
            "content": "absolute path content"
        });

        let ctx = ToolExecCtx::new("test_tool_call");
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.success());

        let content = tokio::fs::read_to_string(absolute_path).await.unwrap();
        assert_eq!(content, "absolute path content");
    }
}
