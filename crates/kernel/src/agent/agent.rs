use super::message_buffer::MessageBuffer;
use super::{AgentExecutionContext, AgentHandle, AgentShared, AgentState, CancelToken};
use crate::event::{AgentEvent, AgentResult, ContentChunk, Event, ModelEvent, ToolEvent};
use crate::prompt::SystemPromptBuilder;
use crate::providers::ModelStreamItem;
use crate::skill::Skill;
use crate::tools::SubAgentTool;
use crate::types::{AgentId, ContentBlock, Message, Role, ToolCall};
use anyhow::Result;
use futures::TryStreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::Duration;

/// Input messages that can be sent to an Agent
#[derive(Debug, Clone)]
pub enum AgentInput {
    /// User message with multi-modal content blocks
    User(Vec<ContentBlock>),
    /// Tool execution result with multi-modal content blocks
    ToolResult {
        tool_id: String,
        content: Vec<ContentBlock>,
    },
    /// Background task completion
    TaskResult {
        task_id: String,
        content: Vec<ContentBlock>,
    },
    /// Cancel current operation
    Cancel,
}

pub struct Agent {
    id: AgentId,
    shared: Arc<AgentShared>,
    message_buffer: MessageBuffer,
    event_tx: mpsc::Sender<Event>,
    input_rx: mpsc::Receiver<AgentInput>,
    context: AgentExecutionContext,
    cancel_token: CancelToken,
    // Storage for message persistence
    storage: Option<Arc<dyn crate::storage::Storage>>,
    session_id: Option<String>,
    // Agent-specific configuration (not shared)
    max_iterations: usize,
    // Store the last error message for display
    last_error: Option<String>,
}

