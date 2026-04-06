use anyhow::Result;
use async_trait::async_trait;
use crate::tool::Tool;
use crate::types::ToolOutput;
use serde_json::Value;
use std::path::PathBuf;

pub struct FileTool {
    base_dir: PathBuf,
}

impl FileTool {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self { base_dir: base_dir.into() }
    }

    fn resolve_path(&self, relative: &str) -> Result<PathBuf> {
        let path = self.base_dir.join(relative);
        let canonical = path.canonicalize().unwrap_or(path);
        if !canonical.starts_with(&self.base_dir) {
            return Err(anyhow::anyhow!("Path escapes base directory: {relative}"));
        }
        Ok(canonical)
    }
}

#[async_trait]
impl Tool for FileTool {
    fn name(&self) -> &'static str { "file" }

    fn description(&self) -> &'static str {
        "Read, write, or modify files in the working directory"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write", "append", "delete", "list"],
                    "description": "The file operation to perform"
                },
                "path": {
                    "type": "string",
                    "description": "Relative path to the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content for write/append operations"
                }
            },
            "required": ["action", "path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput> {
        let action = args["action"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' argument"))?;
        let path_str = args["path"].as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
        let path = self.resolve_path(path_str)?;

        tracing::debug!("File operation: {} {}", action, path.display());

        match action {
            "read" => {
                let content = tokio::fs::read_to_string(&path).await?;
                tracing::debug!("File read successfully: {} ({} bytes)", path.display(), content.len());
                Ok(ToolOutput::new(content, ""))
            }
            "write" => {
                let content = args["content"].as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'content' for write"))?;
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                tokio::fs::write(&path, content).await?;
                tracing::debug!("File written successfully: {} ({} bytes)", path.display(), content.len());
                Ok(ToolOutput::new("File written successfully", ""))
            }
            "append" => {
                let content = args["content"].as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'content' for append"))?;
                let mut file = tokio::fs::OpenOptions::new()
                    .append(true).create(true).open(&path).await?;
                use tokio::io::AsyncWriteExt;
                file.write_all(content.as_bytes()).await?;
                tracing::debug!("File appended successfully: {} ({} bytes)", path.display(), content.len());
                Ok(ToolOutput::new("Content appended successfully", ""))
            }
            "delete" => {
                tokio::fs::remove_file(&path).await?;
                tracing::debug!("File deleted successfully: {}", path.display());
                Ok(ToolOutput::new("File deleted successfully", ""))
            }
            "list" => {
                let mut entries = tokio::fs::read_dir(&path).await?;
                let mut result = String::new();
                let mut count = 0;
                while let Some(entry) = entries.next_entry().await? {
                    let name = entry.file_name();
                    let meta = entry.metadata().await.ok();
                    let is_dir = meta.is_some_and(|m| m.is_dir());
                    result.push_str(&format!("{}{}\n",
                        name.to_string_lossy(),
                        if is_dir { "/" } else { "" }
                    ));
                    count += 1;
                }
                tracing::debug!("Directory listed: {} ({} entries)", path.display(), count);
                Ok(ToolOutput::new(result, ""))
            }
            _ => Err(anyhow::anyhow!("Unknown action: {action}")),
        }
    }

    fn requires_confirmation(&self) -> bool { true }

    async fn is_allowed(&self, args: &Value) -> Result<bool> {
        if let Some(path) = args["path"].as_str() {
            return self.resolve_path(path).map(|_| true);
        }
        Ok(false)
    }
}
