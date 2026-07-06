use super::*;

use crate::tools::DEFAULT_MAX_TOOL_OUTPUT_LENGTH;
use tempfile::TempDir;

#[tokio::test]
async fn test_read_basic() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create test file
    tokio::fs::write(base_path.join("test.txt"), "Hello, World!")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    assert!(result.text_content().contains("Hello, World!"));
}

#[tokio::test]
async fn test_read_with_offset() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "line1\nline2\nline3")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "offset": 2});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    assert!(!content.contains("line1"));
    assert!(content.contains("line2"));
    assert!(content.contains("line3"));
}

#[tokio::test]
async fn test_read_with_limit() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "line1\nline2\nline3")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "limit": 2});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    assert!(content.contains("line1"));
    assert!(content.contains("line2"));
    assert!(!content.contains("line3"));
}

#[tokio::test]
async fn test_read_with_offset_and_limit() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "a\nb\nc\nd\ne")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args =
        serde_json::json!({"path": "test.txt", "offset": 2, "limit": 2, "line_numbers": true});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    let lines: Vec<&str> = content.lines().collect();
    assert!(lines.iter().any(|l| l.contains('b')));
    assert!(lines.iter().any(|l| l.contains('c')));
    assert!(!lines
        .iter()
        .any(|l| l.trim() == "a" || l.trim().ends_with(" a")));
    assert!(!lines
        .iter()
        .any(|l| l.trim() == "d" || l.trim().ends_with(" d")));
}

#[tokio::test]
async fn test_read_with_line_numbers() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "line1\nline2")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "line_numbers": true});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    assert!(content.contains("1\tline1"));
    assert!(content.contains("2\tline2"));
}

#[tokio::test]
async fn test_read_offset_with_line_numbers() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "a\nb\nc")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "offset": 2, "line_numbers": true});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    // Line numbers should start from offset
    assert!(content.contains("2\tb"));
    assert!(content.contains("3\tc"));
    assert!(!content.contains("1\ta"));
}

#[tokio::test]
async fn test_read_file_not_found() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "nonexistent.txt"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.is_error);
    assert!(result.error_text().contains("does not exist"));
}

#[tokio::test]
async fn test_read_offset_out_of_range() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "line1\nline2")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "offset": 10});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.is_error);
    assert!(result.error_text().contains("out of range"));
}

#[tokio::test]
async fn test_read_without_line_numbers_stopped_hint() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    tokio::fs::write(base_path.join("test.txt"), "a\nb\nc\nd\ne")
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "test.txt", "limit": 3, "line_numbers": false});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let content = result.text_content();
    // File content is a\nb\nc — check actual lines, not single chars
    // (prompt text contains letters like 'd' in "read")
    let lines: Vec<&str> = content.lines().collect();
    assert_eq!(lines[0], "a");
    assert_eq!(lines[1], "b");
    assert_eq!(lines[2], "c");
    // Should tell the model where it stopped when line_numbers is false
    assert!(content.contains("Stopped at line 3 of 5"));
    assert!(content.contains("Use offset/limit to read more"));
}

#[tokio::test]
async fn test_read_truncation() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create a large file that will trigger truncation
    // Each line is about 100 chars, create enough lines to exceed limit
    let line = "x".repeat(100);
    let lines_needed = DEFAULT_MAX_TOOL_OUTPUT_LENGTH / 100 + 10;
    let mut content = String::with_capacity(line.len() * lines_needed + lines_needed);
    for _ in 0..lines_needed {
        content.push_str(&line);
        content.push('\n');
    }
    tokio::fs::write(base_path.join("large.txt"), content)
        .await
        .unwrap();

    let tool = ReadTool::default();
    let args = serde_json::json!({"path": "large.txt"});

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();

    assert!(result.success());
    let text = result.text_content();
    // Should contain truncation notice
    assert!(text.contains("Content truncated"));
    // Should indicate line number where truncated
    assert!(text.contains("at line"));
    // Length should be close to limit (allowing for truncation notice overhead)
    assert!(text.len() <= DEFAULT_MAX_TOOL_OUTPUT_LENGTH + 100);
}