impl Agent {
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        id: AgentId,
        shared: &Arc<AgentShared>,
        base_prompt: impl AsRef<str>, // Base system prompt
        skills: Vec<Arc<Skill>>,      // Skills to include in system prompt
        history: Vec<Message>,        // Historical messages (may include old system message)
        storage: Option<Arc<dyn crate::storage::Storage>>,
        session_id: Option<String>,
        max_iterations: usize,
        enable_sub_agents: bool,
    ) -> (AgentHandle, mpsc::Receiver<Event>) {
        let (input_tx, input_rx) = mpsc::channel::<AgentInput>(10);
        let (event_tx, event_rx) = mpsc::channel(100);
        let cancel_token = CancelToken::new();
        let (context, state_rx) = AgentExecutionContext::new(AgentState::Idle);

        // Build system prompt with skills inside Agent
        let system_prompt = SystemPromptBuilder::new()
            .base_prompt(base_prompt.as_ref())
            .with_skills(&skills)
            .build();

        tracing::debug!(
            "Agent {} spawning with system prompt: {}",
            id,
            system_prompt
        );
        // Build MessageBuffer: system message + history (excluding old system messages)
        let mut messages = vec![Message::system(system_prompt)];
        messages.extend(history.into_iter().filter(|m| m.role != Role::System));
        let message_buffer = MessageBuffer::from_messages(messages);

        // Clone the shared resources with a new ToolRegistry for agent-specific tools
        let shared = {
            let new_shared = shared.with_cloned_registry();
            let working_dir =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

            // Register bash tool with background execution support
            let bash_ctx =
                crate::tools::BashToolCtx::new(id.clone(), input_tx.clone(), working_dir.clone());
            let bash_tool = crate::tools::BashTool::new(&working_dir).with_ctx(bash_ctx);
            new_shared.tool_registry.register(Arc::new(bash_tool));

            // Register subagent tool if enabled
            if enable_sub_agents {
                let input_tx_for_subagent = input_tx.clone();
                new_shared
                    .tool_registry
                    .register(Arc::new(SubAgentTool::new(
                        id.clone(),
                        Arc::new(new_shared.clone()),
                        input_tx_for_subagent,
                        skills,
                    )));
            }

            Arc::new(new_shared)
        };

        let agent = Self {
            id: id.clone(),
            shared,
            message_buffer,
            event_tx,
            input_rx,
            context,
            cancel_token: cancel_token.clone(),
            storage,
            session_id,
            max_iterations,
            last_error: None,
        };

        let handle_id = id.clone();
        tokio::spawn(async move {
            if let Err(e) = agent.run().await {
                tracing::error!("Agent {} failed: {}", handle_id, e);
            }
        });

        let handle = AgentHandle::new(id, input_tx, state_rx, cancel_token);
        (handle, event_rx)
    }

    /// Persist a single message to storage
    async fn persist_message(&self, message: &Message) {
        if let (Some(storage), Some(session_id)) = (&self.storage, &self.session_id) {
            let session_id = crate::types::SessionId(session_id.clone());
            let _ = storage
                .append_messages(&session_id, std::slice::from_ref(message))
                .await;
        }
    }

    async fn run(mut self) -> Result<()> {
        tracing::info!(
            "Agent {} started with {} messages",
            self.id,
            self.message_buffer.len()
        );

        self.context.transition_to(AgentState::WaitingForInput);
        loop {
            let state = self.context.current_state();

            if state.is_terminal() {
                break;
            }

            if self.context.iteration_count() >= self.max_iterations {
                tracing::warn!("Max iterations reached, forcing completion");
                self.context.transition_to(AgentState::Completed);
                break;
            }

            // Note: cancel is handled during streaming via select!, not here
            // This prevents the token from getting stuck in cancelled state

            match state {
                AgentState::WaitingForInput => {
                    tracing::debug!("Agent {} waiting for input", self.id);
                    if let Err(e) = self.handle_wait_for_input().await {
                        self.record_error("Wait for input failed", &e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                AgentState::Streaming => {
                    tracing::debug!("Agent {} starting streaming", self.id);
                    if let Err(e) = self.handle_streaming_with_retry().await {
                        self.record_error("Streaming failed", &e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                AgentState::ExecutingTool => {
                    tracing::info!("Agent {} executing tools", self.id);
                    if let Err(e) = self.handle_execute_tool().await {
                        self.record_error("Tool execution failed", &e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                _ => tokio::task::yield_now().await,
            }

            self.context.increment_iteration();
        }

        let tool_calls = self.count_tool_calls();
        let final_state = self.context.current_state();
        tracing::info!(
            "Agent {} finished: state={:?}, messages={}, tool_calls={}",
            self.id,
            final_state,
            self.message_buffer.len(),
            tool_calls
        );

        // Send appropriate event based on final state
        match final_state {
            AgentState::Cancelled => {
                let _ = self
                    .event_tx
                    .send(Event::Agent(AgentEvent::Cancelled {
                        agent_id: self.id.clone(),
                    }))
                    .await;
            }
            AgentState::Failed => {
                let error_msg = self
                    .last_error
                    .clone()
                    .unwrap_or_else(|| "Agent failed".to_string());
                let _ = self
                    .event_tx
                    .send(Event::Agent(AgentEvent::Failed {
                        agent_id: self.id.clone(),
                        error: error_msg,
                    }))
                    .await;
            }
            _ => {
                let result = AgentResult {
                    messages: self.message_buffer.messages().to_vec(),
                    tool_calls,
                };
                let _ = self
                    .event_tx
                    .send(Event::Agent(AgentEvent::Completed {
                        agent_id: self.id.clone(),
                        result,
                    }))
                    .await;
            }
        }
        Ok(())
    }

    /// Handle cancellation - sends Cancelled event, transitions state, returns Ok(())
    async fn handle_cancel(&self, context: &str) -> Result<()> {
        tracing::info!("Agent {} {} cancelled", self.id, context);
        let _ = self
            .event_tx
            .send(Event::Agent(AgentEvent::Cancelled {
                agent_id: self.id.clone(),
            }))
            .await;
        self.context.transition_to(AgentState::WaitingForInput);
        Ok(())
    }

    /// Record an error and store it for later display
    fn record_error(&mut self, context: &str, error: &anyhow::Error) {
        let msg = format!("{context}: {error}");
        tracing::error!("Agent {} failed: {}", self.id, msg);
        self.last_error = Some(msg);
    }

    /// Helper to emit `AgentEvent::Failed` and return error
    async fn fail_agent(&self, context: &str, error: anyhow::Error) -> Result<()> {
        let error_msg = format!("{context}: {error}");
        tracing::error!("Agent {} failed: {}", self.id, error_msg);
        let _ = self
            .event_tx
            .send(Event::Agent(AgentEvent::Failed {
                agent_id: self.id.clone(),
                error: error_msg,
            }))
            .await;
        Err(error)
    }

    async fn handle_wait_for_input(&mut self) -> Result<()> {
        match self.input_rx.recv().await {
            Some(AgentInput::User(content)) => {
                // Reset cancel token for new request
                self.cancel_token.reset();
                // Extract text content for event
                let text_content = content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("");
                let _ = self
                    .event_tx
                    .send(Event::User(crate::event::UserEvent::Message {
                        content: text_content,
                    }))
                    .await;
                let msg = Message::with_blocks(Role::User, content);
                self.persist_message(&msg).await;
                self.message_buffer.push(msg);
                self.context.transition_to(AgentState::Streaming);
                Ok(())
            }
            Some(AgentInput::ToolResult { tool_id, content }) => {
                let msg = Message::with_blocks(Role::Tool, content).with_tool_call_id(tool_id);
                self.persist_message(&msg).await;
                self.message_buffer.push(msg);
                self.context.transition_to(AgentState::Streaming);
                Ok(())
            }
            Some(AgentInput::TaskResult { task_id, content }) => {
                tracing::debug!("Task result received: {}", task_id);
                let msg = Message::with_blocks(Role::User, content);
                self.persist_message(&msg).await;
                self.message_buffer.push(msg);
                self.context.transition_to(AgentState::Streaming);
                Ok(())
            }
            Some(AgentInput::Cancel) => {
                self.cancel_token.cancel();
                self.context.transition_to(AgentState::Cancelled);
                Ok(())
            }
            None => {
                self.context.transition_to(AgentState::Cancelled);
                Ok(())
            }
        }
    }

    async fn handle_streaming(&mut self) -> Result<()> {
        let tools = self.shared.tool_registry.definitions();
        tracing::info!(
            "Agent {} preparing to stream with {} tool(s): {:?}",
            self.id,
            tools.len(),
            tools.iter().map(|t| &t.name).collect::<Vec<_>>()
        );
        let _ = self
            .event_tx
            .send(Event::Model(ModelEvent::Request {
                agent_id: self.id.clone(),
                message_count: self.message_buffer.len(),
            }))
            .await;

        let messages = self.message_buffer.messages();
        let stream_future = tokio::time::timeout(
            Duration::from_secs(10),
            self.shared
                .provider
                .stream(messages, &tools, &self.shared.model_config),
        );
        let mut stream = tokio::select! {
            biased;
            () = self.cancel_token.cancelled() => {
                return self.handle_cancel("stream creation").await;
            }
            result = stream_future => match result {
                Ok(Ok(stream)) => stream,
                Ok(Err(e)) => {
                    return self.fail_agent("Stream creation failed", e).await;
                }
                Err(_) => {
                    return self.fail_agent(
                        "Stream creation timed out",
                        anyhow::anyhow!("timeout after 10s"),
                    ).await;
                }
            }
        };

        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut current_text = String::new();
        let mut current_thinking = String::new();
        let mut pending_tool_calls: Vec<ToolCall> = Vec::new();

        // Stream with hard cancellation support using select!
        loop {
            tokio::select! {
                biased;
                () = self.cancel_token.cancelled() => {
                    drop(stream);
                    return self.handle_cancel("streaming").await;
                }
                item = stream.try_next() => match item {
                    Ok(Some(item)) => match item {
                            ModelStreamItem::Chunk(ContentChunk::Text(text)) => {
                                current_text.push_str(&text);
                                let _ = self
                                    .event_tx
                                    .send(Event::Model(ModelEvent::Chunk {
                                        agent_id: self.id.clone(),
                                        content: ContentChunk::Text(text),
                                    }))
                                    .await;
                            }
                            ModelStreamItem::Chunk(ContentChunk::Thinking {
                                thinking,
                                signature,
                            }) => {
                                current_thinking.push_str(&thinking);
                                let _ = self
                                    .event_tx
                                    .send(Event::Model(ModelEvent::Chunk {
                                        agent_id: self.id.clone(),
                                        content: ContentChunk::Thinking {
                                            thinking,
                                            signature,
                                        },
                                    }))
                                    .await;
                            }
                            ModelStreamItem::Chunk(ContentChunk::RedactedThinking) => {
                                content_blocks.push(ContentBlock::RedactedThinking {
                                    data: String::new(),
                                });
                            }
                            ModelStreamItem::ToolCall(request) => {
                                pending_tool_calls.push(ToolCall {
                                    id: request.id,
                                    name: request.name,
                                    arguments: request.arguments,
                                });
                            }
                            ModelStreamItem::Complete => break,
                            ModelStreamItem::Fallback { from, to } => {
                                let _ = self
                                    .event_tx
                                    .send(Event::Model(ModelEvent::Fallback {
                                        agent_id: self.id.clone(),
                                        from,
                                        to,
                                    }))
                                    .await;
                            }
                            ModelStreamItem::TokenUsage {
                                prompt_tokens,
                                completion_tokens,
                            } => {
                                let total_tokens = prompt_tokens + completion_tokens;
                                let _ = self
                                    .event_tx
                                    .send(Event::Model(ModelEvent::TokenUsage {
                                        agent_id: self.id.clone(),
                                        prompt_tokens,
                                        completion_tokens,
                                        total_tokens,
                                    }))
                                    .await;
                            }
                    },
                    Ok(None) => break,
                    Err(e) => return Err(e),
                }
            }
        }

        if !current_thinking.is_empty() {
            content_blocks.push(ContentBlock::Thinking {
                thinking: current_thinking,
                signature: None,
            });
        }
        if !current_text.is_empty() {
            content_blocks.push(ContentBlock::Text { text: current_text });
        }

        if !content_blocks.is_empty() || !pending_tool_calls.is_empty() {
            let mut msg = Message::with_blocks(Role::Assistant, content_blocks);
            if !pending_tool_calls.is_empty() {
                msg.tool_calls = Some(pending_tool_calls);
            }
            self.persist_message(&msg).await;
            self.message_buffer.push(msg);
        }

        if self
            .message_buffer
            .messages()
            .last()
            .and_then(|m| m.tool_calls.as_ref())
            .is_some()
        {
            let tool_count = self
                .message_buffer
                .messages()
                .last()
                .unwrap()
                .tool_calls
                .as_ref()
                .map_or(0, |c| c.len());
            tracing::info!(
                "Agent {} detected {} tool call(s), transitioning to ExecutingTool",
                self.id,
                tool_count
            );
            self.context.transition_to(AgentState::ExecutingTool);
        } else {
            tracing::debug!(
                "Agent {} streaming complete, waiting for next input",
                self.id
            );
            let _ = self
                .event_tx
                .send(Event::Model(ModelEvent::Complete {
                    agent_id: self.id.clone(),
                }))
                .await;
            self.context.transition_to(AgentState::WaitingForInput);
        }
        Ok(())
    }

    async fn handle_execute_tool(&mut self) -> Result<()> {
        let tool_calls = self
            .message_buffer
            .messages()
            .last()
            .and_then(|m| m.tool_calls.as_deref())
            .unwrap_or(&[]);

        for call in tool_calls {
            let args_str = serde_json::to_string(&call.arguments).ok();
            let _ = self
                .event_tx
                .send(Event::Tool(ToolEvent::Started {
                    agent_id: self.id.clone(),
                    tool_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    arguments: args_str,
                }))
                .await;
        }

        let results =
            crate::tools::execute_tools_parallel(&self.id, tool_calls, &self.shared.tool_registry)
                .await;

        for result in results {
            if self.cancel_token.is_cancelled() {
                return self.handle_cancel("tool execution").await;
            }
            let _ = self.event_tx.send(Event::Tool(result.event)).await;
            self.persist_message(&result.message).await;
            self.message_buffer.push(result.message);
        }

        self.context.transition_to(AgentState::Streaming);
        Ok(())
    }

    fn count_tool_calls(&self) -> usize {
        self.message_buffer
            .messages()
            .iter()
            .filter_map(|m| m.tool_calls.as_ref().map(|c| c.len()))
            .sum()
    }

    #[allow(dead_code)]
    fn messages(&self) -> &[Message] {
        self.message_buffer.messages()
    }

    async fn handle_streaming_with_retry(&mut self) -> Result<()> {
        let max_retries = 10;
        let mut attempt = 0;

        loop {
            match self.handle_streaming().await {
                Ok(()) => return Ok(()),
                Err(e) if attempt >= max_retries => {
                    tracing::error!("Streaming failed after {} attempts: {}", attempt, e);
                    return Err(e);
                }
                Err(e) if !Self::is_retryable_error(&e) => {
                    // Client errors (4xx) should not be retried
                    tracing::error!("Streaming failed with non-retryable error: {}", e);
                    return Err(e);
                }
                Err(e) => {
                    attempt += 1;
                    tracing::warn!("Streaming failed (attempt {}), retrying: {}", attempt, e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(500 * attempt as u64))
                        .await;
                }
            }
        }
    }

    /// Check if an error is retryable.
    fn is_retryable_error(error: &anyhow::Error) -> bool {
        if let Some(http_err) = error.downcast_ref::<crate::providers::HttpError>() {
            return http_err.is_retryable();
        }
        true // Unknown errors default to retryable
    }
}
