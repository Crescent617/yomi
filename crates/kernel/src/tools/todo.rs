use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const TODO_WRITE_TOOL_NAME: &str = "todoWrite";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    #[default]
    Pending,
    InProgress,
    Completed,
}

impl std::fmt::Display for TodoStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::InProgress => write!(f, "in_progress"),
            Self::Completed => write!(f, "completed"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Todo {
    pub id: String,
    pub content: String,
    pub status: TodoStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoList {
    pub todos: Vec<Todo>,
}

/// In-memory store for todos per session
#[derive(Debug, Default)]
pub struct TodoStore {
    sessions: Mutex<HashMap<String, Vec<Todo>>>,
}

impl TodoStore {
    pub fn new() -> Self {
        Self {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    pub fn get_todos(&self, session_id: &str) -> Vec<Todo> {
        let sessions = self.sessions.lock().unwrap();
        sessions.get(session_id).cloned().unwrap_or_default()
    }

    pub fn set_todos(&self, session_id: &str, todos: Vec<Todo>) {
        let mut sessions = self.sessions.lock().unwrap();
        if todos.is_empty() {
            sessions.remove(session_id);
        } else {
            sessions.insert(session_id.to_string(), todos);
        }
    }
}

pub type SharedTodoStore = Arc<TodoStore>;

/// `TodoWriteTool` - Simple in-memory todo list management
/// Replaces the heavier Task system for simple task tracking
pub struct TodoWriteTool {
    store: SharedTodoStore,
    session_id: String,
}

impl TodoWriteTool {
    pub fn new(store: SharedTodoStore, session_id: impl Into<String>) -> Self {
        Self {
            store,
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

        let mut todos = Vec::new();
        for item in todos_array {
            let id = item["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("todo id is required"))?
                .to_string();

            let content = item["content"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("todo content is required"))?
                .to_string();

            let status = match item["status"].as_str() {
                Some("pending") => TodoStatus::Pending,
                Some("in_progress") => TodoStatus::InProgress,
                Some("completed") => TodoStatus::Completed,
                _ => return Err(anyhow::anyhow!("invalid status")),
            };

            let notes = item["notes"].as_str().map(|s| s.to_string());

            todos.push(Todo {
                id,
                content,
                status,
                notes,
            });
        }

        // Check conditions before moving todos
        let all_completed = todos.iter().all(|t| t.status == TodoStatus::Completed);

        // Store new todos (filter out completed if all done - mimics claude-code behavior)
        let todos_to_store: Vec<Todo> = if all_completed {
            Vec::new() // Clear list when all done
        } else {
            todos
        };

        self.store
            .set_todos(&self.session_id, todos_to_store.clone());

        Ok(ToolOutput::text("Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_todo_write_tool() {
        let store = Arc::new(TodoStore::new());
        let tool = TodoWriteTool::new(store.clone(), "test_session");

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
        let result = tool.exec(input, ctx).await.unwrap();

        // Check human-readable success message (claude-code aligned)
        let text = result.text_content();
        assert!(text.contains("Todos have been modified successfully"));

        // Verify storage
        let stored = store.get_todos("test_session");
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].content, "Fix bug");
    }

    #[tokio::test]
    async fn test_verification_nudge_when_all_completed_no_verify() {
        let store = Arc::new(TodoStore::new());
        let tool = TodoWriteTool::new(store.clone(), "test_session");

        // Complete 3+ tasks with no verification step
        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "completed"},
                {"id": "2", "content": "Task 2", "status": "completed"},
                {"id": "3", "content": "Task 3", "status": "completed"}
            ]
        });

        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = tool.exec(input, ctx).await.unwrap();

        let text = result.text_content();
        assert!(text.contains("verification step"));
    }

    #[tokio::test]
    async fn test_no_nudge_when_has_verification_task() {
        let store = Arc::new(TodoStore::new());
        let tool = TodoWriteTool::new(store.clone(), "test_session");

        // Complete 3+ tasks but one is verification
        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "completed"},
                {"id": "2", "content": "Verify implementation", "status": "completed"},
                {"id": "3", "content": "Task 3", "status": "completed"}
            ]
        });

        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = tool.exec(input, ctx).await.unwrap();

        let text = result.text_content();
        assert!(!text.contains("verification step"));
    }

    #[tokio::test]
    async fn test_all_completed_clears_list() {
        let store = Arc::new(TodoStore::new());
        let tool = TodoWriteTool::new(store.clone(), "test_session");

        // Add some todos first
        let input = json!({
            "todos": [
                {"id": "1", "content": "Task 1", "status": "completed"},
                {"id": "2", "content": "Task 2", "status": "completed"}
            ]
        });

        let ctx = ToolExecCtx::new("test", "/tmp");
        tool.exec(input, ctx).await.unwrap();

        // List should be cleared when all completed
        let stored = store.get_todos("test_session");
        assert!(stored.is_empty());
    }
}
