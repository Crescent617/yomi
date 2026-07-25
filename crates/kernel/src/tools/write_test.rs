use super::*;

use tempfile::TempDir;

#[tokio::test]
async fn test_write_tool_create_new() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let tool = WriteTool::default();
    let args = serde_json::json!({
        "file_path": "test.txt",
        "content": "Hello, World!"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("File created"));
    assert!(result.text_content().contains("test.txt"));

    // Verify file was created
    let content = tokio::fs::read_to_string(base_path.join("test.txt"))
        .await
        .unwrap();
    assert_eq!(content, "Hello, World!");
}

#[tokio::test]
async fn test_write_tool_create_in_subdir() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let tool = WriteTool::default();
    let args = serde_json::json!({
        "file_path": "src/nested/test.rs",
        "content": "fn main() {}"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());

    // Verify file was created
    let content = tokio::fs::read_to_string(base_path.join("src/nested/test.rs"))
        .await
        .unwrap();
    assert_eq!(content, "fn main() {}");
}

#[tokio::test]
async fn test_write_tool_update_without_read() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    // Create file first
    tokio::fs::write(base_path.join("existing.txt"), "original content")
        .await
        .unwrap();

    let store = Arc::new(FileStateStore::new());
    let tool = WriteTool::new(store);

    let args = serde_json::json!({
        "file_path": "existing.txt",
        "content": "new content"
    });

    // Should fail because file hasn't been read
    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.is_error);
    assert!(result.error_text().contains("not been read"));
}

#[tokio::test]
async fn test_write_tool_update_after_read() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().canonicalize().unwrap();

    // Create file first
    let file_path = base_path.join("existing.txt");
    tokio::fs::write(&file_path, "original content")
        .await
        .unwrap();

    let store = Arc::new(FileStateStore::new());

    // Record the file as read with the current mtime
    let mtime = crate::tools::helper::get_mtime(&file_path).await.unwrap();
    store.record(file_path.clone(), mtime).await;

    let tool = WriteTool::new(store);

    let args = serde_json::json!({
        "file_path": "existing.txt",
        "content": "new content"
    });

    // Should succeed because file was recorded as read
    let ctx = ToolExecCtx::new("test_tool_call", &base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());
    assert!(result.text_content().contains("File updated"));
    assert!(result.text_content().contains("existing.txt"));

    // Verify file was updated
    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "new content");
}

#[tokio::test]
async fn test_write_overwrite_stale_rejected() {
    // Read-first gate, branch 2: a recorded file that was modified externally
    // must be re-read before overwrite.
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().canonicalize().unwrap();

    let file_path = base_path.join("existing.txt");
    tokio::fs::write(&file_path, "original").await.unwrap();

    let store = Arc::new(FileStateStore::new());
    // Simulate a prior read
    let mtime = crate::tools::helper::get_mtime(&file_path).await.unwrap();
    store.record(file_path.clone(), mtime).await;

    // 外部修改（store 未更新）
    // sleep 1.1s 确保 mtime 真的变化（超过秒级粒度文件系统的分辨率）
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    tokio::fs::write(&file_path, "externally modified")
        .await
        .unwrap();

    let tool = WriteTool::new(store);
    let args = serde_json::json!({
        "file_path": "existing.txt",
        "content": "overwrite"
    });
    let ctx = ToolExecCtx::new("test_tool_call", &base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.is_error);
    assert!(result.error_text().contains("modified since it was read"));

    // External content untouched by the rejected overwrite
    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "externally modified");
}

#[tokio::test]
async fn test_write_tool_absolute_path() {
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path();

    let tool = WriteTool::default();
    let absolute_path = base_path.join("absolute.txt");
    let args = serde_json::json!({
        "file_path": absolute_path.to_str().unwrap(),
        "content": "absolute path content"
    });

    let ctx = ToolExecCtx::new("test_tool_call", base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());

    let content = tokio::fs::read_to_string(absolute_path).await.unwrap();
    assert_eq!(content, "absolute path content");
}

