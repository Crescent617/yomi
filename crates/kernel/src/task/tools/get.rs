use crate::task::store::SharedTaskStore;
use crate::task::types::GetTaskOutput;
use crate::tools::Tool;
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub const TASK_GET_TOOL_NAME: &str = "TaskGet";

pub struct TaskGetTool {
    store: SharedTaskStore,
    get_session_id: Box<dyn Fn() -> String + Send + Sync>,
}

impl TaskGetTool {
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
impl Tool for TaskGetTool {
    fn name(&self) -> &str {
        TASK_GET_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Retrieve a task by ID"
    }

    fn params(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "taskId": {
                    "type": "string",
                    "description": "The ID of the task to retrieve"
                }
            },
            "required": ["taskId"]
        })
    }

    async fn exec(&self, args: Value) -> Result<ToolOutput> {
        let task_id = args["taskId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("taskId is required"))?
            .to_string();

        let task_list_id = self.get_task_list_id();
        let task = self.store.get_task(&task_list_id, &task_id).await?;

        let output = GetTaskOutput { task };

        Ok(ToolOutput::new(serde_json::to_string(&output)?, ""))
    }
}
