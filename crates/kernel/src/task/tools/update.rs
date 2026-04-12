use crate::task::storage::TaskUpdates;
use crate::task::store::SharedTaskStore;
use crate::task::types::{TaskStatus, UpdateTaskOutput, StatusChange};
use crate::tools::Tool;
use crate::types::ToolOutput;
use anyhow::Result;
use async_trait::async_trait;
use serde_json::{json, Value};

pub const TASK_UPDATE_TOOL_NAME: &str = "TaskUpdate";

pub struct TaskUpdateTool {
    store: SharedTaskStore,
    get_session_id: Box<dyn Fn() -> String + Send + Sync>,
}

impl TaskUpdateTool {
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
impl Tool for TaskUpdateTool {
    fn name(&self) -> &str {
        TASK_UPDATE_TOOL_NAME
    }

    fn desc(&self) -> &str {
        "Update a task in the task list"
    }

    fn params(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "taskId": {
                    "type": "string",
                    "description": "The ID of the task to update"
                },
                "subject": {
                    "type": "string",
                    "description": "New subject for the task"
                },
                "description": {
                    "type": "string",
                    "description": "New description for the task"
                },
                "activeForm": {
                    "type": "string",
                    "description": "New activeForm for the task"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "deleted"],
                    "description": "New status for the task"
                },
                "owner": {
                    "type": "string",
                    "description": "New owner for the task"
                },
                "addBlocks": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Task IDs that this task blocks"
                },
                "addBlockedBy": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Task IDs that block this task"
                },
                "metadata": {
                    "type": "object",
                    "description": "Metadata keys to merge into the task"
                }
            },
            "required": ["taskId"]
        })
    }

    async fn exec(&self, args: Value) -> Result<ToolOutput> {
        let task_id = args["taskId"].as_str()
            .ok_or_else(|| anyhow::anyhow!("taskId is required"))?
            .to_string();

        let task_list_id = self.get_task_list_id();

        let existing = self.store.get_task(&task_list_id, &task_id).await?;
        if existing.is_none() {
            let output = UpdateTaskOutput {
                success: false,
                task_id,
                updated_fields: Vec::new(),
                error: Some("Task not found".to_string()),
                status_change: None,
            };
            return Ok(ToolOutput::new(
                serde_json::to_string(&output)?,
                "",
            ));
        }
        let existing = existing.unwrap();

        if let Some("deleted") = args["status"].as_str() {
            let deleted = self.store.delete_task(&task_list_id, &task_id).await?;
            let output = UpdateTaskOutput {
                success: deleted,
                task_id,
                updated_fields: if deleted { vec!["deleted".to_string()] } else { Vec::new() },
                error: if deleted { None } else { Some("Failed to delete task".to_string()) },
                status_change: if deleted {
                    Some(StatusChange { from: existing.status.to_string(), to: "deleted".to_string() })
                } else {
                    None
                },
            };
            return Ok(ToolOutput::new(
                serde_json::to_string(&output)?,
                "",
            ));
        }

        let mut updates = TaskUpdates::default();

        if let Some(subject) = args["subject"].as_str() {
            updates.subject = Some(subject.to_string());
        }
        if let Some(description) = args["description"].as_str() {
            updates.description = Some(description.to_string());
        }
        if let Some(active_form) = args["activeForm"].as_str() {
            updates.active_form = Some(active_form.to_string());
        }
        if let Some(owner) = args["owner"].as_str() {
            updates.owner = Some(owner.to_string());
        }
        if let Some(status_str) = args["status"].as_str() {
            let status = match status_str {
                "pending" => TaskStatus::Pending,
                "in_progress" => TaskStatus::InProgress,
                "completed" => TaskStatus::Completed,
                _ => return Err(anyhow::anyhow!("Invalid status: {}", status_str)),
            };
            updates.status = Some(status);
        }
        if let Some(metadata) = args.get("metadata").and_then(|m| {
            serde_json::from_value::<std::collections::HashMap<String, serde_json::Value>>(m.clone()).ok()
        }) {
            updates.metadata = Some(metadata);
        }

        let result = self.store.update_task(&task_list_id, &task_id, updates).await?;

        if let Some((task, mut fields)) = result {
            if let Some(add_blocks) = args["addBlocks"].as_array() {
                let block_ids: Vec<String> = add_blocks.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();

                for block_id in &block_ids {
                    if !existing.blocks.contains(block_id) {
                        self.store.block_task(&task_list_id, &task_id, block_id).await?;
                    }
                }
                if !block_ids.is_empty() && !fields.contains(&"blocks".to_string()) {
                    fields.push("blocks".to_string());
                }
            }

            if let Some(add_blocked_by) = args["addBlockedBy"].as_array() {
                let blocker_ids: Vec<String> = add_blocked_by.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();

                for blocker_id in &blocker_ids {
                    if !existing.blocked_by.contains(blocker_id) {
                        self.store.block_task(&task_list_id, blocker_id, &task_id).await?;
                    }
                }
                if !blocker_ids.is_empty() && !fields.contains(&"blockedBy".to_string()) {
                    fields.push("blockedBy".to_string());
                }
            }

            let output = UpdateTaskOutput {
                success: true,
                task_id,
                updated_fields: fields.clone(),
                error: None,
                status_change: if fields.contains(&"status".to_string()) {
                    Some(StatusChange {
                        from: existing.status.to_string(),
                        to: task.status.to_string(),
                    })
                } else {
                    None
                },
            };

            Ok(ToolOutput::new(
                serde_json::to_string(&output)?,
                "",
            ))
        } else {
            let output = UpdateTaskOutput {
                success: false,
                task_id,
                updated_fields: Vec::new(),
                error: Some("Task not found".to_string()),
                status_change: None,
            };
            Ok(ToolOutput::new(
                serde_json::to_string(&output)?,
                "",
            ))
        }
    }
}
