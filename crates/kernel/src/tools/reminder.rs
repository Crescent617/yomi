//! Reminder tool for scheduling self-reminders.
//!
//! This tool allows the main agent to schedule a reminder message
//! that will be delivered after a specified delay.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::agent::AgentInput;
use crate::comms::InputBus;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{ContentBlock, KernelError, Result, SessionId, ToolOutput};

pub const REMINDER_TOOL_NAME: &str = "reminder";

/// Tool for scheduling reminders to the agent itself.
pub struct ReminderTool {
    input_bus: Arc<InputBus>,
}

impl ReminderTool {
    /// Create a new reminder tool.
    pub fn new(input_bus: Arc<InputBus>) -> Self {
        Self { input_bus }
    }
}

#[async_trait]
impl Tool for ReminderTool {
    fn name(&self) -> &'static str {
        REMINDER_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Schedule a self-reminder after a delay. Used ONLY when the user explicitly asks to be reminded of something after a short time. Do NOT use to track tool execution, poll async tasks, or wait for other tools to complete — they already notify you automatically when finished."
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

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let delay = args["delay_seconds"]
            .as_u64()
            .ok_or_else(|| KernelError::tool("delay_seconds must be a positive integer"))?;

        if !(30..=3600).contains(&delay) {
            return Err(KernelError::tool(
                "delay_seconds must be between 30 and 3600 (inclusive)",
            ));
        }

        let message = args["message"]
            .as_str()
            .ok_or_else(|| KernelError::tool("message must be a string"))?
            .to_string();

        let input_bus = self.input_bus.clone();
        let session_id = ctx.session_id.clone();
        let _tool_call_id = ctx.tool_call_id.to_string();
        let message_for_reminder = message.clone();

        // Spawn a background task to deliver the reminder
        tokio::spawn(async move {
            sleep(Duration::from_secs(delay)).await;

            // Send reminder as a task result to wake up the agent
            let reminder = format!("Reminder (after {delay}s): {message_for_reminder}");
            if let Err(e) = input_bus.publish(
                SessionId::from(session_id.clone()),
                AgentInput::Steer(vec![ContentBlock::Text { text: reminder }]),
            ) {
                tracing::warn!("Failed to publish reminder: {}", e);
            }
        });

        Ok(ToolOutput::text(format!(
            "Reminder scheduled in {delay} seconds"
        )))
    }
}
