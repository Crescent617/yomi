//! Tool execution for Agent
//!
//! Handles executing tool calls with permission checks and hooks.

use super::message_buffer::MessageBuffer;
use crate::event::{Event, ToolEvent};
use crate::permissions::Checker;
use crate::tools::{executor::execute_tools_parallel, ToolExecutionResult, ToolRegistry};
use crate::types::{AgentId, Message, MessageId};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Tool execution handler
pub struct ToolExecutionHandler {
    /// Agent ID
    pub agent_id: AgentId,
    /// Session ID
    pub session_id: String,
    /// Working directory
    pub working_dir: PathBuf,
    /// Event sender
    pub event_tx: mpsc::Sender<Event>,
    /// Permission checker
    pub permission_checker: Option<Arc<Checker>>,
    /// Tool registry
    pub tool_registry: Arc<ToolRegistry>,
    /// Hook registry
    pub hook_registry: crate::hooks::HookRegistry,
}

impl ToolExecutionHandler {
    /// Execute tool calls from the last assistant message.
    ///
    /// File tracking for checkpoints is done via `ToolExecCtx::track_edit` during tool execution.
    pub async fn execute_tools(
        &self,
        message_buffer: &mut MessageBuffer,
        cancel_token: &super::CancelToken,
        skills: &[Arc<crate::skill::Skill>],
    ) -> Result<ExecutionOutcome, super::AgentError> {
        let tool_calls: Vec<_> = message_buffer
            .messages()
            .last()
            .and_then(|m| m.tool_calls.clone())
            .unwrap_or_default();

        if tool_calls.is_empty() {
            return Ok(ExecutionOutcome::NoTools);
        }

        // Pre-generate MessageId for each tool call
        let mut tool_message_ids: HashMap<String, MessageId> = HashMap::new();
        for call in &tool_calls {
            tool_message_ids.insert(call.id.clone(), MessageId::new());
        }

        // Send Started event for ALL tool calls
        self.send_tool_start_events(&tool_calls, &tool_message_ids)
            .await;

        // Check permissions
        let permission_result = crate::permissions::check_tool_permissions(
            &tool_calls,
            self.permission_checker.as_deref(),
            &self.agent_id,
        )
        .await;

        let mut approved_calls = permission_result.approved;
        let mut denied_results: Vec<_> = permission_result
            .denied
            .into_iter()
            .map(|(tool_call_id, error_msg)| {
                let message_id = tool_message_ids[&tool_call_id].clone();
                let message = Message::tool_result(
                    message_id.clone(),
                    tool_call_id.clone(),
                    error_msg.clone(),
                );
                ToolExecutionResult {
                    tool_call_id: tool_call_id.clone(),
                    message_id: message_id.clone(),
                    event: ToolEvent::End {
                        agent_id: self.agent_id.clone(),
                        message_id,
                        tool_id: tool_call_id.clone(),
                        tool_name: String::new(),
                        content_blocks: vec![crate::types::ToolOutputBlock::Text {
                            text: error_msg.clone(),
                        }],
                        elapsed_ms: 0,
                        is_error: true,
                    },
                    message,
                }
            })
            .collect();

        // PreToolUse hooks
        approved_calls = super::hooks::run_pre_tool_hooks(
            &self.agent_id,
            &self.session_id,
            &self.working_dir,
            &self.hook_registry,
            approved_calls,
            &mut denied_results,
        )
        .await;

        // Create runtime token for cancellation
        let runtime_token = cancel_token.runtime_token();

        // Execute approved calls
        let results = if approved_calls.is_empty() {
            Vec::new()
        } else {
            execute_tools_parallel(
                &self.agent_id,
                &approved_calls,
                &self.tool_registry,
                Some(&runtime_token),
                Some(message_buffer.messages()),
                &self.working_dir,
                &self.session_id,
                &tool_message_ids,
                None, // No turn in ToolExecutionHandler context
                skills,
            )
            .await
        };

        // PostToolUse hooks
        let (post_results, continue_session, post_contexts) = super::hooks::run_post_tool_hooks(
            &self.agent_id,
            &self.session_id,
            &self.working_dir,
            &self.hook_registry,
            results,
            &tool_calls,
        )
        .await;

        // Combine denied and executed results
        let all_results: Vec<_> = denied_results.into_iter().chain(post_results).collect();

        // Send events and add messages to buffer
        for result in &all_results {
            if cancel_token.is_cancelled() {
                return Err(super::AgentError::Cancelled("tool execution".into()));
            }
            let _ = self.event_tx.send(Event::Tool(result.event.clone())).await;
        }

        Ok(ExecutionOutcome::Completed {
            results: all_results,
            post_contexts,
            continue_session,
        })
    }

    /// Send tool start events
    async fn send_tool_start_events(
        &self,
        tool_calls: &[crate::types::ToolCall],
        tool_message_ids: &HashMap<String, MessageId>,
    ) {
        for call in tool_calls {
            let args_str = serde_json::to_string(&call.arguments).ok();
            let message_id = tool_message_ids[&call.id].clone();
            let _ = self
                .event_tx
                .send(Event::Tool(ToolEvent::Start {
                    agent_id: self.agent_id.clone(),
                    message_id,
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    arguments: args_str,
                }))
                .await;
        }
    }
}

/// Outcome of tool execution
pub enum ExecutionOutcome {
    /// No tools to execute
    NoTools,
    /// Tools executed successfully
    Completed {
        /// Tool execution results
        results: Vec<ToolExecutionResult>,
        /// Hook contexts to inject as messages
        post_contexts: Vec<String>,
        /// Whether to continue the session
        continue_session: bool,
    },
}

impl ExecutionOutcome {
    /// Check if we should continue to streaming state
    pub fn should_continue(&self) -> bool {
        match self {
            ExecutionOutcome::NoTools => false,
            ExecutionOutcome::Completed {
                continue_session, ..
            } => *continue_session,
        }
    }

    /// Get results if completed
    pub fn results(&self) -> Option<&[ToolExecutionResult]> {
        match self {
            ExecutionOutcome::NoTools => None,
            ExecutionOutcome::Completed { results, .. } => Some(results),
        }
    }

    /// Get hook contexts if completed
    pub fn post_contexts(&self) -> Option<&[String]> {
        match self {
            ExecutionOutcome::NoTools => None,
            ExecutionOutcome::Completed { post_contexts, .. } => Some(post_contexts),
        }
    }
}
