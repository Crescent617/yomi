//! Streaming response handling for Agent
//!
//! Handles collecting chunks and tool calls from model streams.

use super::stream_collector::{StreamCollectionResult, StreamCollectorState};
use super::AgentError;
use crate::compactor::DEFAULT_CONTEXT_WINDOW;
use crate::event::{Event, ModelEvent};
use crate::providers::{ModelConfig, ModelStream, ModelStreamItem, Provider};
use crate::types::{AgentId, Message, MessageId, MessageTokenUsage, Role};
use futures::TryStreamExt;
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Streaming handler for agent responses
pub struct StreamingHandler {
    /// Agent ID
    pub agent_id: AgentId,
    /// Event sender
    pub event_tx: mpsc::Sender<Event>,
    /// Model configuration
    pub model_config: Arc<ModelConfig>,
    /// Provider for API calls
    pub provider: Arc<dyn Provider>,
    /// Context window size (for token usage events)
    pub context_window: u32,
}

impl StreamingHandler {
    /// Create a new streaming handler
    pub fn new(
        agent_id: AgentId,
        event_tx: mpsc::Sender<Event>,
        model_config: Arc<ModelConfig>,
        provider: Arc<dyn Provider>,
        compactor: Option<&crate::compactor::Compactor>,
    ) -> Self {
        let context_window = compactor.map_or(DEFAULT_CONTEXT_WINDOW, |c| c.context_window);

        Self {
            agent_id,
            event_tx,
            model_config,
            provider,
            context_window,
        }
    }

    /// Start a streaming request to the provider
    pub async fn start_stream(
        &self,
        messages: Vec<Arc<Message>>,
        tools: Vec<crate::types::ToolDefinition>,
        cancel_token: &super::CancelToken,
    ) -> Result<ModelStream, AgentError> {
        let tools_arc: Vec<Arc<crate::types::ToolDefinition>> =
            tools.into_iter().map(Arc::new).collect();
        // Spawn provider request in a separate task to allow cancellation
        let provider = self.provider.clone();
        let model_config = self.model_config.clone();
        let stream_task =
            tokio::spawn(
                async move { provider.stream(&messages, &tools_arc, &model_config).await },
            );
        let abort_handle = stream_task.abort_handle();

        debug!("Agent {} waiting for model stream to start", self.agent_id);

        tokio::select! {
            biased;
            () = cancel_token.cancelled() => {
                abort_handle.abort();
                Err(AgentError::Cancelled("stream creation".into()))
            }
            result = stream_task => match result {
                Ok(Ok(stream)) => Ok(stream),
                Ok(Err(e)) => Err(AgentError::Provider(e)),
                Err(e) if e.is_cancelled() => Err(AgentError::Cancelled("stream creation".into())),
                Err(e) => Err(AgentError::StreamTaskPanicked(e.to_string())),
            }
        }
    }

    /// Collect all output from a stream until completion
    pub async fn collect_output(
        &self,
        stream: &mut ModelStream,
        message_id: MessageId,
        cancel_token: &super::CancelToken,
    ) -> Result<StreamCollectionResult, AgentError> {
        let mut state = StreamCollectorState::default();

        loop {
            tokio::select! {
                biased;
                () = cancel_token.cancelled() => {
                    return Err(AgentError::Cancelled("streaming".into()));
                }
                item = stream.try_next() => match item {
                    Ok(Some(item)) => {
                        self.handle_stream_item(item, &mut state, &message_id).await;
                    }
                    Ok(None) => break,
                    Err(e) => {
                        warn!("Agent {} stream error: {}", self.agent_id, e);
                        return Err(AgentError::Provider(e));
                    }
                }
            }
        }

        Ok(state.build_result())
    }

    /// Handle a single stream item
    async fn handle_stream_item(
        &self,
        item: ModelStreamItem,
        state: &mut StreamCollectorState,
        message_id: &MessageId,
    ) {
        match item {
            ModelStreamItem::Chunk(chunk) => {
                state.handle_chunk(&chunk);
                if let Err(e) = self.event_tx.try_send(Event::Model(ModelEvent::Chunk {
                    agent_id: self.agent_id.clone(),
                    message_id: message_id.clone(),
                    content: chunk,
                })) {
                    warn!("Failed to send chunk event: {}", e);
                }
            }
            ModelStreamItem::ToolCallDelta {
                id,
                name,
                arguments_delta,
            } => {
                if let Err(e) = self
                    .event_tx
                    .try_send(Event::Model(ModelEvent::ToolCallDelta {
                        agent_id: self.agent_id.clone(),
                        message_id: message_id.clone(),
                        tool_id: id,
                        tool_name: name,
                        arguments_delta,
                    }))
                {
                    warn!("Failed to send tool call delta event: {}", e);
                }
            }
            ModelStreamItem::ToolCall(request) => {
                state.handle_tool_call(request);
            }
            ModelStreamItem::Complete | ModelStreamItem::ResponseMeta { .. } => {
                // Response metadata not used in streaming
            }
            ModelStreamItem::Fallback { from, to } => {
                if let Err(e) = self.event_tx.try_send(Event::Model(ModelEvent::Fallback {
                    agent_id: self.agent_id.clone(),
                    message_id: message_id.clone(),
                    from,
                    to,
                })) {
                    warn!("Failed to send fallback event: {}", e);
                }
            }
            ModelStreamItem::TokenUsage(usage) => {
                let total = usage.total_tokens();
                state.handle_token_usage(usage);

                if let Err(e) = self.event_tx.try_send(Event::Model(ModelEvent::TokenUsage {
                    agent_id: self.agent_id.clone(),
                    message_id: message_id.clone(),
                    prompt_tokens: usage.prompt_tokens,
                    completion_tokens: usage.completion_tokens,
                    total_tokens: total,
                    context_window: self.context_window,
                })) {
                    warn!("Failed to send token usage event: {}", e);
                }
            }
        }
    }

    /// Build a message from stream result
    pub fn build_message(&self, result: &StreamCollectionResult, message_id: MessageId) -> Message {
        let mut msg = Message::with_blocks(Role::Assistant, result.content_blocks.clone());
        msg.id = message_id;

        if !result.tool_calls.is_empty() {
            msg.tool_calls = Some(result.tool_calls.clone());
        }

        if let Some(usage) = result.token_usage {
            msg.token_usage = Some(MessageTokenUsage {
                prompt_tokens: usage.prompt_tokens,
                completion_tokens: usage.completion_tokens,
                total_tokens: usage.total_tokens(),
            });
        }

        if let Some(ref response_id) = result.response_id {
            msg.response_id = Some(response_id.clone());
        }

        if let Some(fr) = result.finish_reason {
            msg.finish_reason = Some(fr);
        }

        msg
    }

    /// Send completion event
    pub async fn send_completion_event(&self, message_id: &MessageId) {
        if let Err(e) = self.event_tx.try_send(Event::Model(ModelEvent::Completed {
            agent_id: self.agent_id.clone(),
            message_id: message_id.clone(),
        })) {
            warn!("Failed to send completed event: {}", e);
        }
    }
}
