use std::sync::Arc;

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use crate::agent::AgentInput;
use crate::comms::InputBus;
use crate::const_concat;
use crate::storage::SessionStore;
use crate::tools::{Tool, ToolExecCtx};
use crate::types::{ContentBlock, KernelError, Result, SessionId, ToolOutput};

pub const POST_MESSAGE_TOOL_NAME: &str = "post_message";

pub struct PostMessageTool {
    input_bus: Arc<InputBus>,
    session_store: Option<Arc<dyn SessionStore>>,
}

impl PostMessageTool {
    pub fn new(input_bus: Arc<InputBus>, session_store: Option<Arc<dyn SessionStore>>) -> Self {
        Self {
            input_bus,
            session_store,
        }
    }
}

#[derive(Deserialize)]
struct PostMessageArgs {
    agent_id: String,
    title: String,
    content: String,
}

fn format_message(from: &str, title: &str, content: &str) -> String {
    crate::tools::format_agent_message(from, format_args!("{title}\n{content}"))
}

#[async_trait]
impl Tool for PostMessageTool {
    fn name(&self) -> &'static str {
        POST_MESSAGE_TOOL_NAME
    }

    fn desc(&self) -> &'static str {
        const_concat!(
            "Send/Replay a titled message to another agent by its ID. Use this to coordinate work, share findings, request help, or assign tasks to an agent. The recipient receives the message with your current session ID identified as the sender. Messages from other agents have the form `[From Agent: <agent_id>] <title>\\n<content>`; set `agent_id` to the sender ID from that prefix when replying. When sending a message to a background agent, ",
            crate::tools::ASYNC_LAUNCH_GUIDE
        )
    }

    fn schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "ID of the agent that should receive the message. Use the exact agent ID provided when that agent was created."
                },
                "title": {
                    "type": "string",
                    "description": "Short subject describing the purpose of the message."
                },
                "content": {
                    "type": "string",
                    "description": "Complete message for the recipient, including relevant context, findings, questions, or requested actions."
                }
            },
            "required": ["agent_id", "title", "content"],
            "additionalProperties": false
        })
    }

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let args: PostMessageArgs = serde_json::from_value(args).map_err(|error| {
            KernelError::tool(format!("Invalid post_message arguments: {error}"))
        })?;

        let target = SessionId::from(args.agent_id.clone());
        if args.agent_id.trim().is_empty() {
            return Err(KernelError::tool("agent_id must not be empty"));
        }
        if let Some(store) = &self.session_store {
            let exists = store
                .get(&target)
                .await
                .map_err(|error| KernelError::tool(format!("Failed to find agent: {error}")))?
                .is_some();
            if !exists {
                return Err(KernelError::tool(format!(
                    "Agent '{}' does not exist",
                    args.agent_id
                )));
            }
        }

        let text = format_message(&ctx.session_id, &args.title, &args.content);
        self.input_bus
            .publish(target, AgentInput::Steer(vec![ContentBlock::Text { text }]))
            .map_err(|error| KernelError::tool(format!("Failed to send message: {error}")))?;

        Ok(ToolOutput::text(format!(
            "Message sent to agent {}",
            args.agent_id
        )))
    }
}

#[cfg(test)]
#[path = "post_message_test.rs"]
mod tests;
