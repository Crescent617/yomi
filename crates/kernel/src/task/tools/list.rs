use crate::task::store::SharedTaskStore;
use crate::task::types::{ListTasksOutput, TaskListItem};
use crate::tools::Tool;
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub const TASK_LIST_TOOL_NAME: &str = "TaskList";

pub struct TaskListTool {
    store: SharedTaskStore,
    get_session_id: Box<dyn Fn() -> String + Send + Sync>,
}

impl TaskListTool {
    pub fn new<F>(store: SharedTaskStore, get_session_id: F) -> Self
    where
        F: Fn() -> String + Send + Sync + 'static,
    {
        Self {
            store,
            get_session_id: Box::new(get_session_id),
        }
    }

    fn get_task_list_id(&self) -> String {
        (self.get_session_id)()
    }
}

#[async_trait]
impl Tool for TaskListTool {
    fn name(&self) -> &str {
        TASK_LIST_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "List all tasks in the task list"
    }

    fn params(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "includeCompleted": {
                    "type": "boolean",
                    "description": "Whether to include completed tasks (default: false)"
                }
            },
            "required": []
        })
    }

    async fn exec(&self, args: Value) -> Result<ToolOutput> {
        let task_list_id = self.get_task_list_id();
        let all_tasks = self.store.list_tasks(&task_list_id).await?;

        let include_completed = args["includeCompleted"].as_bool().unwrap_or(false);

        // Always collect completed IDs for filtering blocked_by
        let resolved_ids: std::collections::HashSet<_> = all_tasks
            .iter()
            .filter(|t| matches!(t.status, crate::task::types::TaskStatus::Completed))
            .map(|t| t.id.clone())
            .collect();

        // Filter to only pending/in_progress unless includeCompleted is true
        let filtered_tasks: Vec<_> = if include_completed {
            all_tasks
        } else {
            all_tasks
                .into_iter()
                .filter(|t| !matches!(t.status, crate::task::types::TaskStatus::Completed))
                .collect()
        };

        let tasks: Vec<TaskListItem> = filtered_tasks
            .into_iter()
            .filter(|t| {
                !t.metadata
                    .as_ref()
                    .is_some_and(|m| m.contains_key("_internal"))
            })
            .map(|task| TaskListItem {
                id: task.id,
                subject: task.subject,
                status: task.status,
                owner: task.owner,
                blocked_by: task
                    .blocked_by
                    .into_iter()
                    .filter(|id| !resolved_ids.contains(id))
                    .collect(),
            })
            .collect();

        let output = ListTasksOutput { tasks };

        Ok(ToolOutput::new(serde_json::to_string(&output)?, ""))
    }
}
