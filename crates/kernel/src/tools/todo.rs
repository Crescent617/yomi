use crate::storage::TodoStorage;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub const TODO_WRITE_TOOL_NAME: &str = "todoWrite";
pub const TODO_READ_TOOL_NAME: &str = "todoRead";

/// `TodoWriteTool` - Simple todo list management tool
/// Persists todo list to file for persistence and TUI display
pub struct TodoWriteTool {
    storage: Arc<TodoStorage>,
    session_id: String,
}

impl TodoWriteTool {
    pub fn new(storage: Arc<TodoStorage>, session_id: impl Into<String>) -> Self {
        Self {
            storage,
            session_id: session_id.into(),
        }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        TODO_WRITE_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        r"Manage a todo list for tracking tasks.
When to use it:
- Tasks with 3+ distinct steps
- User provides multiple tasks or a list of things to do
- Complex refactoring or feature implementation

Guidelines:
- Mark tasks as `in_progress` BEFORE starting work on them
- Mark tasks as `completed` IMMEDIATELY after finishing
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

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let todos_array = args["todos"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("todos must be an array"))?;

        // Validate todo items
        for item in todos_array {
            if item["id"].as_str().is_none() {
                return Err(anyhow::anyhow!("todo id is required"));
            }
            if item["content"].as_str().is_none() {
                return Err(anyhow::anyhow!("todo content is required"));
            }
            match item["status"].as_str() {
                Some("pending" | "in_progress" | "completed") => {}
                _ => return Err(anyhow::anyhow!("invalid status")),
            }
        }

        // Persist to file (delete if empty)
        if todos_array.is_empty() {
            self.storage.clear(&self.session_id)?;
        } else {
            let json_str = serde_json::to_string(&args)?;
            self.storage.save(&self.session_id, &json_str)?;
        }

        Ok(ToolOutput::text("Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable"))
    }
}

/// `TodoReadTool` - Read the current todo list
/// Returns the current todo list from storage
pub struct TodoReadTool {
    storage: Arc<TodoStorage>,
    session_id: String,
}

impl TodoReadTool {
    pub fn new(storage: Arc<TodoStorage>, session_id: impl Into<String>) -> Self {
        Self {
            storage,
            session_id: session_id.into(),
        }
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

    async fn exec(&self, _args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        // Load todo list from storage
        match self.storage.load(&self.session_id) {
            Some(json_str) => Ok(ToolOutput::text(json_str)),
            None => Ok(ToolOutput::text(r#"{"todos": []}"#)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_todo_write_tool() {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(TodoStorage::new(temp_dir.path()));
        let tool = TodoWriteTool::new(storage.clone(), "test-session");

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

        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = tool.exec(input.clone(), ctx).await.unwrap();

        // Check success message
        let text = result.text_content();
        assert!(text.contains("Todos have been modified successfully"));

        // Verify file was saved
        let loaded = storage.load("test-session").unwrap();
        let loaded_json: Value = serde_json::from_str(&loaded).unwrap();
        assert_eq!(loaded_json, input);
    }

    #[tokio::test]
    async fn test_todo_write_tool_empty_list_deletes_file() {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(TodoStorage::new(temp_dir.path()));
        let tool = TodoWriteTool::new(storage.clone(), "test-session");

        // First add some todos
        let input1 = json!({
            "todos": [{"id": "1", "content": "Task 1", "status": "pending"}]
        });
        let ctx = ToolExecCtx::new("test", "/tmp");
        tool.exec(input1, ctx).await.unwrap();
        assert!(storage.exists("test-session"));

        // Then clear with empty list - should delete the file
        let input2 = json!({ "todos": [] });
        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = tool.exec(input2, ctx).await.unwrap();

        let text = result.text_content();
        assert!(text.contains("Todos have been modified successfully"));
        // Verify file was deleted
        assert!(!storage.exists("test-session"));
        assert!(storage.load("test-session").is_none());
    }

    #[tokio::test]
    async fn test_todo_write_tool_invalid_status() {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(TodoStorage::new(temp_dir.path()));
        let tool = TodoWriteTool::new(storage, "test-session");

        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "invalid_status"}
            ]
        });

        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = tool.exec(input, ctx).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("invalid status"));
    }

    #[tokio::test]
    async fn test_todo_write_tool_missing_id() {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(TodoStorage::new(temp_dir.path()));
        let tool = TodoWriteTool::new(storage, "test-session");

        let input = json!({
            "todos": [
                {"content": "Task 1", "status": "pending"}
            ]
        });

        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = tool.exec(input, ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("todo id is required"));
    }

    #[tokio::test]
    async fn test_todo_write_tool_missing_content() {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(TodoStorage::new(temp_dir.path()));
        let tool = TodoWriteTool::new(storage, "test-session");

        let input = json!({
            "todos": [
                {"id": "1", "status": "pending"}
            ]
        });

        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = tool.exec(input, ctx).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("todo content is required"));
    }

    #[tokio::test]
    async fn test_todo_read_tool_with_data() {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(TodoStorage::new(temp_dir.path()));

        // First write some todos
        let write_tool = TodoWriteTool::new(storage.clone(), "test-session");
        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "pending"},
                {"id": "2", "content": "Task 2", "status": "in_progress"}
            ]
        });
        let ctx = ToolExecCtx::new("test", "/tmp");
        write_tool.exec(input.clone(), ctx).await.unwrap();

        // Then read them back
        let read_tool = TodoReadTool::new(storage, "test-session");
        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = read_tool.exec(json!({}), ctx).await.unwrap();

        let text = result.text_content();
        let result_json: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(result_json["todos"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_todo_read_tool_empty() {
        let temp_dir = TempDir::new().unwrap();
        let storage = Arc::new(TodoStorage::new(temp_dir.path()));
        let tool = TodoReadTool::new(storage, "test-session");

        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = tool.exec(json!({}), ctx).await.unwrap();

        let text = result.text_content();
        assert_eq!(text, r#"{"todos": []}"#);
    }
}
