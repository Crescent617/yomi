use crate::tools::base::{get_mtime, MAX_FILE_SIZE};
use crate::tools::file_lock::{lock_exclusive_timeout, DEFAULT_LOCK_TIMEOUT};
use crate::tools::file_state::FileStateStore;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use crate::utils::diff::generate_diff;
use crate::utils::line_numbers::format_file_lines;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;
use std::sync::Arc;

pub const EDIT_TOOL_NAME: &str = "edit";

pub struct EditTool {
    file_state_store: Option<Arc<FileStateStore>>,
}

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl EditTool {
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
    async fn check_staleness(&self, path: &Path) -> Option<String> {
        let store = self.file_state_store.as_ref()?;

        // Check if file has been modified (mtime changed)
        let current_mtime = get_mtime(path).await;
        if store.is_stale(path, current_mtime) {
            return Some(
                "File has been modified since it was read. Read the file again before editing."
                    .to_string(),
            );
        }

        None
    }
}
/// Find the actual string in file content.
///
/// Returns `Some(search_string)` if found, None otherwise.
/// This is a simple wrapper that may be extended for quote normalization in the future.
fn find_actual_string(file_content: &str, search_string: &str) -> Option<String> {
    if file_content.contains(search_string) {
        Some(search_string.to_string())
    } else {
        None
    }
}

#[async_trait]
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        EDIT_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Replace text in a file. Use this instead of sed. Provide old_str to locate the text (should be unique enough) and new_str to replace it. Supports replace_all=true to replace all occurrences."
    }

    fn params(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file"
                },
                "old_str": {
                    "type": "string",
                    "description": "The text to find and replace. Should be unique enough to identify the location."
                },
                "new_str": {
                    "type": "string",
                    "description": "The new text to replace old_str with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "If true, replace all occurrences. Default false (replace first only).",
                    "default": false
                }
            },
            "required": ["path", "old_str", "new_str"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
        let old_str = args["old_str"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'old_str' argument"))?;
        let new_str = args["new_str"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'new_str' argument"))?;
        let replace_all = args["replace_all"].as_bool().unwrap_or(false);

        let path = ctx.working_dir.join(path_str);

        tracing::debug!("Edit: replace in {}", path.display());

        // Check if file exists
        if !tokio::fs::try_exists(&path).await? {
            return Ok(ToolOutput::error(format!(
                "File does not exist: {path_str}"
            )));
        }
        // Check file size
        if tokio::fs::metadata(&path).await?.len() > MAX_FILE_SIZE {
            return Ok(ToolOutput::error(format!(
                "File is too large to edit: {path_str}"
            )));
        }

        // Check if file has been read before editing
        if let Some(ref store) = self.file_state_store {
            if !store.has_recorded(&path) {
                return Ok(ToolOutput::error(format!(
                    "File has not been read yet. Read it first before editing: {path_str}"
                )));
            }

            // Check for staleness
            if let Some(error) = self.check_staleness(&path).await {
                return Ok(ToolOutput::error(error));
            }
        }

        // Acquire exclusive lock BEFORE reading to prevent race conditions
        // This ensures no other process can modify the file between our read and write
        let _guard = lock_exclusive_timeout(&path, DEFAULT_LOCK_TIMEOUT)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire write lock: {e}"))?;

        // Read file content (now protected by exclusive lock)
        let content = tokio::fs::read_to_string(&path).await?;

        // Validate old_str is not empty (except for creating new files)
        if old_str.is_empty() && !content.is_empty() {
            return Ok(ToolOutput::error(
                "Cannot use empty old_str on existing file with content. Provide the text to replace."
            ));
        }

        // Check if old_str and new_str are the same
        if old_str == new_str {
            return Ok(ToolOutput::error(
                "No changes to make: old_str and new_str are exactly the same.",
            ));
        }

        // Find the actual string in the file (handles quote normalization)
        let Some(actual_old_str) = find_actual_string(&content, old_str) else {
            return Ok(ToolOutput::error(format!(
                "Could not find 'old_str' in file. String not found:\n{old_str}"
            )));
        };

        // Count occurrences
        let occurrences = content.matches(&actual_old_str).count();
        if occurrences == 0 {
            return Ok(ToolOutput::error(format!(
                "Could not find 'old_str' in file. String not found:\n{old_str}"
            )));
        }

        // Check for multiple matches when replace_all is false
        if occurrences > 1 && !replace_all {
            return Ok(ToolOutput::error(format!(
                "Found {occurrences} matches of the string to replace, but replace_all is false. \
                 To replace all occurrences, set replace_all to true. \
                 To replace only one occurrence, please provide more context to uniquely identify the instance.\n\
                 String: {old_str}"
            )));
        }

        // Perform the replacement
        let new_content = if replace_all {
            content.replace(&actual_old_str, new_str)
        } else {
            content.replacen(&actual_old_str, new_str, 1)
        };

        // Write the new content (exclusive lock still held)
        tokio::fs::write(&path, &new_content).await?;

        // Update file mtime in store
        if let Some(ref store) = self.file_state_store {
            let mtime = get_mtime(&path).await;
            store.record(path.clone(), mtime);
        }

        // Generate diff
        let diff = generate_diff(&content, &new_content, 3);

        // Build success message
        let action = if replace_all {
            format!("Replaced all {occurrences} occurrences")
        } else {
            "Replaced 1 occurrence".to_string()
        };

        let response = format!(
            "{} in {}\n\nDiff:\n{}",
            action,
            path_str,
            format_file_lines(&diff, 1)
        );

        Ok(ToolOutput::text_with_summary(response, ""))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_edit_tool_basic() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "hello world").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();
        // Use canonicalized path for file state store
        let full_path = path.join(file_name).canonicalize().unwrap();

        let tool = EditTool::new();

        // First, simulate a read by setting file state with actual file's mtime
        let store = Arc::new(FileStateStore::new());
        let _content = "hello world".to_string();

        // Get actual file mtime
        let mtime = crate::tools::base::get_mtime(&full_path).await;

        store.record(full_path.clone(), mtime);

        let tool = tool.with_file_state_store(store);

        let args = serde_json::json!({
            "path": file_name,
            "old_str": "hello",
            "new_str": "goodbye"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.text_content().contains("Replaced"));

        let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
        assert_eq!(new_content, "goodbye world\n");
    }

    #[tokio::test]
    async fn test_edit_tool_no_read_first() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "hello world").unwrap();
        let path = temp_file.path().parent().unwrap();
        let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

        let store = Arc::new(FileStateStore::new());
        let tool = EditTool::new().with_file_state_store(store);

        let args = serde_json::json!({
            "path": file_name,
            "old_str": "hello",
            "new_str": "goodbye"
        });

        let ctx = ToolExecCtx::new("test_tool_call", path);
        let result = tool.exec(args, ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.error_text().contains("not been read"));
    }
}
