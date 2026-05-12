use crate::storage::TodoStore;
use crate::tools::helper::g_lock::g_lock;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub const TODO_TOOL_NAME: &str = "todo";

/// `TodoTool` - Unified todo list management tool
/// Supports read, write (full replace), and update (partial batch) operations
pub struct TodoTool {
    storage: Arc<dyn TodoStore>,
}

impl TodoTool {
    pub fn new(storage: Arc<dyn TodoStore>) -> Self {
        Self { storage }
    }

    /// Handle read action
    async fn handle_read(&self, ctx: &ToolExecCtx<'_>) -> Result<ToolOutput> {
        match self.storage.load(&ctx.session_id).await? {
            Some(json_str) => Ok(ToolOutput::text(json_str)),
            None => Ok(ToolOutput::text(r#"{"todos": []}"#)),
        }
    }

    /// Handle write action (full replace)
    async fn handle_write(
        &self,
        todos_array: &[Value],
        ctx: &ToolExecCtx<'_>,
    ) -> Result<ToolOutput> {
        // Lock on session_id to prevent concurrent todo modifications
        let _lock = g_lock(format!("todo-{}", ctx.session_id)).await;

        // Validate todo items
        for item in todos_array {
            if item["id"].as_str().is_none() {
                return Err(KernelError::tool("todo id is required"));
            }
            if item["content"].as_str().is_none() {
                return Err(KernelError::tool("todo content is required"));
            }
            match item["status"].as_str() {
                Some("pending" | "in_progress" | "completed") => {}
                _ => return Err(KernelError::tool("invalid status")),
            }
        }

        // Persist to file (delete if empty)
        if todos_array.is_empty() {
            self.storage.clear(&ctx.session_id).await?;
        } else {
            let data = json!({ "todos": todos_array });
            let json_str = serde_json::to_string(&data)?;
            self.storage.save(&ctx.session_id, &json_str).await?;
        }

        Ok(ToolOutput::text("Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with your current tasks if applicable"))
    }

    /// Handle update action (partial batch update)
    async fn handle_update(
        &self,
        updates_array: &[Value],
        ctx: &ToolExecCtx<'_>,
    ) -> Result<ToolOutput> {
        // Lock on session_id to prevent concurrent todo modifications
        let _lock = g_lock(format!("todo-{}", ctx.session_id)).await;

        // Load current todos
        let mut todos: Value = match self.storage.load(&ctx.session_id).await? {
            Some(json_str) => serde_json::from_str(&json_str)?,
            None => json!({"todos": []}),
        };

        let todos_array = todos["todos"]
            .as_array_mut()
            .ok_or_else(|| KernelError::tool("invalid todos format"))?;

        let mut updated_todos = Vec::new();

        for update in updates_array {
            let id = update["id"]
                .as_str()
                .ok_or_else(|| KernelError::tool("update item must have id"))?;

            // Find and update the todo
            let mut found = false;
            for todo in todos_array.iter_mut() {
                if todo["id"].as_str() == Some(id) {
                    // Update status if provided
                    if let Some(status) = update["status"].as_str() {
                        match status {
                            "pending" | "in_progress" | "completed" => {
                                todo["status"] = json!(status);
                            }
                            _ => return Err(KernelError::tool("invalid status")),
                        }
                    }
                    // Update content if provided
                    if let Some(content) = update["content"].as_str() {
                        todo["content"] = json!(content);
                    }
                    // Update notes if provided
                    if update.get("notes").is_some() {
                        if let Some(notes) = update["notes"].as_str() {
                            todo["notes"] = json!(notes);
                        } else if update["notes"].is_null() {
                            todo.as_object_mut().unwrap().remove("notes");
                        }
                    }
                    updated_todos.push(todo.clone());
                    found = true;
                    break;
                }
            }

            if !found {
                return Err(KernelError::tool(format!("todo with id '{id}' not found")));
            }
        }

        // Save updated todos
        let json_str = serde_json::to_string(&todos)?;
        self.storage.save(&ctx.session_id, &json_str).await?;

        // Return all updated todos
        let result = json!({ "updated": updated_todos });
        Ok(ToolOutput::text(result.to_string()))
    }
}

#[async_trait]
impl Tool for TodoTool {
    fn name(&self) -> &str {
        TODO_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        r"Manage todo list for tracking tasks.
When to use:
- Tasks with 3+ distinct steps
- User provides multiple tasks or a list of things to do
- Complex refactoring or feature implementation

Guidelines:
- Include clear, actionable task descriptions
- Mark task as `in_progress` by action `update` BEFORE starting work on them
- Mark task as `completed` by action `update` IMMEDIATELY after finishing
- Skip for trivial single-step tasks"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write", "update"],
                    "description": "Operation type: read - get current list, write - full replace (needs id/content/status), update - batch partial update (only id required)"
                },
                "todos": {
                    "type": "array",
                    "description": "Required for write/update. For write: full todo list with all fields. For update: items with id + fields to change",
                    "items": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for this todo item"
                            },
                            "content": {
                                "type": "string",
                                "description": "The task description (required for write)"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Current status (required for write)"
                            },
                            "notes": {
                                "type": "string",
                                "description": "Optional additional notes"
                            }
                        }
                    }
                }
            }
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| KernelError::tool("action is required"))?;

        match action {
            "read" => self.handle_read(&ctx).await,
            "write" => {
                let todos = args["todos"]
                    .as_array()
                    .ok_or_else(|| KernelError::tool("todos array is required for write"))?;
                self.handle_write(todos, &ctx).await
            }
            "update" => {
                let todos = args["todos"]
                    .as_array()
                    .ok_or_else(|| KernelError::tool("todos array is required for update"))?;
                self.handle_update(todos, &ctx).await
            }
            _ => Err(KernelError::tool(format!("unknown action: {action}"))),
        }
    }
}

#[cfg(test)]
mod tests {
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
}
