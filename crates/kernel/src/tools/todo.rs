use crate::storage::TodoStore;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use crate::utils::g_lock::{g_lock_timeout, DEFAULT_LOCK_TIMEOUT};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub const TODO_TOOL_NAME: &str = "todo";

/// Hard cap on todo items per write/update call. Guards against runaway
/// generation (degenerate models repeating array elements) and keeps the
/// list at a size the model can reliably re-emit.
const MAX_TODO_ITEMS: usize = 50;

fn check_max_items(items: &[Value]) -> Result<()> {
    if items.len() > MAX_TODO_ITEMS {
        return Err(KernelError::tool(format!(
            "too many todo items: {} exceeds max {MAX_TODO_ITEMS}; \
             keep the list focused — drop completed items or split the work into phases",
            items.len()
        )));
    }
    Ok(())
}

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
        let _lock =
            g_lock_timeout(format!("todo-{}", ctx.session_id), DEFAULT_LOCK_TIMEOUT).await?;

        check_max_items(todos_array)?;

        // Validate todo items
        if todos_array.iter().any(|item| {
            item["id"].as_u64().is_none()
                || item["content"].as_str().is_none_or(|s| s.trim().is_empty())
        }) {
            return Err(KernelError::tool(
                "todo id and non-empty content are required".to_string(),
            ));
        }
        if todos_array.iter().any(|item| {
            !matches!(
                item["status"].as_str(),
                Some("pending" | "in_progress" | "completed")
            )
        }) {
            return Err(KernelError::tool("invalid status".to_string()));
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
        let _lock =
            g_lock_timeout(format!("todo-{}", ctx.session_id), DEFAULT_LOCK_TIMEOUT).await?;

        check_max_items(updates_array)?;

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
                .as_u64()
                .ok_or_else(|| KernelError::tool("update item must have numeric id"))?;

            // Find and update the todo
            let mut found = false;
            for todo in todos_array.iter_mut() {
                if todo["id"].as_u64() == Some(id) {
                    // Update status if provided
                    if let Some(status) = update["status"].as_str().filter(|s| !s.is_empty()) {
                        match status {
                            "pending" | "in_progress" | "completed" => {
                                todo["status"] = json!(status);
                            }
                            _ => return Err(KernelError::tool("invalid status")),
                        }
                    }
                    // Update content if provided
                    if let Some(content) = update["content"]
                        .as_str()
                        .filter(|content| !content.trim().is_empty())
                    {
                        todo["content"] = json!(content);
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
        r"Manage todo list for tracking progress.
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
                    "description": "read: get current list; write: full replace (needs id/content/status); update: batch partial update (only id required)"
                },
                "todos": {
                    "type": "array",
                    "maxItems": 50,
                    "description": "Required for write/update. For write: full todo list with all fields. For update: items with id + fields to change. At most 50 items per call.",
                    "items": {
                        "type": "object",
                        "required": ["id"],
                        "properties": {
                            "id": {
                                "type": "integer",
                                "description": "Identifier for this todo item"
                            },
                            "content": {
                                "type": "string",
                                "description": "The task description, must be concise and clear. No more than 100 words."
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"]
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
#[path = "todo_test.rs"]
mod tests;