#[tokio::test]
async fn test_write_append_does_not_unlock_overwrite() {
    // Appending to an existing, never-read file must not mark it as known,
    // so a subsequent overwrite is still blocked by the read-first gate.
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().canonicalize().unwrap();

    tokio::fs::write(base_path.join("existing.txt"), "original")
        .await
        .unwrap();

    let store = Arc::new(FileStateStore::new());
    let tool = WriteTool::new(store);

    // Append without reading: allowed
    let args = serde_json::json!({
        "file_path": "existing.txt",
        "content": "+more",
        "mode": "append"
    });
    let ctx = ToolExecCtx::new("test_tool_call", &base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());

    // Overwrite without reading: still blocked
    let args = serde_json::json!({
        "file_path": "existing.txt",
        "content": "blind overwrite"
    });
    let ctx = ToolExecCtx::new("test_tool_call_2", &base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.is_error);
    assert!(result.error_text().contains("not been read"));

    // Content untouched by the rejected overwrite
    let content = tokio::fs::read_to_string(base_path.join("existing.txt"))
        .await
        .unwrap();
    assert_eq!(content, "original+more");
}

#[tokio::test]
async fn test_write_append_after_read_then_overwrite() {
    // Appending to a previously-read file refreshes its recorded mtime,
    // so a subsequent overwrite passes the staleness check.
    let temp_dir = TempDir::new().unwrap();
    let base_path = temp_dir.path().canonicalize().unwrap();

    let file_path = base_path.join("existing.txt");
    tokio::fs::write(&file_path, "original").await.unwrap();

    let store = Arc::new(FileStateStore::new());
    // Simulate a prior read
    let mtime = crate::tools::helper::get_mtime(&file_path).await.unwrap();
    store.record(file_path.clone(), mtime).await;

    let tool = WriteTool::new(store);

    let args = serde_json::json!({
        "file_path": "existing.txt",
        "content": "+more",
        "mode": "append"
    });
    let ctx = ToolExecCtx::new("test_tool_call", &base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(result.success());

    let args = serde_json::json!({
        "file_path": "existing.txt",
        "content": "overwritten"
    });
    let ctx = ToolExecCtx::new("test_tool_call_2", &base_path, "test-session");
    let result = tool.exec(args, ctx).await.unwrap();
    assert!(
        result.success(),
        "overwrite after append should succeed, got: {}",
        result.error_text()
    );

    let content = tokio::fs::read_to_string(&file_path).await.unwrap();
    assert_eq!(content, "overwritten");
}

#[tokio::test]
async fn test_write_then_edit_no_need_read() {
    let temp_dir = TempDir::new().unwrap();
    // Use non-canonicalized path to match real-world usage
    let base_path = temp_dir.path().to_path_buf();

    // Create a shared file state store
    let store = Arc::new(crate::tools::helper::FileStateStore::new());

    // Create WriteTool with file state store
    let write_tool = WriteTool::new(Arc::clone(&store));

    // Write a new file
    let args = serde_json::json!({
        "file_path": "test.txt",
        "content": "Hello, World!"
    });
    let ctx = ToolExecCtx::new("test_tool_call", &base_path, "test-session");
    let result = write_tool.exec(args, ctx).await.unwrap();
    assert!(result.success());

    // Now create EditTool with the same file state store
    let edit_tool = crate::tools::edit::EditTool::new(store);

    // Try to edit the file without reading first
    // This should succeed because WriteTool already recorded the file state
    let args = serde_json::json!({
        "path": "test.txt",
        "old_str": "Hello",
        "new_str": "Goodbye"
    });
    let ctx = ToolExecCtx::new("test_tool_call_2", &base_path, "test-session");
    let result = edit_tool.exec(args, ctx).await.unwrap();

    // Should succeed, not fail with "not been read" error
    assert!(
        result.success(),
        "Edit after write should succeed without read first, but got: {}",
        result.error_text()
    );

    // Verify file was edited
    let content = tokio::fs::read_to_string(base_path.join("test.txt"))
        .await
        .unwrap();
    assert_eq!(content, "Goodbye, World!");
}
