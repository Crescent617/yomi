//! Reminder tool for scheduling self-reminders.
//!
//! This tool allows the main agent to schedule a reminder message
//! that will be delivered after a specified delay.

use async_trait::async_trait;
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::time::sleep;

use crate::agent::AgentInput;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{ContentBlock, KernelError, Result, ToolOutput};

pub const REMINDER_TOOL_NAME: &str = "reminder";

/// Tool for scheduling reminders to the agent itself.
pub struct ReminderTool {
    input_tx: mpsc::Sender<AgentInput>,
}

impl ReminderTool {
    /// Create a new reminder tool.
    pub fn new(input_tx: mpsc::Sender<AgentInput>) -> Self {
        Self { input_tx }
    }
}

#[async_trait]
impl Tool for ReminderTool {
    fn name(&self) -> &'static str {
        "reminder"
    }

    fn desc(&self) -> &'static str {
        "Schedule a self-reminder after a delay. Use to check on tasks, follow-ups, or periodic check-ins. Delays should be 30s-3600s. Reminder arrives as a new message when the delay expires."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "delay_seconds": {
                    "type": "integer",
                    "description": "Number of seconds to wait before delivering the reminder. Keep reasonable (30s - 3600s).",
                    "minimum": 30,
                    "maximum": 3600
                },
                "message": {
                    "type": "string",
                    "description": "The reminder message to deliver. Be specific about what to check or do."
                }
            },
            "required": ["delay_seconds", "message"]
        })
    }

    async fn exec(&self, args: Value, _ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let delay = args["delay_seconds"]
            .as_u64()
            .ok_or_else(|| KernelError::tool("delay_seconds must be a positive integer"))?;

        let message = args["message"]
            .as_str()
            .ok_or_else(|| KernelError::tool("message must be a string"))?
            .to_string();

        let input_tx = self.input_tx.clone();
        let tool_call_id = _ctx.tool_call_id.to_string();
        let message_for_reminder = message.clone();

        // Spawn a background task to deliver the reminder
        tokio::spawn(async move {
            sleep(Duration::from_secs(delay)).await;

            // Send reminder as a task result to wake up the agent
            let reminder = format!("Reminder (after {delay}s): {message_for_reminder}");
            let _ = input_tx
                .send(AgentInput::TaskResult {
                    task_id: tool_call_id,
                    content: vec![ContentBlock::Text { text: reminder }],
                })
                .await;
        });

        Ok(ToolOutput::text(format!(
            "Reminder scheduled  in {delay} seconds"
        )))
    }
}
