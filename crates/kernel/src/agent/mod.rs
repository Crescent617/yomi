mod cancel;
mod config;
mod context;
mod handle;
mod message_buffer;
mod state;
mod subagent;

pub use cancel::CancelToken;
pub use config::{AgentConfig, SubAgentMode};
pub use context::AgentExecutionContext;
pub use handle::AgentHandle;
pub use state::AgentState;
pub use subagent::SubAgentManager;

use message_buffer::MessageBuffer;

use crate::event::{AgentEvent, AgentResult, ContentChunk, Event, ModelEvent, ToolEvent};
use crate::provider::{ModelProvider, ModelStreamItem};
use crate::tool::{ToolRegistry, ToolSandbox};
use crate::types::{AgentId, ContentBlock, Message, Role, ToolCall};
use anyhow::Result;
use futures::TryStreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Input messages that can be sent to an Agent
#[derive(Debug, Clone)]
pub enum AgentInput {
    /// User message
    User(String),
    /// Tool execution result
    ToolResult { tool_id: String, output: String },
    /// Cancel current operation
    Cancel,
}

pub struct Agent {
    id: AgentId,
    config: AgentConfig,
    message_buffer: MessageBuffer,
    event_tx: mpsc::Sender<Event>,
    provider: Arc<dyn ModelProvider>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    #[allow(dead_code)]
    input_tx: mpsc::Sender<AgentInput>,
    input_rx: mpsc::Receiver<AgentInput>,
    context: AgentExecutionContext,
    cancel_token: CancelToken,
    token_usage: std::sync::Arc<std::sync::atomic::AtomicU64>,
    // Storage for message persistence
    storage: Option<Arc<dyn crate::storage::Storage>>,
    session_id: Option<String>,
}

