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

    // First, simulate a read by setting file state with actual file's mtime
    let store = Arc::new(FileStateStore::new());
    let _content = "hello world".to_string();

    // Get actual file mtime
    let mtime = crate::tools::helper::get_mtime(&full_path).await.unwrap();

    store.record(full_path.clone(), mtime).await;

    let tool = EditTool::new(store);

    let args = serde_json::json!({
        "path": file_name,
        "old_str": "hello",
        "new_str": "goodbye"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.text_content().contains("Replaced"));

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "goodbye world\n");
}

#[tokio::test]
async fn test_edit_tool_no_read_first_multiline() {
    // Multi-line edit should require read first
    let mut temp_file = NamedTempFile::new().unwrap();
    writeln!(temp_file, "line 1").unwrap();
    writeln!(temp_file, "line 2").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let tool = EditTool::new(store);

    // Multi-line old_str requires read first
    let args = serde_json::json!({
        "path": file_name,
        "old_str": "line 1\nline 2",
        "new_str": "replaced"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.is_error);
    assert!(result.error_text().contains("not been read"));
}

#[tokio::test]
async fn test_edit_tool_simple_edit_no_read() {
    // Simple single-line edit should work without read
    let mut temp_file = NamedTempFile::new().unwrap();
    writeln!(temp_file, "hello world").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let tool = EditTool::new(store);

    let args = serde_json::json!({
        "path": file_name,
        "old_str": "hello",
        "new_str": "goodbye"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);
    assert!(result.text_content().contains("Replaced"));

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "goodbye world\n");
}

#[tokio::test]
async fn test_edit_crlf() {
    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "line1\r\nline2\r\n").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
        .await
        .unwrap();
    store
        .record(temp_file.path().canonicalize().unwrap(), mtime)
        .await;

    let tool = EditTool::new(store);
    let args = serde_json::json!({
        "path": file_name,
        "old_str": "line1\nline2",
        "new_str": "foo\nbar"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "foo\nbar\r\n");
}

#[tokio::test]
async fn test_edit_curly_quotes() {
    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "println!(\u{2018}hello\u{2019});").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
        .await
        .unwrap();
    store
        .record(temp_file.path().canonicalize().unwrap(), mtime)
        .await;

    let tool = EditTool::new(store);
    let args = serde_json::json!({
        "path": file_name,
        "old_str": "println!('hello');",
        "new_str": "println!('world');"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "println!('world');");
}

#[tokio::test]
async fn test_edit_unicode_whitespace() {
    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "let\u{00a0}x = 1;").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
        .await
        .unwrap();
    store
        .record(temp_file.path().canonicalize().unwrap(), mtime)
        .await;

    let tool = EditTool::new(store);
    let args = serde_json::json!({
        "path": file_name,
        "old_str": "let x = 1;",
        "new_str": "let y = 2;"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "let y = 2;");
}

#[tokio::test]
async fn test_edit_crlf_multi() {
    // 多个 CRLF 连续：文件 a\r\nb\r\nc\r\n，搜索 b\nc，替换 X\nY
    // 验证映射不会越界，只替换中间一段
    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "a\r\nb\r\nc\r\n").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
        .await
        .unwrap();
    store
        .record(temp_file.path().canonicalize().unwrap(), mtime)
        .await;

    let tool = EditTool::new(store);
    let args = serde_json::json!({
        "path": file_name,
        "old_str": "b\nc",
        "new_str": "X\nY"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "a\r\nX\nY\r\n");
}

#[tokio::test]
async fn test_edit_crlf_mixed_lf() {
    // CRLF + 纯 LF 混合：文件 a\r\nb\nc，搜索 b\nc
    // 验证匹配到纯 LF 段，映射提取的是 b\nc 而非 b\r\nc
    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "a\r\nb\nc").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
        .await
        .unwrap();
    store
        .record(temp_file.path().canonicalize().unwrap(), mtime)
        .await;

    let tool = EditTool::new(store);
    let args = serde_json::json!({
        "path": file_name,
        "old_str": "b\nc",
        "new_str": "X\nY"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "a\r\nX\nY");
}

#[tokio::test]
async fn test_edit_crlf_trailing() {
    // 文件末尾 CRLF：a\r\n，搜索 a\n，替换 X
    // 验证 end == byte_map.len() 的 map_range 边界（orig_end = orig_len）
    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "a\r\n").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
        .await
        .unwrap();
    store
        .record(temp_file.path().canonicalize().unwrap(), mtime)
        .await;

    let tool = EditTool::new(store);
    let args = serde_json::json!({
        "path": file_name,
        "old_str": "a\n",
        "new_str": "X"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "X");
}

#[tokio::test]
async fn test_edit_fullwidth_quotes() {
    // 全角引号：\u{FF07} (') \u{FF02} (")
    let mut temp_file = NamedTempFile::new().unwrap();
    write!(temp_file, "say\u{FF07}hello\u{FF02}world\u{FF07}").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let mtime = get_mtime(&temp_file.path().canonicalize().unwrap())
        .await
        .unwrap();
    store
        .record(temp_file.path().canonicalize().unwrap(), mtime)
        .await;

    let tool = EditTool::new(store);
    let args = serde_json::json!({
        "path": file_name,
        "old_str": "say'hello\"world'",
        "new_str": "print'goodbye\"earth'"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "print'goodbye\"earth'");
}
