use crate::task::storage::TaskUpdates;
use crate::task::store::SharedTaskStore;
use crate::task::types::{StatusChange, TaskStatus, UpdateTaskOutput};
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

    /// Add a dependency relationship: this_task blocks target_task
    async fn add_reverse_blocked_by(
        &self,
        task_list_id: &str,
        this_task_id: &str,
        target_task_id: &str,
    ) -> Result<()> {
        let target = self.store.get_task(task_list_id, target_task_id).await?;
        if let Some(target) = target {
            if !target.blocked_by.contains(&this_task_id.to_string()) {
                let mut new_blocked_by = target.blocked_by.clone();
                new_blocked_by.push(this_task_id.to_string());
                self.store
                    .update_task(
                        task_list_id,
                        target_task_id,
                        TaskUpdates {
                            blocked_by: Some(new_blocked_by),
                            ..Default::default()
                        },
                    )
                    .await?;
            }
        }
        Ok(())
    }

    /// Add a dependency relationship: blocker_task blocks this_task
    async fn add_reverse_blocks(
        &self,
        task_list_id: &str,
        this_task_id: &str,
        blocker_task_id: &str,
    ) -> Result<()> {
        let blocker = self.store.get_task(task_list_id, blocker_task_id).await?;
        if let Some(blocker) = blocker {
            if !blocker.blocks.contains(&this_task_id.to_string()) {
                let mut new_blocks = blocker.blocks.clone();
                new_blocks.push(this_task_id.to_string());
                self.store
                    .update_task(
                        task_list_id,
                        blocker_task_id,
                        TaskUpdates {
                            blocks: Some(new_blocks),
                            ..Default::default()
                        },
                    )
                    .await?;
            }
        }
        Ok(())
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
        let task_id = args["taskId"]
            .as_str()
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
            return Ok(ToolOutput::new(serde_json::to_string(&output)?, ""));
        }
        let existing = existing.unwrap();

        if args["status"].as_str() == Some("deleted") {
            let deleted = self.store.delete_task(&task_list_id, &task_id).await?;
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
            return Ok(ToolOutput::new(serde_json::to_string(&output)?, ""));
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
        if let Some(active_form) = args["activeForm"].as_str() {
            if Some(active_form.to_string()) != existing.active_form {
                updates.active_form = Some(active_form.to_string());
                updated_fields.push("active_form");
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
                _ => return Err(anyhow::anyhow!("Invalid status: {status_str}")),
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
            .update_task(&task_list_id, &task_id, updates)
            .await?;

        if let Some((task, _)) = result {
            // Update reverse relationships (the other side of the dependency)
            if let Some(add_blocks) = args["addBlocks"].as_array() {
                for block_id in add_blocks.iter().filter_map(|v| v.as_str()) {
                    self.add_reverse_blocked_by(&task_list_id, &task_id, block_id)
                        .await?;
                }
            }

            if let Some(add_blocked_by) = args["addBlockedBy"].as_array() {
                for blocker_id in add_blocked_by.iter().filter_map(|v| v.as_str()) {
                    self.add_reverse_blocks(&task_list_id, &task_id, blocker_id)
                        .await?;
                }
            }

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

            Ok(ToolOutput::new(serde_json::to_string(&output)?, ""))
        } else {
            let output = UpdateTaskOutput {
                success: false,
                task_id,
                updated_fields: Vec::new(),
                error: Some("Task not found".to_string()),
                status_change: None,
            };
            Ok(ToolOutput::new(serde_json::to_string(&output)?, ""))
        }
    }
}
