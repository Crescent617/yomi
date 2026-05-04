//! `SimpleAgent` - Minimal agent implementation for subagents
//!
//! Unlike the full Agent, `SimpleAgent`:
//! - Has no complex state machine
//! - No persistence (no storage dependency)
//! - Uses tokio native `CancellationToken`
//! - Single request-response loop with tool execution
//! - Supports streaming events via callback
//! - Works with Arc<Message> for efficient message sharing
//!
//! This is designed to be used by `SubagentTool` without depending on
//! the full Agent infrastructure.

use crate::event::{Event, ModelEvent, ToolEvent};
use crate::permissions::Checker;
use crate::providers::{ModelConfig, Provider};
use crate::tools::{ToolExecCtx, ToolRegistry};
use crate::types::{AgentId, KernelError, Message, Result, ToolCall};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Create a cancellation error
pub fn cancelled_error(msg: &str) -> KernelError {
    KernelError::cancelled(msg)
}

/// Check if an error is a cancellation error
pub fn is_cancelled_error(err: &KernelError) -> bool {
    err.is_cancelled()
}

/// Metrics collected during execution
#[derive(Debug, Default)]
pub struct ExecuteMetrics {
    /// Total iterations (model requests)
    pub iteration_count: usize,
    /// Token usage (prompt, completion, cached)
    pub token_usage: crate::providers::TokenUsage,
    /// Assistant output text (for progress reporting)
    pub output_text: String,
    /// Whether the task completed (false if `max_iterations` reached)
    pub completed: bool,
}

/// Minimal agent for executing a single task
pub struct SimpleAgent {
    provider: Arc<dyn Provider>,
    model_config: ModelConfig,
    tool_registry: ToolRegistry,
    max_iterations: usize,
    /// Optional permission checker for tool execution
    permission_checker: Option<Arc<Checker>>,
    /// Event sender for tool events (permission requests, started, etc.)
    event_tx: Option<mpsc::Sender<Event>>,
    /// Agent ID for events
    agent_id: AgentId,
    /// Working directory for tool execution
    working_dir: std::path::PathBuf,
}

