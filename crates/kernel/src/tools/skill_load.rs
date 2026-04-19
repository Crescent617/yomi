use crate::skill::SkillLoader;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::path::PathBuf;

pub const SKILL_TOOL_NAME: &str = "skill";

/// Tool for loading skill content
pub struct SkillTool {
    loader: SkillLoader,
}

impl SkillTool {
    pub fn new(skill_folders: Vec<PathBuf>) -> Self {
        Self {
            loader: SkillLoader::new(skill_folders),
        }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &'static str {
        SKILL_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Load a skill file and return its content. Use this to load skill documentation on demand."
    }

    fn params(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the skill to load (e.g., 'debugging')"
                },
                "path": {
                    "type": "string",
                    "description": "Direct path to a SKILL.md file. If provided, 'name' is ignored."
                }
            },
            "required": []
        })
    }

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let path_arg = args["path"].as_str();
        let name_arg = args["name"].as_str();

        // Determine which file to load
        let skill_path = if let Some(path_str) = path_arg {
            let path = PathBuf::from(path_str);
            if !tokio::fs::try_exists(&path).await.unwrap_or(false) {
                return Ok(ToolOutput::new_err(format!(
                    "Skill file not found: {}",
                    path.display()
                )));
            }
            path
        } else if let Some(name) = name_arg {
            match self.loader.find_skill_file(name).await {
                Some(path) => path,
                None => {
                    return Ok(ToolOutput::new_err(format!(
                        "Skill '{name}' not found in configured skill folders"
                    )));
                }
            }
        } else {
            return Ok(ToolOutput::new_err(
                "Either 'name' or 'path' must be provided".to_string(),
            ));
        };

        // Read and return the skill content
        match SkillLoader::read_skill_content(&skill_path).await {
            Ok(content) => {
                let summary = format!("Loaded skill from {}", skill_path.display());
                Ok(ToolOutput::new(content, &summary))
            }
            Err(e) => Ok(ToolOutput::new_err(format!(
                "Failed to read skill file: {e}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_load_skill_by_path() {
        let temp = TempDir::new().unwrap();
        let skill_content = r"---
description: Test skill
triggers:
  - test
---

# Test Skill

This is a test skill.";

        let skill_path = temp.path().join("SKILL.md");
        let mut file = std::fs::File::create(&skill_path).unwrap();
        file.write_all(skill_content.as_bytes()).unwrap();

        let tool = SkillTool::new(vec![]);
        let args = serde_json::json!({
            "path": skill_path.to_str().unwrap()
        });

        let ctx = ToolExecCtx::new("test_tool_call");
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        assert!(result.stdout.contains("Test Skill"));
        assert!(result.stdout.contains("description: Test skill"));
    }

    #[tokio::test]
    async fn test_load_skill_by_name() {
        let temp = TempDir::new().unwrap();
        let skills_dir = temp.path().join("skills").join("debugging");
        std::fs::create_dir_all(&skills_dir).unwrap();

        let skill_content = r"---
description: Debugging skill
---

# Debugging Skill";

        let skill_path = skills_dir.join("SKILL.md");
        let mut file = std::fs::File::create(&skill_path).unwrap();
        file.write_all(skill_content.as_bytes()).unwrap();

        let tool = SkillTool::new(vec![temp.path().join("skills")]);
        let args = serde_json::json!({
            "name": "debugging"
        });

        let ctx = ToolExecCtx::new("test_tool_call");
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        assert!(result.stdout.contains("Debugging Skill"));
    }

    #[tokio::test]
    async fn test_load_skill_not_found() {
        let tool = SkillTool::new(vec![]);
        let args = serde_json::json!({
            "name": "nonexistent"
        });

        let ctx = ToolExecCtx::new("test_tool_call");
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(!result.success());
        assert!(result.stderr.contains("not found"));
    }

    #[tokio::test]
    async fn test_load_skill_path_not_found() {
        let tool = SkillTool::new(vec![]);
        let args = serde_json::json!({
            "path": "/nonexistent/path/SKILL.md"
        });

        let ctx = ToolExecCtx::new("test_tool_call");
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(!result.success());
        assert!(result.stderr.contains("not found"));
    }

    #[tokio::test]
    async fn test_load_skill_nested_name() {
        let temp = TempDir::new().unwrap();
        let skills_dir = temp
            .path()
            .join("skills")
            .join("superpowers")
            .join("writing");
        std::fs::create_dir_all(&skills_dir).unwrap();

        let skill_content = r"---
description: Writing superpower
---

# Writing Superpower";

        let skill_path = skills_dir.join("SKILL.md");
        let mut file = std::fs::File::create(&skill_path).unwrap();
        file.write_all(skill_content.as_bytes()).unwrap();

        let tool = SkillTool::new(vec![temp.path().join("skills")]);
        let args = serde_json::json!({
            "name": "superpowers:writing"
        });

        let ctx = ToolExecCtx::new("test_tool_call");
        let result = tool.exec(args, ctx).await.unwrap();

        assert!(result.success());
        assert!(result.stdout.contains("Writing Superpower"));
    }
}