impl Agent {
    pub fn spawn(
        id: AgentId,
        config: AgentConfig,
        provider: Arc<dyn ModelProvider>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
        storage: Option<Arc<dyn crate::storage::Storage>>,
        session_id: Option<String>,
    ) -> (AgentHandle, mpsc::Receiver<Event>) {
        let (input_tx, input_rx) = mpsc::channel::<AgentInput>(100);
        let (event_tx, event_rx) = mpsc::channel(1000);
        let cancel_token = CancelToken::new();
        let (context, state_rx) = AgentExecutionContext::new(AgentState::Idle);

        let mut message_buffer = MessageBuffer::new(100);
        message_buffer.push(Message::system(&config.system_prompt));

        let agent = Self {
            id: id.clone(),
            config,
            message_buffer,
            event_tx,
            provider,
            tool_registry,
            sandbox,
            input_tx: input_tx.clone(),
            input_rx,
            context,
            cancel_token: cancel_token.clone(),
            token_usage: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
            storage,
            session_id,
        };

        let handle_id = id.clone();
        tokio::spawn(async move {
            if let Err(e) = agent.run().await {
                tracing::error!("Agent {} failed: {}", handle_id.0, e);
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
                .append_messages(&session_id, &[message.clone()])
                .await;
        }
    }

    async fn run(mut self) -> Result<()> {
        tracing::info!("Agent {} started", self.id.0);

        // Load historical messages if storage and session_id are provided
        if let (Some(storage), Some(session_id)) = (&self.storage, &self.session_id) {
            let session_id_wrapped = crate::types::SessionId(session_id.clone());
            if let Ok(messages) = storage.get_messages(&session_id_wrapped).await {
                for msg in messages {
                    // Skip system message as it's already in the buffer
                    if msg.role != Role::System {
                        self.message_buffer.push(msg);
                    }
                }
                tracing::info!("Loaded {} messages from storage", self.message_buffer.len());
            }
        }

        self.context.transition_to(AgentState::WaitingForInput);
        loop {
            let state = self.context.current_state();

            if state.is_terminal() {
                break;
            }

            if self.context.iteration_count() >= self.config.max_iterations {
                tracing::warn!("Max iterations reached, forcing completion");
                self.context.transition_to(AgentState::Completed);
                break;
            }

            // Note: cancel is handled during streaming via select!, not here
            // This prevents the token from getting stuck in cancelled state

            match state {
                AgentState::WaitingForInput => {
                    tracing::debug!("Agent {} waiting for input", self.id.0);
                    if let Err(e) = self.handle_wait_for_input().await {
                        tracing::error!("Wait for input failed: {}", e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                AgentState::Streaming => {
                    tracing::debug!("Agent {} starting streaming", self.id.0);
                    if let Err(e) = self.handle_streaming_with_retry().await {
                        tracing::error!("Streaming failed after retries: {}", e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                AgentState::ExecutingTool => {
                    tracing::info!("Agent {} executing tools", self.id.0);
                    if let Err(e) = self.handle_execute_tool().await {
                        tracing::error!("Tool execution failed: {}", e);
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
            self.id.0,
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
                let _ = self
                    .event_tx
                    .send(Event::Agent(AgentEvent::Failed {
                        agent_id: self.id.clone(),
                        error: "Agent failed".to_string(),
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
    async fn handle_cancel(&mut self, context: &str) -> Result<()> {
        tracing::info!("Agent {} {} cancelled", self.id.0, context);
        let _ = self
            .event_tx
            .send(Event::Agent(AgentEvent::Cancelled {
                agent_id: self.id.clone(),
            }))
            .await;
        self.context.transition_to(AgentState::WaitingForInput);
        Ok(())
    }

    async fn handle_wait_for_input(&mut self) -> Result<()> {
        match self.input_rx.recv().await {
            Some(AgentInput::User(content)) => {
                // Reset cancel token for new request
                self.cancel_token.reset();
                let _ = self
                    .event_tx
                    .send(Event::User(crate::event::UserEvent::Message {
                        content: content.clone(),
                    }))
                    .await;
                let msg = Message::user(content);
                self.message_buffer.push(msg.clone());
                self.persist_message(&msg).await;
                self.context.transition_to(AgentState::Streaming);
                Ok(())
            }
            Some(AgentInput::ToolResult { tool_id, output }) => {
                let msg = Message::tool_result(&tool_id, output);
                self.message_buffer.push(msg.clone());
                self.persist_message(&msg).await;
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
        let tools = self.tool_registry.definitions();
        tracing::info!(
            "Agent {} preparing to stream with {} tool(s): {:?}",
            self.id.0,
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

        let messages_vec: Vec<_> = self.message_buffer.messages().to_vec();

        // Start the provider stream with cancellation support
        let mut stream = tokio::select! {
            biased;
            _ = self.cancel_token.cancelled() => {
                return self.handle_cancel("stream start").await;
            }
            result = self.provider.stream(&messages_vec, &tools, &self.config.model) => {
                result?
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
                _ = self.cancel_token.cancelled() => {
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
                                self.token_usage
                                    .fetch_add(u64::from(total_tokens), std::sync::atomic::Ordering::SeqCst);
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

        let estimated_tokens = current_text.len() / 4 + current_thinking.len() / 4;
        self.token_usage
            .fetch_add(estimated_tokens as u64, std::sync::atomic::Ordering::SeqCst);

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
            self.message_buffer.push(msg.clone());
            self.persist_message(&msg).await;
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
                self.id.0,
                tool_count
            );
            self.context.transition_to(AgentState::ExecutingTool);
        } else {
            tracing::debug!(
                "Agent {} streaming complete, waiting for next input",
                self.id.0
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
            .and_then(|m| m.tool_calls.clone())
            .unwrap_or_default();

        for call in &tool_calls {
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

        let results = crate::tools::execute_tools_parallel(
            &self.id,
            tool_calls,
            &self.tool_registry,
            &self.sandbox,
            self.sandbox.default_timeout(),
        )
        .await;

        for result in results {
            if self.cancel_token.is_cancelled() {
                return self.handle_cancel("tool execution").await;
            }
            let _ = self.event_tx.send(Event::Tool(result.event)).await;
            self.message_buffer.push(result.message.clone());
            self.persist_message(&result.message).await;
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
        let max_retries = 3;
        let mut attempt = 0;

        loop {
            match self.handle_streaming().await {
                Ok(()) => return Ok(()),
                Err(e) if attempt >= max_retries => {
                    tracing::error!("Streaming failed after {} attempts: {}", attempt, e);
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
}