impl SimpleAgent {
    pub fn new(
        provider: Arc<dyn Provider>,
        model_config: ModelConfig,
        tool_registry: ToolRegistry,
        working_dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            provider,
            model_config,
            tool_registry,
            max_iterations: 100,
            permission_checker: None,
            event_tx: None,
            agent_id: AgentId::new(),
            working_dir: working_dir.into(),
        }
    }

    /// Set permission checker for tool execution
    #[must_use]
    pub fn with_permission_checker(mut self, checker: Arc<Checker>) -> Self {
        self.permission_checker = Some(checker);
        self
    }

    /// Set optional permission checker for tool execution
    #[must_use]
    pub fn with_permission_checker_opt(mut self, checker: Option<Arc<Checker>>) -> Self {
        self.permission_checker = checker;
        self
    }

    /// Set event sender for tool events
    #[must_use]
    pub fn with_event_tx(mut self, event_tx: mpsc::Sender<Event>) -> Self {
        self.event_tx = Some(event_tx);
        self
    }

    /// Set agent ID for events
    #[must_use]
    pub fn with_agent_id(mut self, agent_id: AgentId) -> Self {
        self.agent_id = agent_id;
        self
    }

    /// Execute a single task with the given prompt and optional history
    /// Returns the final messages (including assistant responses and tool results)
    /// and execution metrics.
    ///
    /// Events are sent via the `on_event` callback for streaming output and progress tracking.
    ///
    /// # Arguments
    /// * `system_prompt` - System message defining the agent's role
    /// * `history` - Optional conversation history (for context inheritance)
    /// * `user_prompt` - The task description
    /// * `cancel_token` - Cancellation token for stopping execution
    /// * `on_event` - Callback for receiving events (chunks, token usage, etc.)
    pub async fn execute<F>(
        &mut self,
        system_prompt: String,
        history: Option<Vec<Arc<Message>>>,
        user_prompt: String,
        cancel_token: CancellationToken,
        mut on_event: F,
    ) -> Result<(Vec<Arc<Message>>, ExecuteMetrics)>
    where
        F: FnMut(Event),
    {
        let mut messages: Vec<Arc<Message>> = Vec::new();
        messages.push(Arc::new(Message::system(system_prompt)));

        // Add history if provided (for context inheritance)
        if let Some(history) = history {
            // Filter out system messages from history to avoid duplication
            for msg in history {
                if msg.role != crate::types::Role::System {
                    messages.push(msg);
                }
            }
        }

        messages.push(Arc::new(Message::user(user_prompt)));

        // Track metrics for progress reporting
        let mut metrics = ExecuteMetrics::default();

        // Execute iterations
        let mut task_completed = false;
        for iteration in 0..self.max_iterations {
            if cancel_token.is_cancelled() {
                return Err(cancelled_error("execution cancelled"));
            }

            // iteration is 0-based, so add 1 to get actual count
            metrics.iteration_count = iteration + 1;
            on_event(Event::Model(ModelEvent::Request {
                agent_id: self.agent_id.clone(),
                message_count: messages.len(),
            }));

            // Get model response
            let (assistant_msg, token_usage) = self
                .stream_model(&messages, &cancel_token, &mut on_event)
                .await?;

            // Update token usage if available
            // Note: For each model response:
            // - prompt_tokens includes the FULL conversation history (all previous messages)
            // - completion_tokens is the CURRENT response's output tokens only
            // So total_tokens = prompt_tokens + completion_tokens represents the cumulative total
            // across all iterations. We directly assign (not accumulate) because each response
            // already includes the complete history in prompt_tokens.
            if let Some(usage) = token_usage {
                metrics.token_usage = usage;
                on_event(Event::Model(ModelEvent::TokenUsage {
                    agent_id: self.agent_id.clone(),
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: usage.total_tokens(),
                    context_window: 0, // SimpleAgent doesn't track context window
                }));
            }

            // Check if cancelled during streaming
            if cancel_token.is_cancelled() {
                return Err(cancelled_error("execution cancelled"));
            }

            // Collect output text from assistant message
            for block in &assistant_msg.content {
                if let crate::types::ContentBlock::Text { text } = block {
                    metrics.output_text.push_str(text);
                }
            }

            let has_tool_calls = assistant_msg.tool_calls.is_some();
            let assistant_arc = Arc::new(assistant_msg);
            messages.push(assistant_arc.clone());

            if !has_tool_calls {
                // Done - no tool calls
                task_completed = true;
                break;
            }

            // Get tool calls
            let tool_calls = assistant_arc.tool_calls.clone().unwrap_or_default();

            // Send Started event for all tool calls first
            for call in &tool_calls {
                let args_str = serde_json::to_string(&call.arguments).ok();
                on_event(Event::Tool(ToolEvent::Started {
                    agent_id: self.agent_id.clone(),
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    arguments: args_str,
                }));
            }

            // Check permissions
            let permission_result = crate::permissions::check_tool_permissions(
                &tool_calls,
                self.permission_checker.as_deref(),
                &self.agent_id,
            )
            .await;

            // Execute approved calls
            for call in permission_result.approved {
                if cancel_token.is_cancelled() {
                    return Err(cancelled_error("execution cancelled"));
                }

                let result = self.execute_tool(&call, &cancel_token).await?;
                messages.push(Arc::new(result));
            }

            // Add denied results as tool result messages
            for (tool_call_id, error_msg) in permission_result.denied {
                messages.push(Arc::new(Message::tool_result(tool_call_id, error_msg)));
            }
        }

        // Set completion status (false if max_iterations reached without finishing)
        metrics.completed = task_completed;

        // Add clear marker to output if task did not complete
        if !task_completed {
            use std::fmt::Write;
            write!(
                metrics.output_text,
                "\n\n[Task incomplete: reached {} iteration limit. \
                 Consider breaking into smaller sub-tasks]",
                metrics.iteration_count
            )
            .ok();
        }

        Ok((messages, metrics))
    }

    /// Stream from model, collecting the response
    /// Returns the assistant message and optional token usage
    async fn stream_model<F>(
        &mut self,
        messages: &[Arc<Message>],
        cancel_token: &CancellationToken,
        on_event: &mut F,
    ) -> Result<(Message, Option<crate::providers::TokenUsage>)>
    where
        F: FnMut(Event),
    {
        use super::stream_collector::StreamCollectorState;
        use crate::providers::ModelStreamItem;
        use futures::TryStreamExt;

        let tools = self.tool_registry.definitions();

        let mut stream = self
            .provider
            .stream(messages, &tools, &self.model_config)
            .await
            .map_err(crate::agent::AgentError::from)?;

        let mut state = StreamCollectorState::default();

        loop {
            tokio::select! {
                biased;
                () = cancel_token.cancelled() => {
                    return Err(cancelled_error("during streaming"));
                }
                item = stream.try_next() => match item {
                    Ok(Some(item)) => match item {
                        ModelStreamItem::Chunk(chunk) => {
                            state.handle_chunk(&chunk);
                            on_event(Event::Model(ModelEvent::Chunk {
                                agent_id: self.agent_id.clone(),
                                content: chunk,
                            }));
                        }
                        ModelStreamItem::ToolCallDelta { id, name, arguments_delta } => {
                            // Forward incremental tool call update for UI feedback
                            on_event(Event::Model(ModelEvent::ToolCallDelta {
                                agent_id: self.agent_id.clone(),
                                tool_id: id,
                                tool_name: name,
                                arguments_delta,
                            }));
                        }
                        ModelStreamItem::ToolCall(request) => {
                            state.handle_tool_call(request);
                        }
                        ModelStreamItem::TokenUsage(usage) => {
                            state.handle_token_usage(usage);
                        }
                        ModelStreamItem::Complete => break,
                        ModelStreamItem::Fallback { from, to } => {
                            on_event(Event::Model(ModelEvent::Fallback {
                                agent_id: self.agent_id.clone(),
                                from,
                                to,
                            }));
                        }
                    },
                    Ok(None) => break,
                    Err(e) => return Err(crate::agent::AgentError::from(e).into()),
                }
            }
        }

        let result = state.build_result();

        // Build the message from collected content blocks
        let mut msg = if result.content_blocks.is_empty() {
            Message::assistant(String::new())
        } else {
            Message::with_blocks(crate::types::Role::Assistant, result.content_blocks)
        };

        if !result.tool_calls.is_empty() {
            msg.tool_calls = Some(result.tool_calls);
        }

        Ok((msg, result.token_usage))
    }

    /// Execute a single tool call
    async fn execute_tool(
        &self,
        call: &ToolCall,
        cancel_token: &CancellationToken,
    ) -> Result<Message> {
        let tool = self
            .tool_registry
            .get(&call.name)
            .ok_or_else(|| KernelError::tool(format!("Tool '{}' not found", call.name)))?;

        let ctx = ToolExecCtx::with_parent_ctx(
            &call.id,
            None,
            Some(cancel_token.clone()),
            &self.working_dir,
        );

        let output = tool.exec(call.arguments.clone(), ctx).await?;

        Ok(Message::tool_result(call.id.clone(), output.text_content()))
    }
}
