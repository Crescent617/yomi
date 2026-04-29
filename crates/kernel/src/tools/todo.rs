use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub const TODO_WRITE_TOOL_NAME: &str = "todoWrite";

/// `TodoWriteTool` - Simple todo list management tool
/// Stateless tool that receives todo list and confirms receipt
pub struct TodoWriteTool;

impl TodoWriteTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for TodoWriteTool {
    fn default() -> Self {
        Self::new()
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

        Ok(ToolOutput::text("Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_todo_write_tool() {
        let tool = TodoWriteTool::new();

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

        // Check success message
        let text = result.text_content();
        assert!(text.contains("Todos have been modified successfully"));
    }

    #[tokio::test]
    async fn test_todo_write_tool_empty_list() {
        let tool = TodoWriteTool::new();

        let input = json!({
            "todos": []
        });

        let ctx = ToolExecCtx::new("test", "/tmp");
        let result = tool.exec(input, ctx).await.unwrap();

        let text = result.text_content();
        assert!(text.contains("Todos have been modified successfully"));
    }

    #[tokio::test]
    async fn test_todo_write_tool_invalid_status() {
        let tool = TodoWriteTool::new();

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
        let tool = TodoWriteTool::new();

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
        let tool = TodoWriteTool::new();

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
}
