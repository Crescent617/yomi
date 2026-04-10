use crate::tool::Tool;
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

pub struct EditTool {
    base_dir: PathBuf,
}

impl EditTool {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
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
impl Tool for EditTool {
    fn name(&self) -> &'static str {
        "edit"
    }

    fn description(&self) -> &'static str {
        "Replace text in a file. Use old_str to locate the text and new_str to replace it."
    }

    fn parameters_schema(&self) -> Value {
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
                "multi": {
                    "type": "boolean",
                    "description": "If true, replace all occurrences. Default false (replace first only).",
                    "default": false
                }
            },
            "required": ["path", "old_str", "new_str"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
        let old_str = args["old_str"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'old_str' argument"))?;
        let new_str = args["new_str"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'new_str' argument"))?;
        let multi = args["multi"].as_bool().unwrap_or(false);

        let path = self.resolve_path(path_str)?;

        tracing::debug!("Edit: replace in {}", path.display());

        let content = tokio::fs::read_to_string(&path).await?;

        if !content.contains(old_str) {
            return Ok(ToolOutput::new(
                "",
                format!("Could not find 'old_str' in {}", path.display()),
            ));
        }

        let new_content = if multi {
            content.replace(old_str, new_str)
        } else {
            content.replacen(old_str, new_str, 1)
        };

        tokio::fs::write(&path, new_content).await?;

        let occurrences = if multi {
            content.matches(old_str).count()
        } else {
            1
        };

        Ok(ToolOutput::new(
            format!("Replaced {occurrences} occurrence(s) in {}", path.display()),
            "",
        ))
    }

}
