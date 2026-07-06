use super::*;

use std::io::Write;
use tempfile::TempDir;

#[tokio::test]
async fn test_glob_tool_basic() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let mut file1 = std::fs::File::create(base_path.join("test1.rs")).unwrap();
    writeln!(file1, "content").unwrap();

    let mut file2 = std::fs::File::create(base_path.join("test2.rs")).unwrap();
    writeln!(file2, "content").unwrap();

    std::fs::File::create(base_path.join("test.txt")).unwrap();

    let tool = GlobTool::new();
    let args = serde_json::json!({
        "pattern": "*.rs"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("test1.rs"));
    assert!(result.text_content().contains("test2.rs"));
    assert!(!result.text_content().contains("test.txt"));
}

#[tokio::test]
async fn test_glob_tool_recursive() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let sub_dir = base_path.join("src");
    std::fs::create_dir(&sub_dir).unwrap();

    let mut file = std::fs::File::create(sub_dir.join("main.rs")).unwrap();
    writeln!(file, "content").unwrap();

    let tool = GlobTool::new();
    let args = serde_json::json!({
        "pattern": "**/*.rs"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("src/main.rs"));
}

#[tokio::test]
async fn test_glob_tool_respects_gitignore() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Initialize git repo
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(base_path)
        .output()
        .expect("Failed to init git repo");

    let mut file1 = std::fs::File::create(base_path.join("tracked.rs")).unwrap();
    writeln!(file1, "content").unwrap();

    let target_dir = base_path.join("target");
    std::fs::create_dir(&target_dir).unwrap();
    let mut file2 = std::fs::File::create(target_dir.join("ignored.rs")).unwrap();
    writeln!(file2, "content").unwrap();

    let mut gitignore = std::fs::File::create(base_path.join(".gitignore")).unwrap();
    writeln!(gitignore, "target/").unwrap();

    let tool = GlobTool::new();
    let args = serde_json::json!({
        "pattern": "**/*.rs"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("tracked.rs"));
    assert!(!result.text_content().contains("target/ignored.rs"));
}

#[tokio::test]
async fn test_glob_tool_no_matches() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let tool = GlobTool::new();
    let args = serde_json::json!({
        "pattern": "*.nonexistent"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("No files found"));
}

#[tokio::test]
async fn test_glob_tool_with_path() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let sub_dir = base_path.join("src");
    std::fs::create_dir(&sub_dir).unwrap();

    let mut file = std::fs::File::create(sub_dir.join("main.rs")).unwrap();
    writeln!(file, "content").unwrap();

    let tool = GlobTool::new();
    let args = serde_json::json!({
        "pattern": "*.rs",
        "path": "src"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("main.rs"));
}

#[tokio::test]
async fn test_glob_tool_nonexistent_dir() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let tool = GlobTool::new();
    let args = serde_json::json!({
        "pattern": "*.rs",
        "path": "nonexistent"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.is_error);
    assert!(result.error_text().contains("does not exist"));
}

#[tokio::test]
async fn test_glob_tool_hidden_files() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create hidden file
    let mut hidden = std::fs::File::create(base_path.join(".hidden.rs")).unwrap();
    writeln!(hidden, "content").unwrap();

    // Create normal file
    let mut normal = std::fs::File::create(base_path.join("normal.rs")).unwrap();
    writeln!(normal, "content").unwrap();

    let tool = GlobTool::new();

    // Without include_hidden flag (default true) - should include .hidden.rs
    let args = serde_json::json!({
        "pattern": "*.rs"
    });
    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains(".hidden.rs"));
    assert!(result.text_content().contains("normal.rs"));

    // With include_hidden: false - should not include .hidden.rs
    let args = serde_json::json!({
        "pattern": "*.rs",
        "include_hidden": false
    });
    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(!result.text_content().contains(".hidden.rs"));
    assert!(result.text_content().contains("normal.rs"));
}

#[tokio::test]
async fn test_glob_tool_brace_expansion() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create files with different extensions
    let mut file1 = std::fs::File::create(base_path.join("test.rs")).unwrap();
    writeln!(file1, "content").unwrap();

    let mut file2 = std::fs::File::create(base_path.join("test.ts")).unwrap();
    writeln!(file2, "content").unwrap();

    let mut file3 = std::fs::File::create(base_path.join("test.js")).unwrap();
    writeln!(file3, "content").unwrap();

    std::fs::File::create(base_path.join("test.txt")).unwrap();

    let tool = GlobTool::new();
    // Use brace expansion to match multiple extensions
    let args = serde_json::json!({
        "pattern": "*.{rs,ts,js}"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(
        result.text_content().contains("test.rs"),
        "Should match .rs files"
    );
    assert!(
        result.text_content().contains("test.ts"),
        "Should match .ts files"
    );
    assert!(
        result.text_content().contains("test.js"),
        "Should match .js files"
    );
    assert!(
        !result.text_content().contains("test.txt"),
        "Should not match .txt files"
    );
}
