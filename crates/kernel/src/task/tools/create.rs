use crate::task::store::SharedTaskStore;
use crate::task::types::{CreateTaskInput, CreateTaskOutput, TaskSummary};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub const TASK_CREATE_TOOL_NAME: &str = "taskCreate";

pub struct TaskCreateTool {
    store: SharedTaskStore,
    task_list_id: String,
}

impl TaskCreateTool {
    pub const fn new(store: SharedTaskStore, task_list_id: String) -> Self {
        Self {
            store,
            task_list_id,
        }
    }
}

#[async_trait]
impl Tool for TaskCreateTool {
    fn name(&self) -> &str {
        TASK_CREATE_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Create a new task in the task list"
    }

    fn params(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "subject": {
                    "type": "string",
                    "description": "A brief title for the task in imperative form (e.g., 'Fix authentication bug')"
                },
                "description": {
                    "type": "string",
                    "description": "What needs to be done"
                },
                "activeForm": {
                    "type": "string",
                    "description": "Present continuous form shown in spinner when in_progress (e.g., 'Fixing authentication bug')"
                },
                "metadata": {
                    "type": "object",
                    "description": "Arbitrary metadata to attach to the task"
                }
            },
            "required": ["subject", "description"]
        })
    }

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let subject = args["subject"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("subject is required"))?
            .to_string();

        let description = args["description"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("description is required"))?
            .to_string();

        let active_form = args["activeForm"].as_str().map(|s| s.to_string());
        let metadata = args.get("metadata").and_then(|m| {
            serde_json::from_value::<std::collections::HashMap<String, serde_json::Value>>(
                m.clone(),
            )
            .ok()
        });

        let input = CreateTaskInput {
            subject,
            description,
            active_form,
            metadata,
        };

        let task = self.store.create_task(&self.task_list_id, input).await?;

        let output = CreateTaskOutput {
            task: TaskSummary {
                id: task.id,
                subject: task.subject,
            },
        };

        Ok(ToolOutput::text_with_summary(serde_json::to_string(&output)?, ""))
    }
}
