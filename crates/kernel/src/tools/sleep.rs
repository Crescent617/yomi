//! Sleep tool for synchronous waiting.
//!
//! This tool allows the agent to pause execution for a specified duration.
//! Unlike the reminder tool, this blocks synchronously and returns when the
//! delay has elapsed.
//!
//! When an input bus is available, the sleep wakes up early if new input
//! (e.g. a user message or a steer message) arrives for the current session,
//! so the agent can react promptly instead of sleeping through it.

use async_trait::async_trait;
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;

use crate::agent::AgentInput;
use crate::comms::InputBus;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{Result, SessionId, ToolOutput};

pub const SLEEP_TOOL_NAME: &str = "sleep";

/// Tool for synchronous sleeping / waiting.
pub struct SleepTool {
    /// Input bus used to wake the sleep early when new input arrives for the
    /// current session. `None` in contexts without a bus (e.g. subagents).
    input_bus: Option<Arc<InputBus>>,
}

impl SleepTool {
    /// Create a new sleep tool.
    pub fn new(input_bus: Option<Arc<InputBus>>) -> Self {
        Self { input_bus }
    }
}

/// Returns `true` for inputs that should wake a sleeping agent early.
///
/// Excluded:
/// - `Cancel`: handled deterministically via the cancel token.
/// - `PermissionResponse` / `AskUserResponse`: directed at specific
///   subscribers (Checker / AskUserTool), not the agent loop.
fn should_wake(input: &AgentInput) -> bool {
    !matches!(
        input,
        AgentInput::Cancel
            | AgentInput::PermissionResponse { .. }
            | AgentInput::AskUserResponse { .. }
    )
}

/// Human-readable kind of an input, used in the early-wake message.
fn describe_input(input: &AgentInput) -> &'static str {
    match input {
        AgentInput::User { .. } => "user message",
        AgentInput::Continue => "continue signal",
        AgentInput::Cancel => "cancel request",
        AgentInput::Steer(_) => "steer message",
        AgentInput::PermissionResponse { .. } => "permission response",
        AgentInput::Shutdown => "shutdown signal",
        AgentInput::Compact => "compact request",
        AgentInput::Rewind { .. } => "rewind request",
        AgentInput::Clear => "clear request",
        AgentInput::AskUserResponse { .. } => "ask_user response",
    }
}

#[async_trait]
impl Tool for SleepTool {
    fn name(&self) -> &'static str {
        SLEEP_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        "Sleep for a specified number of seconds. Use when an external process needs time to settle or requires waiting. Wakes up early if new input arrives for the current session."
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                    "seconds": {
                        "type": "integer",
                        "description": "Number of seconds to sleep. Keep reasonable (1 - 3600).",
                        "minimum": 1,
                        "maximum": 3600
                    }
            },
            "required": ["seconds"]
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let delay = args["seconds"]
            .as_u64()
            .ok_or_else(|| crate::types::KernelError::tool("seconds must be a positive integer"))?;

        if delay > 3600 {
            return Err(crate::types::KernelError::tool(
                "seconds must be between 1 and 3600 (inclusive)",
            ));
        }

        // Subscribe BEFORE starting the timer to minimize the chance of
        // missing an input that arrives right as the sleep starts.
        // The subscription is a fan-out peek: messages are still queued to
        // the session mailbox by the conductor and processed after we return.
        let mut subscriber = self
            .input_bus
            .as_ref()
            .map(|bus| bus.subscribe_filtered(SessionId::from(ctx.session_id.clone()), should_wake));

        let start = tokio::time::Instant::now();
        tokio::select! {
            () = sleep(Duration::from_secs(delay)) => {
                Ok(ToolOutput::text(format!("Slept for {delay} seconds")))
            }
            () = ctx.cancelled() => {
                let elapsed = start.elapsed().as_secs();
                Ok(ToolOutput::text(format!(
                    "Sleep cancelled after {elapsed} seconds (planned {delay} seconds, not completed)"
                )))
            }
            input = recv_wake(&mut subscriber) => {
                let elapsed = start.elapsed().as_secs();
                Ok(ToolOutput::text(format!(
                    "Sleep interrupted after {elapsed} seconds (planned {delay} seconds): new {} arrived for this session. It will be processed next.",
                    describe_input(&input)
                )))
            }
        }
    }
}

/// Wait for a wake-worthy input. Resolves to `AgentInput::Continue` as a
/// placeholder when there is no subscriber (the branch then never completes
/// because the inner future pends forever).
async fn recv_wake(
    subscriber: &mut Option<crate::comms::InputBusSubscriber>,
) -> AgentInput {
    match subscriber {
        Some(sub) => match sub.recv().await {
            Some((_, input)) => input,
            // Bus closed; never resolve so the timer/cancel branches decide.
            None => std::future::pending().await,
        },
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "sleep_test.rs"]
mod tests;
