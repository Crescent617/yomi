use crate::task::store::SharedTaskStore;
use crate::task::types::TaskUpdates;
use crate::task::types::{StatusChange, TaskStatus, UpdateTaskOutput};
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use async_trait::async_trait;
use serde_json::{json, Value};

pub const TASK_UPDATE_TOOL_NAME: &str = "taskUpdate";

pub struct TaskUpdateTool {
    store: SharedTaskStore,
    task_list_id: String,
}

impl TaskUpdateTool {
    pub const fn new(store: SharedTaskStore, task_list_id: String) -> Self {
        Self {
            store,
            task_list_id,
        }
    }
}

#[async_trait]
impl Tool for TaskUpdateTool {
    fn name(&self) -> &str {
        TASK_UPDATE_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Update a task in the task list"
    }

    fn schema(&self) -> Value {
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

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let task_id = args["taskId"]
            .as_str()
            .ok_or_else(|| KernelError::tool("taskId is required"))?
            .to_string();

        let existing = self.store.get_task(&self.task_list_id, &task_id).await?;
        if existing.is_none() {
            let output = UpdateTaskOutput {
                success: false,
                task_id,
                updated_fields: Vec::new(),
                error: Some("Task not found".to_string()),
                status_change: None,
            };
            return Ok(ToolOutput::text_with_summary(
                serde_json::to_string(&output)?,
                "",
            ));
        }
        let existing = existing.unwrap();

        if args["status"].as_str() == Some("deleted") {
            let deleted = self.store.delete_task(&self.task_list_id, &task_id).await?;
            let output = UpdateTaskOutput {
                success: deleted,
                task_id,
                updated_fields: if deleted {
                    vec!["deleted".to_string()]
                } else {
                    Vec::new()
                },
                error: if deleted {
                    None
                } else {
                    Some("Failed to delete task".to_string())
                },
                status_change: if deleted {
                    Some(StatusChange {
                        from: existing.status.to_string(),
                        to: "deleted".to_string(),
                    })
                } else {
                    None
                },
            };
            return Ok(ToolOutput::text_with_summary(
                serde_json::to_string(&output)?,
                "",
            ));
        }

        // Build updates - including blocks/blocked_by changes
        let mut updates = TaskUpdates::default();
        let mut updated_fields = Vec::new();

        if let Some(subject) = args["subject"].as_str() {
            if subject != existing.subject {
                updates.subject = Some(subject.to_string());
                updated_fields.push("subject");
            }
        }
        if let Some(description) = args["description"].as_str() {
            if description != existing.description {
                updates.description = Some(description.to_string());
                updated_fields.push("description");
            }
        }
        if let Some(owner) = args["owner"].as_str() {
            if Some(owner.to_string()) != existing.owner {
                updates.owner = Some(owner.to_string());
                updated_fields.push("owner");
            }
        }
        if let Some(status_str) = args["status"].as_str() {
            let status = match status_str {
                "pending" => TaskStatus::Pending,
                "in_progress" => TaskStatus::InProgress,
                "completed" => TaskStatus::Completed,
                _ => return Err(KernelError::tool(format!("Invalid status: {status_str}"))),
            };
            if status != existing.status {
                updates.status = Some(status);
                updated_fields.push("status");
            }
        }
        if let Some(metadata) = args.get("metadata").and_then(|m| {
            serde_json::from_value::<std::collections::HashMap<String, serde_json::Value>>(
                m.clone(),
            )
            .ok()
        }) {
            updates.metadata = Some(metadata);
            updated_fields.push("metadata");
        }

        // Handle addBlocks - merge into existing blocks
        if let Some(add_blocks) = args["addBlocks"].as_array() {
            let mut new_blocks = existing.blocks.clone();
            let block_ids: Vec<String> = add_blocks
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();

            for block_id in block_ids {
                if !new_blocks.contains(&block_id) {
                    new_blocks.push(block_id);
                }
            }

            if new_blocks.len() != existing.blocks.len() {
                updates.blocks = Some(new_blocks);
                updated_fields.push("blocks");
            }
        }

        // Handle addBlockedBy - merge into existing blocked_by
        if let Some(add_blocked_by) = args["addBlockedBy"].as_array() {
            let mut new_blocked_by = existing.blocked_by.clone();
            let blocker_ids: Vec<String> = add_blocked_by
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();

            for blocker_id in blocker_ids {
                if !new_blocked_by.contains(&blocker_id) {
                    new_blocked_by.push(blocker_id);
                }
            }

            if new_blocked_by.len() != existing.blocked_by.len() {
                updates.blocked_by = Some(new_blocked_by);
                updated_fields.push("blocked_by");
            }
        }

        // Perform single atomic update
        let result = self
            .store
            .update_task(&self.task_list_id, &task_id, updates)
            .await?;

        if let Some((task, _)) = result {
            let output = UpdateTaskOutput {
                success: true,
                task_id,
                updated_fields: updated_fields.into_iter().map(|s| s.to_string()).collect(),
                error: None,
                status_change: if args["status"].as_str().is_some() {
                    Some(StatusChange {
                        from: existing.status.to_string(),
                        to: task.status.to_string(),
                    })
                } else {
                    None
                },
            };

            Ok(ToolOutput::text_with_summary(
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
            Ok(ToolOutput::text_with_summary(
                serde_json::to_string(&output)?,
                "",
            ))
        }
    }
}
