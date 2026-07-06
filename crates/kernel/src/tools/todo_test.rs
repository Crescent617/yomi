use super::*;

use crate::storage::JsonTodoStore;
use tempfile::TempDir;

async fn create_test_storage() -> (Arc<dyn TodoStore>, TempDir) {
    let temp = TempDir::new().unwrap();
    let store: Arc<dyn TodoStore> = Arc::new(JsonTodoStore::new(temp.path()));
    (store, temp)
}

#[tokio::test]
async fn test_todo_read_empty() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage);

    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(json!({"action": "read"}), ctx).await.unwrap();

    assert_eq!(result.text_content(), r#"{"todos": []}"#);
}

#[tokio::test]
async fn test_todo_write_and_read() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage.clone());

    // Write todos
    let write_input = json!({
        "action": "write",
        "todos": [
            {"id": "1", "content": "Task 1", "status": "pending"},
            {"id": "2", "content": "Task 2", "status": "in_progress"}
        ]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(write_input, ctx).await.unwrap();
    assert!(result.text_content().contains("modified successfully"));

    // Read them back
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(json!({"action": "read"}), ctx).await.unwrap();

    let result_json: Value = serde_json::from_str(&result.text_content()).unwrap();
    assert_eq!(result_json["todos"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_todo_write_empty_clears() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage.clone());

    // First add some todos
    let input1 = json!({
        "action": "write",
        "todos": [{"id": "1", "content": "Task 1", "status": "pending"}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    tool.exec(input1, ctx).await.unwrap();
    assert!(storage.load("test-session").await.unwrap().is_some());

    // Then clear with empty list
    let input2 = json!({"action": "write", "todos": []});
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    tool.exec(input2, ctx).await.unwrap();

    // Verify file was deleted
    assert!(storage.load("test-session").await.unwrap().is_none());
}

#[tokio::test]
async fn test_todo_write_missing_content() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage);

    let input = json!({
        "action": "write",
        "todos": [{"id": "1", "status": "pending"}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(input, ctx).await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("todo content is required"));
}

#[tokio::test]
async fn test_todo_write_invalid_status() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage);

    let input = json!({
        "action": "write",
        "todos": [{"id": "1", "content": "Task 1", "status": "invalid"}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(input, ctx).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("invalid status"));
}

#[tokio::test]
async fn test_todo_update_single_status() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage.clone());

    // First write some todos
    let write_input = json!({
        "action": "write",
        "todos": [
            {"id": "1", "content": "Task 1", "status": "pending"},
            {"id": "2", "content": "Task 2", "status": "in_progress"}
        ]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    tool.exec(write_input, ctx).await.unwrap();

    // Update status of todo "1"
    let update_input = json!({
        "action": "update",
        "todos": [{"id": "1", "status": "completed"}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(update_input, ctx).await.unwrap();

    let result_json: Value = serde_json::from_str(&result.text_content()).unwrap();
    let updated = result_json["updated"].as_array().unwrap();
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0]["status"], "completed");
    assert_eq!(updated[0]["content"], "Task 1");

    // Verify storage
    let loaded = storage.load("test-session").await.unwrap().unwrap();
    let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
    let todos = loaded_json["todos"].as_array().unwrap();
    assert_eq!(todos[0]["status"], "completed");
    assert_eq!(todos[1]["status"], "in_progress");
}

#[tokio::test]
async fn test_todo_update_batch_multiple() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage.clone());

    // First write some todos
    let write_input = json!({
        "action": "write",
        "todos": [
            {"id": "1", "content": "Task 1", "status": "pending"},
            {"id": "2", "content": "Task 2", "status": "pending"},
            {"id": "3", "content": "Task 3", "status": "pending"}
        ]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    tool.exec(write_input, ctx).await.unwrap();

    // Batch update multiple todos
    let update_input = json!({
        "action": "update",
        "todos": [
            {"id": "1", "status": "in_progress"},
            {"id": "2", "status": "completed"}
        ]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(update_input, ctx).await.unwrap();

    let result_json: Value = serde_json::from_str(&result.text_content()).unwrap();
    let updated = result_json["updated"].as_array().unwrap();
    assert_eq!(updated.len(), 2);

    // Verify storage
    let loaded = storage.load("test-session").await.unwrap().unwrap();
    let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
    let todos = loaded_json["todos"].as_array().unwrap();
    assert_eq!(todos[0]["status"], "in_progress");
    assert_eq!(todos[1]["status"], "completed");
    assert_eq!(todos[2]["status"], "pending");
}

#[tokio::test]
async fn test_todo_update_content_and_notes() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage.clone());

    // First write a todo
    let write_input = json!({
        "action": "write",
        "todos": [{"id": "1", "content": "Task 1", "status": "pending", "notes": "Original note"}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    tool.exec(write_input, ctx).await.unwrap();

    // Update content and remove notes
    let update_input = json!({
        "action": "update",
        "todos": [{"id": "1", "content": "Updated Task", "notes": null}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(update_input, ctx).await.unwrap();

    let result_json: Value = serde_json::from_str(&result.text_content()).unwrap();
    let updated = result_json["updated"].as_array().unwrap();
    assert_eq!(updated[0]["content"], "Updated Task");
    assert!(updated[0]["notes"].is_null());
    assert_eq!(updated[0]["status"], "pending"); // unchanged

    // Verify storage
    let loaded = storage.load("test-session").await.unwrap().unwrap();
    let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
    let todos = loaded_json["todos"].as_array().unwrap();
    assert_eq!(todos[0]["content"], "Updated Task");
    assert!(todos[0].get("notes").is_none());
}

#[tokio::test]
async fn test_todo_update_not_found() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage.clone());

    // First write a todo
    let write_input = json!({
        "action": "write",
        "todos": [{"id": "1", "content": "Task 1", "status": "pending"}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    tool.exec(write_input, ctx).await.unwrap();

    // Try to update non-existent todo
    let update_input = json!({
        "action": "update",
        "todos": [{"id": "999", "status": "completed"}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(update_input, ctx).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not found"));
}

#[tokio::test]
async fn test_todo_update_invalid_status() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage.clone());

    // First write a todo
    let write_input = json!({
        "action": "write",
        "todos": [{"id": "1", "content": "Task 1", "status": "pending"}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    tool.exec(write_input, ctx).await.unwrap();

    // Try to update with invalid status
    let update_input = json!({
        "action": "update",
        "todos": [{"id": "1", "status": "invalid_status"}]
    });
    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(update_input, ctx).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("invalid status"));
}

#[tokio::test]
async fn test_todo_missing_action() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage);

    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(json!({}), ctx).await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("action is required"));
}

#[tokio::test]
async fn test_todo_missing_todos_for_write() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage);

    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(json!({"action": "write"}), ctx).await;

    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("todos array is required"));
}

#[tokio::test]
async fn test_todo_unknown_action() {
    let (storage, _temp) = create_test_storage().await;
    let tool = TodoTool::new(storage);

    let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
    let result = tool.exec(json!({"action": "delete"}), ctx).await;

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("unknown action"));
}
