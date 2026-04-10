use crate::tool::Tool;
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

pub struct ReadTool {
    base_dir: PathBuf,
}

impl ReadTool {
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
impl Tool for ReadTool {
    fn name(&self) -> &'static str {
        "read"
    }

    fn description(&self) -> &'static str {
        "Read the contents of a file. Use offset and limit to read specific sections."
    }

    fn parameters_schema(&self) -> Value {
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

    async fn execute(&self, args: Value) -> Result<ToolOutput> {
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;
        let offset = args["offset"].as_u64().map_or(1, |n| n as usize);
        let limit = args["limit"].as_u64().map(|n| n as usize);

        let path = self.resolve_path(path_str)?;

        tracing::debug!("Read: {}", path.display());

        let content = tokio::fs::read_to_string(&path).await?;
        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        let start = offset.saturating_sub(1); // Convert to 0-based
        let end = limit
            .map_or(total_lines, |l| start + l)
            .min(total_lines);

        if start >= total_lines {
            return Ok(ToolOutput::new(
                format!("File has {total_lines} lines, offset {offset} is out of range"),
                "",
            ));
        }

        let result = if start == 0 && end == total_lines {
            // Reading whole file
            content
        } else {
            // Reading partial content
            lines[start..end].join("\n")
        };

        Ok(ToolOutput::new(result, ""))
    }

    fn requires_confirmation(&self) -> bool {
        false
    }

    async fn is_allowed(&self, args: &Value) -> Result<bool> {
        if let Some(path) = args["path"].as_str() {
            return self.resolve_path(path).map(|_| true);
        }
        Ok(false)
    }
}
