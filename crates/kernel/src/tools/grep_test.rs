use super::*;

use tempfile::TempDir;

#[tokio::test]
async fn test_grep_tool_filename() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(
        base_path.join("test1.rs"),
        "fn main() {\n    println!(\"hello\");\n}",
    )
    .await
    .unwrap();
    tokio::fs::write(
        base_path.join("test2.rs"),
        "fn foo() {\n    println!(\"world\");\n}",
    )
    .await
    .unwrap();
    tokio::fs::write(base_path.join("test.txt"), "just text")
        .await
        .unwrap();

    let tool = GrepTool::default();
    let args = serde_json::json!({
        "pattern": "println!",
        "output_mode": "filename"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("test1.rs"));
    assert!(result.text_content().contains("test2.rs"));
    assert!(!result.text_content().contains("test.txt"));
}

#[tokio::test]
async fn test_grep_tool_content_mode() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(
        base_path.join("test.rs"),
        "fn main() {\n    println!(\"hello\");\n}",
    )
    .await
    .unwrap();

    let tool = GrepTool::default();
    let args = serde_json::json!({
        "pattern": "println!",
        "output_mode": "content"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("println"));
}

#[tokio::test]
async fn test_grep_tool_case_insensitive() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.rs"), "fn MAIN() {}")
        .await
        .unwrap();

    let tool = GrepTool::default();
    let args = serde_json::json!({
        "pattern": "main",
        "output_mode": "content",
        "-i": true
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("MAIN"));
}

#[tokio::test]
async fn test_grep_tool_glob_filter() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.rs"), "fn main() {}")
        .await
        .unwrap();
    tokio::fs::write(base_path.join("test.js"), "function main() {}")
        .await
        .unwrap();

    let tool = GrepTool::default();
    let args = serde_json::json!({
        "pattern": "main",
        "glob": "*.rs"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("test.rs"));
    assert!(!result.text_content().contains("test.js"));
}

#[tokio::test]
async fn test_grep_tool_no_matches() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.rs"), "fn main() {}")
        .await
        .unwrap();

    let tool = GrepTool::default();
    let args = serde_json::json!({
        "pattern": "nonexistent",
        "output_mode": "filename"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("No files found"));
}

#[tokio::test]
async fn test_grep_tool_context_lines() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(
        base_path.join("test.rs"),
        "line 1\nline 2\nfn main() {\nline 4\nline 5\n}",
    )
    .await
    .unwrap();

    let tool = GrepTool::default();
    let args = serde_json::json!({
        "pattern": "fn main",
        "output_mode": "content",
        "-B": 2,
        "-A": 2
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    let content = result.text_content();
    println!("Content output:\n{content}");
    assert!(
        content.contains("line 1"),
        "Expected 'line 1' in:\n{content}"
    );
    assert!(
        content.contains("line 2"),
        "Expected 'line 2' in:\n{content}"
    );
    assert!(
        content.contains("fn main"),
        "Expected 'fn main' in:\n{content}"
    );
    assert!(
        content.contains("line 4"),
        "Expected 'line 4' in:\n{content}"
    );
    assert!(
        content.contains("line 5"),
        "Expected 'line 5' in:\n{content}"
    );
}

#[tokio::test]
async fn test_grep_tool_hidden_files() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join(".hidden.rs"), "fn secret() {}")
        .await
        .unwrap();
    tokio::fs::write(base_path.join("normal.rs"), "fn main() {}")
        .await
        .unwrap();

    let tool = GrepTool::default();

    // Always searches hidden files (claude-code behavior)
    let args = serde_json::json!({
        "pattern": "fn secret"
    });
    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains(".hidden.rs"));
}

#[tokio::test]
async fn test_grep_tool_content_mode_records_files() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(
        base_path.join("test.rs"),
        "fn main() {\n    println!(\"hello\");\n}",
    )
    .await
    .unwrap();

    // Use file_state_store to track reads
    let store = Arc::new(FileStateStore::new());
    let tool = GrepTool::new(Arc::clone(&store));

    let args = serde_json::json!({
        "pattern": "println!",
        "output_mode": "content"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());

    // Verify file was recorded in the store
    let file_path = base_path.join("test.rs").canonicalize().unwrap();
    assert!(store.has_recorded(&file_path));
    assert!(store.get_mtime(&file_path).unwrap() > 0);
}

#[tokio::test]
async fn test_grep_tool_filename_does_not_record() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(
        base_path.join("test.rs"),
        "fn main() {\n    println!(\"hello\");\n}",
    )
    .await
    .unwrap();

    // Use file_state_store to track reads
    let store = Arc::new(FileStateStore::new());
    let tool = GrepTool::new(Arc::clone(&store));

    let args = serde_json::json!({
        "pattern": "println!",
        "output_mode": "filename"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());

    // Verify file was NOT recorded (filename doesn't record)
    let file_path = base_path.join("test.rs").canonicalize().unwrap();
    assert!(!store.has_recorded(&file_path));
}

#[tokio::test]
async fn test_grep_tool_content_mode_pagination_records_only_displayed() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create multiple files with matches
    tokio::fs::write(base_path.join("file1.rs"), "fn main() { println!(\"1\"); }")
        .await
        .unwrap();
    tokio::fs::write(base_path.join("file2.rs"), "fn foo() { println!(\"2\"); }")
        .await
        .unwrap();
    tokio::fs::write(base_path.join("file3.rs"), "fn bar() { println!(\"3\"); }")
        .await
        .unwrap();

    // Use file_state_store to track reads
    let store = Arc::new(FileStateStore::new());
    let tool = GrepTool::new(Arc::clone(&store));

    // Search with limit=1, only first match should be recorded
    let args = serde_json::json!({
        "pattern": "println!",
        "output_mode": "content",
        "limit": 1
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());

    println!("Result content:\n{}", result.text_content());

    // Verify only one file was recorded
    let file1 = base_path.join("file1.rs").canonicalize().unwrap();
    let file2 = base_path.join("file2.rs").canonicalize().unwrap();
    let file3 = base_path.join("file3.rs").canonicalize().unwrap();

    println!("file1: {file1:?}");
    println!("file2: {file2:?}");
    println!("file3: {file3:?}");

    // Check how many files were recorded
    let recorded_count = [
        store.has_recorded(&file1),
        store.has_recorded(&file2),
        store.has_recorded(&file3),
    ]
    .iter()
    .filter(|&&b| b)
    .count();

    assert_eq!(
        recorded_count,
        1,
        "Expected exactly 1 file recorded. file1={}, file2={}, file3={}",
        store.has_recorded(&file1),
        store.has_recorded(&file2),
        store.has_recorded(&file3)
    );

    // The recorded file should be the one that appears in the output
    let content = result.text_content();
    if content.contains("file1.rs") {
        assert!(store.has_recorded(&file1), "file1.rs should be recorded");
    } else if content.contains("file2.rs") {
        assert!(store.has_recorded(&file2), "file2.rs should be recorded");
    } else if content.contains("file3.rs") {
        assert!(store.has_recorded(&file3), "file3.rs should be recorded");
    }
}
