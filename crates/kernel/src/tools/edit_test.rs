use super::*;

use crate::tools::helper::get_mtime;
use std::io::Write;
use tempfile::{NamedTempFile, TempDir};

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
    // Multi-line edit on a never-recorded file is allowed (no read-first gate);
    // old_str exact-match is the safeguard for unseen files.
    let mut temp_file = NamedTempFile::new().unwrap();
    writeln!(temp_file, "line 1").unwrap();
    writeln!(temp_file, "line 2").unwrap();
    let path = temp_file.path().parent().unwrap();
    let file_name = temp_file.path().file_name().unwrap().to_str().unwrap();

    let store = Arc::new(FileStateStore::new());
    let tool = EditTool::new(store);

    let args = serde_json::json!({
        "path": file_name,
        "old_str": "line 1\nline 2",
        "new_str": "replaced"
    });

    let ctx = ToolExecCtx::new("test_tool_call", path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);
    assert!(result.text_content().contains("Replaced"));

    let new_content = tokio::fs::read_to_string(temp_file.path()).await.unwrap();
    assert_eq!(new_content, "replaced\n");
}

#[tokio::test]
async fn test_edit_succeeds_despite_external_change_elsewhere() {
    // Anti-regression pin: edit has NO staleness gate. Even with a recorded
    // mtime that no longer matches (file modified externally), the edit must
    // succeed as long as old_str matches the current bytes. If a staleness
    // gate is ever re-introduced, this test goes red.
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().canonicalize().unwrap();

    let file_path = base_path.join("test.txt");
    tokio::fs::write(&file_path, "hello\nworld\nfoo")
        .await
        .unwrap();

    let store = Arc::new(FileStateStore::new());
    let mtime = get_mtime(&file_path).await.unwrap();
    store.record(file_path.clone(), mtime).await;

    // 外部修改文件的其他区域（store 未更新）
    // sleep 1.1s 确保 mtime 真的变化（超过秒级粒度文件系统的分辨率）
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    tokio::fs::write(&file_path, "hello\nCHANGED\nfoo")
        .await
        .unwrap();

    let tool = EditTool::new(store);
    let args = serde_json::json!({
        "path": "test.txt",
        "old_str": "foo",
        "new_str": "bar"
    });

    let ctx = ToolExecCtx::new("test_tool_call", &base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "hello\nCHANGED\nbar");
}

#[tokio::test]
async fn test_edit_external_change_breaks_old_str_match() {
    // The natural guard without a staleness gate: if the current content no
    // longer contains old_str (e.g. after an external modification), the edit
    // fails with "not found" — prompting a re-read.
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().canonicalize().unwrap();

    let file_path = base_path.join("test.txt");
    tokio::fs::write(&file_path, "hello\nworld").await.unwrap();
    // 外部修改覆盖了目标文本
    tokio::fs::write(&file_path, "modified\ncontent")
        .await
        .unwrap();

    let tool = EditTool::default();
    let args = serde_json::json!({
        "path": "test.txt",
        "old_str": "hello\nworld",
        "new_str": "goodbye\nearth"
    });

    let ctx = ToolExecCtx::new("test_tool_call", &base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.is_error);
    assert!(result.error_text().contains("Could not find"));
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

#[tokio::test]
async fn test_concurrent_edit_overlapping_old_str() {
    // 两个并发 edit 调同一个文件，重叠 old_str
    // 锁串行化后，第二个 edit 的 old_str 已不在文件里 → 失败
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().canonicalize().unwrap();

    let file_path = base_path.join("test.txt");
    tokio::fs::write(&file_path, "hello world").await.unwrap();

    let store = Arc::new(FileStateStore::new());
    let mtime = get_mtime(&file_path).await.unwrap();
    store.record(file_path.clone(), mtime).await;

    let tool1 = EditTool::new(Arc::clone(&store));
    let tool2 = EditTool::new(Arc::clone(&store));

    let args1 = serde_json::json!({
        "path": "test.txt",
        "old_str": "hello",
        "new_str": "goodbye"
    });

    let args2 = serde_json::json!({
        "path": "test.txt",
        "old_str": "hello",
        "new_str": "hi"
    });

    let ctx1 = ToolExecCtx::new("test1", &base_path, "test-session");
    let ctx2 = ToolExecCtx::new("test2", &base_path, "test-session");

    let (r1, r2) = tokio::join!(tool1.exec(args1, ctx1), tool2.exec(args2, ctx2));

    let result1 = r1.unwrap();
    let result2 = r2.unwrap();

    let exactly_one =
        (result1.success() && result2.is_error) || (result1.is_error && result2.success());
    assert!(exactly_one, "Expected one success and one failure");

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert!(content.contains("goodbye") || content.contains("hi"));
}

#[tokio::test]
async fn test_blind_edit_does_not_unlock_write_overwrite() {
    // A successful edit on a never-read file must NOT mark it as known:
    // write-overwrite's read-first gate stays closed until an actual read.
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().canonicalize().unwrap();

    let file_path = base_path.join("test.txt");
    tokio::fs::write(&file_path, "hello world").await.unwrap();

    let store = Arc::new(FileStateStore::new());
    let edit_tool = EditTool::new(Arc::clone(&store));

    // Blind edit succeeds (edit itself has no gate)
    let args = serde_json::json!({
        "path": "test.txt",
        "old_str": "hello",
        "new_str": "goodbye"
    });
    let ctx = ToolExecCtx::new("test1", &base_path, "test-session");
    let result = edit_tool.exec(args, ctx).await.unwrap();
    assert!(!result.is_error);

    // ...but the file is still unknown to the store
    assert!(!store.has_recorded(&file_path));

    // So write-overwrite is still blocked by the read-first gate
    let write_tool = crate::tools::write::WriteTool::new(store);
    let args = serde_json::json!({
        "file_path": "test.txt",
        "content": "blind overwrite"
    });
    let ctx = ToolExecCtx::new("test2", &base_path, "test-session");
    let result = write_tool.exec(args, ctx).await.unwrap();
    assert!(result.is_error);
    assert!(result.error_text().contains("not been read"));

    // Content from the earlier edit is untouched by the rejected overwrite
    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "goodbye world");
}
