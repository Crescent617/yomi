use crate::storage::TodoStore;
use crate::tools::helper::g_lock::g_lock;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub const TODO_WRITE_TOOL_NAME: &str = "todoWrite";
pub const TODO_READ_TOOL_NAME: &str = "todoRead";
pub const TODO_UPDATE_TOOL_NAME: &str = "todoUpdate";

/// `TodoWriteTool` - Simple todo list management tool
/// Persists todo list to file for persistence and TUI display
pub struct TodoWriteTool {
    storage: Arc<dyn TodoStore>,
}

impl TodoWriteTool {
    pub fn new(storage: Arc<dyn TodoStore>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        TODO_WRITE_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        r"Write todo list for tracking tasks.
When to use:
- Tasks with 3+ distinct steps
- User provides multiple tasks or a list of things to do
- Complex refactoring or feature implementation
- Rewrite entire todo list

Guidelines:
- Include clear, actionable task descriptions
- Skip for trivial single-step tasks"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The complete todo list to replace the current list",
                    "items": {
                        "type": "object",
                        "properties": {
                            "id": {
                                "type": "string",
                                "description": "Unique identifier for this todo item"
                            },
                            "content": {
                                "type": "string",
                                "description": "The task description"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Current status of the task"
                            },
                            "notes": {
                                "type": "string",
                                "description": "Optional additional notes"
                            }
                        },
                        "required": ["id", "content", "status"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        // Lock on session_id to prevent concurrent todo modifications
        let _lock = g_lock(format!("todo-{}", ctx.session_id)).await;

        let todos_array = args["todos"]
            .as_array()
            .ok_or_else(|| KernelError::tool("todos must be an array"))?;

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
            let json_str = serde_json::to_string(&args)?;
            self.storage.save(&ctx.session_id, &json_str).await?;
        }

        Ok(ToolOutput::text("Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with your current tasks if applicable"))
    }
}

/// `TodoReadTool` - Read the current todo list
/// Returns the current todo list from storage
pub struct TodoReadTool {
    storage: Arc<dyn TodoStore>,
}

impl TodoReadTool {
    pub fn new(storage: Arc<dyn TodoStore>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl Tool for TodoReadTool {
    fn name(&self) -> &str {
        TODO_READ_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        r"Read the current todo list. Use this when lost track of your tasks or want to review the list"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn exec(&self, _args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        // Load todo list from storage
        match self.storage.load(&ctx.session_id).await? {
            Some(json_str) => Ok(ToolOutput::text(json_str)),
            None => Ok(ToolOutput::text(r#"{"todos": []}"#)),
        }
    }
}

/// `TodoUpdateTool` - Update a single todo item by id
/// Allows partial updates to status and/or content
pub struct TodoUpdateTool {
    storage: Arc<dyn TodoStore>,
}

impl TodoUpdateTool {
    pub fn new(storage: Arc<dyn TodoStore>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl Tool for TodoUpdateTool {
    fn name(&self) -> &str {
        TODO_UPDATE_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        r"Update a single existing todo item by id. Supports partial updates - only provided fields are modified.
When to use:
- Mark task as `in_progress` BEFORE starting work on them
- Mark task as `completed` IMMEDIATELY after finishing"
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Unique identifier of the todo item to update"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed"],
                    "description": "New status for the todo (optional)"
                },
                "content": {
                    "type": "string",
                    "description": "New content for the todo (optional)"
                },
                "notes": {
                    "type": "string",
                    "description": "New notes for the todo (optional)"
                }
            },
            "required": ["id"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        // Lock on session_id to prevent concurrent todo modifications
        let _lock = g_lock(format!("todo-{}", ctx.session_id)).await;

        let id = args["id"]
            .as_str()
            .ok_or_else(|| KernelError::tool("id is required"))?;

        // Load current todos
        let mut todos: Value = match self.storage.load(&ctx.session_id).await? {
            Some(json_str) => serde_json::from_str(&json_str)?,
            None => json!({"todos": []}),
        };

        let todos_array = todos["todos"]
            .as_array_mut()
            .ok_or_else(|| KernelError::tool("invalid todos format"))?;

        // Find and update the todo, keeping reference to the updated todo
        let mut updated_todo: Option<&Value> = None;
        for todo in todos_array.iter_mut() {
            if todo["id"].as_str() == Some(id) {
                // Update status if provided
                if let Some(status) = args["status"].as_str() {
                    match status {
                        "pending" | "in_progress" | "completed" => {
                            todo["status"] = json!(status);
                        }
                        _ => return Err(KernelError::tool("invalid status")),
                    }
                }
                // Update content if provided
                if let Some(content) = args["content"].as_str() {
                    todo["content"] = json!(content);
                }
                // Update notes if provided
                if args.get("notes").is_some() {
                    if let Some(notes) = args["notes"].as_str() {
                        todo["notes"] = json!(notes);
                    } else if args["notes"].is_null() {
                        todo.as_object_mut().unwrap().remove("notes");
                    }
                }
                updated_todo = Some(todo);
                break;
            }
        }

        let updated = updated_todo
            .ok_or_else(|| KernelError::tool(format!("todo with id '{id}' not found")))?;

        // Clone the updated todo before saving
        let result = updated.to_string();

        // Save updated todos
        let json_str = serde_json::to_string(&todos)?;
        self.storage.save(&ctx.session_id, &json_str).await?;

        // Return the complete updated todo entry
        Ok(ToolOutput::text(result))
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
    async fn test_todo_write_tool() {
        let (storage, _temp) = create_test_storage().await;
        let tool = TodoWriteTool::new(storage.clone());

        let input = json!({
            "todos": [
                {
                    "id": "1",
                    "content": "Fix bug",
                    "status": "pending"
                },
                {
                    "id": "2",
                    "content": "Write tests",
                    "status": "in_progress"
                }
            ]
        });

        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = tool.exec(input.clone(), ctx).await.unwrap();

        // Check success message
        let text = result.text_content();
        assert!(text.contains("Todos have been modified successfully"));

        // Verify file was saved
        let loaded = storage.load("test-session").await.unwrap().unwrap();
        let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
        assert_eq!(loaded_json, input);
    }

    #[tokio::test]
    async fn test_todo_write_tool_empty_list_clears() {
        let (storage, _temp) = create_test_storage().await;
        let tool = TodoWriteTool::new(storage.clone());

        // First add some todos
        let input1 = json!({
            "todos": [{"id": "1", "content": "Task 1", "status": "pending"}]
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        tool.exec(input1, ctx).await.unwrap();
        assert!(storage.load("test-session").await.unwrap().is_some());

        // Then clear with empty list - should delete the file
        let input2 = json!({ "todos": [] });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = tool.exec(input2, ctx).await.unwrap();

        let text = result.text_content();
        assert!(text.contains("Todos have been modified successfully"));
        // Verify file was deleted
        assert!(storage.load("test-session").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_todo_write_tool_invalid_status() {
        let (storage, _temp) = create_test_storage().await;
        let tool = TodoWriteTool::new(storage);

        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "invalid_status"}
            ]
        });

        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = tool.exec(input, ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid status"));
    }

    #[tokio::test]
    async fn test_todo_write_tool_missing_id() {
        let (storage, _temp) = create_test_storage().await;
        let tool = TodoWriteTool::new(storage);

        let input = json!({
            "todos": [
                {"content": "Task 1", "status": "pending"}
            ]
        });

        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = tool.exec(input, ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("todo id is required"));
    }

    #[tokio::test]
    async fn test_todo_write_tool_missing_content() {
        let (storage, _temp) = create_test_storage().await;
        let tool = TodoWriteTool::new(storage);

        let input = json!({
            "todos": [
                {"id": "1", "status": "pending"}
            ]
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
    async fn test_todo_read_tool_with_data() {
        let (storage, _temp) = create_test_storage().await;

        // First write some todos
        let write_tool = TodoWriteTool::new(storage.clone());
        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "pending"},
                {"id": "2", "content": "Task 2", "status": "in_progress"}
            ]
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        write_tool.exec(input.clone(), ctx).await.unwrap();

        // Then read them back
        let read_tool = TodoReadTool::new(storage);
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = read_tool.exec(json!({}), ctx).await.unwrap();

        let text = result.text_content();
        let result_json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(result_json["todos"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_todo_read_tool_empty() {
        let (storage, _temp) = create_test_storage().await;
        let tool = TodoReadTool::new(storage);

        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = tool.exec(json!({}), ctx).await.unwrap();

        let text = result.text_content();
        assert_eq!(text, r#"{"todos": []}"#);
    }

    #[tokio::test]
    async fn test_todo_update_tool_status() {
        let (storage, _temp) = create_test_storage().await;

        // First create some todos
        let write_tool = TodoWriteTool::new(storage.clone());
        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "pending"},
                {"id": "2", "content": "Task 2", "status": "in_progress"}
            ]
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        write_tool.exec(input, ctx).await.unwrap();

        // Update status of todo "1"
        let update_tool = TodoUpdateTool::new(storage.clone());
        let update = json!({
            "id": "1",
            "status": "completed"
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = update_tool.exec(update, ctx).await.unwrap();

        // Verify result returns the complete updated todo
        let result_json: Value = serde_json::from_str(&result.text_content()).unwrap();
        assert_eq!(result_json["id"], "1");
        assert_eq!(result_json["status"], "completed");
        assert_eq!(result_json["content"], "Task 1");

        // Verify the update in storage
        let loaded = storage.load("test-session").await.unwrap().unwrap();
        let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
        let todos = loaded_json["todos"].as_array().unwrap();
        assert_eq!(todos[0]["status"], "completed");
        assert_eq!(todos[0]["content"], "Task 1"); // content unchanged
        assert_eq!(todos[1]["status"], "in_progress"); // other todo unchanged
    }

    #[tokio::test]
    async fn test_todo_update_tool_content() {
        let (storage, _temp) = create_test_storage().await;

        // First create a todo
        let write_tool = TodoWriteTool::new(storage.clone());
        let input = json!({
            "todos": [
                {"id": "1", "content": "Old content", "status": "pending"}
            ]
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        write_tool.exec(input, ctx).await.unwrap();

        // Update content
        let update_tool = TodoUpdateTool::new(storage.clone());
        let update = json!({
            "id": "1",
            "content": "New content"
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = update_tool.exec(update, ctx).await.unwrap();

        // Verify result returns the complete updated todo
        let result_json: Value = serde_json::from_str(&result.text_content()).unwrap();
        assert_eq!(result_json["id"], "1");
        assert_eq!(result_json["content"], "New content");
        assert_eq!(result_json["status"], "pending"); // status unchanged

        // Verify the update in storage
        let loaded = storage.load("test-session").await.unwrap().unwrap();
        let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
        let todos = loaded_json["todos"].as_array().unwrap();
        assert_eq!(todos[0]["content"], "New content");
        assert_eq!(todos[0]["status"], "pending"); // status unchanged
    }

    #[tokio::test]
    async fn test_todo_update_tool_both() {
        let (storage, _temp) = create_test_storage().await;

        // First create a todo
        let write_tool = TodoWriteTool::new(storage.clone());
        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "pending"}
            ]
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        write_tool.exec(input, ctx).await.unwrap();

        // Update both status and content
        let update_tool = TodoUpdateTool::new(storage.clone());
        let update = json!({
            "id": "1",
            "status": "in_progress",
            "content": "Updated task"
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = update_tool.exec(update, ctx).await.unwrap();

        // Verify result returns the complete updated todo
        let result_json: Value = serde_json::from_str(&result.text_content()).unwrap();
        assert_eq!(result_json["id"], "1");
        assert_eq!(result_json["status"], "in_progress");
        assert_eq!(result_json["content"], "Updated task");

        // Verify the update in storage
        let loaded = storage.load("test-session").await.unwrap().unwrap();
        let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
        let todos = loaded_json["todos"].as_array().unwrap();
        assert_eq!(todos[0]["status"], "in_progress");
        assert_eq!(todos[0]["content"], "Updated task");
    }

    #[tokio::test]
    async fn test_todo_update_tool_notes() {
        let (storage, _temp) = create_test_storage().await;

        // First create a todo with notes
        let write_tool = TodoWriteTool::new(storage.clone());
        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "pending", "notes": "Initial note"}
            ]
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        write_tool.exec(input, ctx).await.unwrap();

        // Update notes
        let update_tool = TodoUpdateTool::new(storage.clone());
        let update = json!({
            "id": "1",
            "notes": "Updated note"
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = update_tool.exec(update, ctx).await.unwrap();

        // Verify result returns the complete updated todo
        let result_json: Value = serde_json::from_str(&result.text_content()).unwrap();
        assert_eq!(result_json["id"], "1");
        assert_eq!(result_json["notes"], "Updated note");
        assert_eq!(result_json["status"], "pending"); // unchanged
        assert_eq!(result_json["content"], "Task 1"); // unchanged

        // Verify the update in storage
        let loaded = storage.load("test-session").await.unwrap().unwrap();
        let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
        let todos = loaded_json["todos"].as_array().unwrap();
        assert_eq!(todos[0]["notes"], "Updated note");
    }

    #[tokio::test]
    async fn test_todo_update_tool_notes_null() {
        let (storage, _temp) = create_test_storage().await;

        // First create a todo with notes
        let write_tool = TodoWriteTool::new(storage.clone());
        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "pending", "notes": "Some note"}
            ]
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        write_tool.exec(input, ctx).await.unwrap();

        // Remove notes by setting to null
        let update_tool = TodoUpdateTool::new(storage.clone());
        let update = json!({
            "id": "1",
            "notes": null
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = update_tool.exec(update, ctx).await.unwrap();

        // Verify result returns the updated todo without notes
        let result_json: Value = serde_json::from_str(&result.text_content()).unwrap();
        assert_eq!(result_json["id"], "1");
        assert!(!result_json.as_object().unwrap().contains_key("notes"));

        // Verify the update in storage
        let loaded = storage.load("test-session").await.unwrap().unwrap();
        let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
        let todos = loaded_json["todos"].as_array().unwrap();
        assert!(!todos[0].as_object().unwrap().contains_key("notes"));
    }

    #[tokio::test]
    async fn test_todo_update_tool_not_found() {
        let (storage, _temp) = create_test_storage().await;

        // Create a todo
        let write_tool = TodoWriteTool::new(storage.clone());
        let input = json!({
            "todos": [{"id": "1", "content": "Task 1", "status": "pending"}]
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        write_tool.exec(input, ctx).await.unwrap();

        // Try to update non-existent todo
        let update_tool = TodoUpdateTool::new(storage);
        let update = json!({
            "id": "999",
            "status": "completed"
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = update_tool.exec(update, ctx).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_todo_update_tool_invalid_status() {
        let (storage, _temp) = create_test_storage().await;

        // Create a todo
        let write_tool = TodoWriteTool::new(storage.clone());
        let input = json!({
            "todos": [{"id": "1", "content": "Task 1", "status": "pending"}]
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        write_tool.exec(input, ctx).await.unwrap();

        // Try to update with invalid status
        let update_tool = TodoUpdateTool::new(storage);
        let update = json!({
            "id": "1",
            "status": "invalid_status"
        });
        let ctx = ToolExecCtx::new("test", "/tmp", "test-session");
        let result = update_tool.exec(update, ctx).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid status"));
    }
}
