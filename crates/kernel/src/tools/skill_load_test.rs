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

    let ctx = ToolExecCtx::new("test_tool_call", temp.path(), "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    assert!(result.text_content().contains("Test Skill"));
    assert!(result.text_content().contains("description: Test skill"));
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

    let ctx = ToolExecCtx::new("test_tool_call", temp.path(), "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    assert!(result.text_content().contains("Debugging Skill"));
}

#[tokio::test]
async fn test_load_skill_not_found() {
    let temp = TempDir::new().unwrap();
    let tool = SkillTool::new(vec![]);
    let args = serde_json::json!({
        "name": "nonexistent"
    });

    let ctx = ToolExecCtx::new("test_tool_call", temp.path(), "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.is_error);
    assert!(result.error_text().contains("not found"));
}

#[tokio::test]
async fn test_load_skill_path_not_found() {
    let temp = TempDir::new().unwrap();
    let tool = SkillTool::new(vec![]);
    let args = serde_json::json!({
        "path": "/nonexistent/path/SKILL.md"
    });

    let ctx = ToolExecCtx::new("test_tool_call", temp.path(), "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.is_error);
    assert!(result.error_text().contains("not found"));
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

    let ctx = ToolExecCtx::new("test_tool_call", temp.path(), "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    assert!(result.text_content().contains("Writing Superpower"));
}
