use crate::task::store::SharedTaskStore;
use crate::task::types::GetTaskOutput;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub const TASK_GET_TOOL_NAME: &str = "taskGet";

pub struct TaskGetTool {
    store: SharedTaskStore,
    task_list_id: String,
}

impl TaskGetTool {
    pub const fn new(store: SharedTaskStore, task_list_id: String) -> Self {
        Self {
            store,
            task_list_id,
        }
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

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let task_id = args["taskId"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("taskId is required"))?
            .to_string();

        let task = self.store.get_task(&self.task_list_id, &task_id).await?;

        let output = GetTaskOutput { task };

        Ok(ToolOutput::text_with_summary(
            serde_json::to_string(&output)?,
            "",
        ))
    }
}
