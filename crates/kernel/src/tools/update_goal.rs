use crate::goal::GoalStatus;
use crate::goal::GoalStore;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{KernelError, Result, ToolOutput};
use async_trait::async_trait;
use serde_json::json;
use serde_json::Value;
use std::sync::Arc;

pub const UPDATE_GOAL_TOOL_NAME: &str = "update_goal";

/// `UpdateGoalTool` - Allows the model to update the status of the active goal.
/// Only `completed` and `blocked` are allowed; other status changes are controlled
/// by the user or system.
pub struct UpdateGoalTool {
    store: Arc<dyn GoalStore>,
}

impl UpdateGoalTool {
    pub fn new(store: Arc<dyn GoalStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for UpdateGoalTool {
    fn name(&self) -> &str {
        UPDATE_GOAL_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        r#"Update the existing goal status.

Use this tool only to mark the goal achieved or genuinely blocked.
- Set status to "completed" only when the objective has actually been achieved and no required work remains. Before marking complete, verify the actual current state against every requirement in the objective. Do not rely on intent, partial progress, or memory of earlier work as proof of completion.
- Set status to "blocked" only when the same blocking condition has repeated for at least three consecutive goal turns, counting the original/user-triggered turn and any automatic goal continuations, and the agent cannot make meaningful progress without user input or an external-state change.

Do not mark a goal complete merely because you are stopping work or because max iterations is approaching.
Do not use "blocked" merely because the work is hard, slow, uncertain, incomplete, or would benefit from clarification."#
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["completed", "blocked"],
                    "description": "Required. Set to 'completed' only when the objective is achieved and no required work remains. Set to 'blocked' only after the same blocking condition has recurred for at least three consecutive goal turns and the agent is at an impasse."
                }
            },
            "required": ["status"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let status = args
            .get("status")
            .ok_or_else(|| KernelError::tool("status is required"))?
            .as_str()
            .ok_or_else(|| KernelError::tool("status must be a string"))?;

        let mut state = self
            .store
            .load(&ctx.session_id)
            .await
            .map_err(|e| KernelError::tool(format!("failed to load goal: {e}")))?
            .ok_or_else(|| KernelError::tool("no active goal found for this session"))?;

        match status {
            "completed" => {
                state.status = GoalStatus::Completed;
            }
            "blocked" => {
                state.status = GoalStatus::Blocked;
            }
            _ => {
                return Err(KernelError::tool(
                    "invalid status: must be 'completed' or 'blocked'",
                ))
            }
        }

        self.store
            .save(&ctx.session_id, &state)
            .await
            .map_err(|e| KernelError::tool(format!("failed to save goal: {e}")))?;

        Ok(ToolOutput::text(format!(
            "Goal status updated to \"{}\". The objective was:\n{}",
            status, state.description
        )))
    }
}

#[cfg(test)]
#[path = "update_goal_test.rs"]
mod tests;
