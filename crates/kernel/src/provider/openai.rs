use crate::event::ContentChunk;
use crate::provider::{
    HttpError, ModelConfig, ModelStream, ModelStreamItem, Provider, ProviderError, ToolCallRequest,
};
use crate::types::{FinishReason, Message, Result, Role, ToolDefinition};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::{self, StreamExt, TryStreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

// Stream transformer: SSE -> ModelStreamItem
// Accumulates tool calls, emits content immediately
// 2-minute idle timeout to detect stalled connections
const IDLE_TIMEOUT: Duration = Duration::from_mins(2);

pub struct OpenAIProvider {
    client: Arc<Client>,
    name: String,
}

impl OpenAIProvider {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: crate::provider::http_client(),
            name: "openai".to_string(),
        })
    }

    fn convert_messages(messages: &[Arc<Message>]) -> Vec<OpenAIMessage> {
        messages
            .iter()
            .filter(|m| !matches!(m.as_ref().role, Role::Internal))
            .map(|m| {
                let m = m.as_ref();

                let blocks: Vec<_> = m
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        crate::types::ContentBlock::Text { text } if !text.is_empty() => {
                            Some(OpenAIContentBlock {
                                type_: "text".into(),
                                text: Some(text.clone()),
                                image_url: None,
                            })
                        }
                        crate::types::ContentBlock::ImageUrl { image_url } => {
                            Some(OpenAIContentBlock {
                                type_: "image_url".into(),
                                text: None,
                                image_url: Some(OpenAIImageUrl {
                                    url: image_url.url.clone(),
                                    detail: image_url.detail.clone(),
                                }),
                            })
                        }
                        _ => None,
                    })
                    .collect();

                let reasoning_content = m
                    .content
                    .iter()
                    .find_map(|c| match c {
                        crate::types::ContentBlock::Thinking { thinking, .. } => {
                            Some(thinking.clone())
                        }
                        _ => None,
                    })
                    .unwrap_or_default();

                let role = match m.role {
                    Role::System => "system",
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::Tool => "tool",
                    Role::Internal => unreachable!("Internal messages should be filtered out"),
                };

                let tool_calls = m.tool_calls.as_ref().map(|calls| {
                    calls
                        .iter()
                        .map(|c| OpenAIToolCall {
                            index: None,
                            id: Some(c.id.clone()),
                            type_: Some("function".into()),
                            function: OpenAIFunction {
                                name: Some(c.name.clone()),
                                arguments: Some(c.arguments.to_string()),
                            },
                        })
                        .collect()
                });

                OpenAIMessage {
                    role: role.into(),
                    content: OpenAIContent::Blocks(blocks),
                    reasoning_content: Some(reasoning_content),
                    tool_calls,
                    tool_call_id: m.tool_call_id.clone(),
                }
            })
            .collect()
    }

    fn convert_tools(tools: &[Arc<ToolDefinition>]) -> Vec<OpenAITool> {
        tools
            .iter()
            .map(|t| OpenAITool {
                type_: "function".to_string(),
                function: OpenAIFunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect()
    }
}

#[async_trait]
impl Provider for OpenAIProvider {
    async fn stream(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
    ) -> std::result::Result<ModelStream, ProviderError> {
        let url = if config.endpoint.is_empty() {
            "https://api.openai.com/v1/chat/completions".to_string()
        } else {
            format!("{}/chat/completions", config.endpoint.trim_end_matches('/'))
        };

        tracing::debug!(
            "OpenAI API request: model={}, messages={}, tools={}",
            config.model_id,
            messages.len(),
            tools.len()
        );

        // Check if any message contains an image (for debug logging)
        let has_image = messages.iter().any(|m| {
            m.as_ref()
                .content
                .iter()
                .any(|c| matches!(c, crate::types::ContentBlock::ImageUrl { .. }))
        });

        // Calls from Agent/Compactor resolve this before entering the provider.
        // The provider itself only serializes the supplied config.
        let request_body = OpenAIRequest {
            model: config.model_id.clone(),
            messages: Self::convert_messages(messages),
            tools: if tools.is_empty() {
                None
            } else {
                Some(Self::convert_tools(tools))
            },
            stream: true,
            stream_options: Some(StreamOptions {
                include_usage: true,
            }),
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            reasoning_effort: if config.thinking.enabled {
                Some(
                    config
                        .thinking
                        .effort
                        .clone()
                        .unwrap_or_else(|| "medium".to_string()),
                )
            } else {
                None
            },
            has_image,
        };

        // Debug: log request body for vision requests
        if request_body.has_image {
            let body_json = serde_json::to_string_pretty(&request_body).unwrap_or_default();
            // Truncate base64 data for readability
            let truncated = body_json.replace(|c: char| c.is_ascii() && c.is_control(), "");
            tracing::debug!(
                "OpenAI request with image: {}",
                &truncated[..truncated.len().min(500)]
            );
        }

        let request = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body);

        // Inject custom headers from config
        let request = config
            .headers
            .iter()
            .fold(request, |req, (k, v)| req.header(k, v));
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            // Truncate error message if too long
            let truncated = if text.len() > 200 {
                format!("{}... (truncated)", &text[..200])
            } else {
                text
            };
            tracing::error!("OpenAI API error: {} - {}", status, truncated);
            return Err(ProviderError::Http(HttpError(status.as_u16())));
        }

        tracing::debug!("OpenAI API response received, starting stream processing");

        let eventsource = response.bytes_stream().eventsource();

        let stream = stream::try_unfold(
            (
                eventsource,
                MsgChunkAssembler::new(),
                tokio::time::Instant::now(),
            ),
            |(mut eventsource, mut assembler, last_content_time)| async move {
                loop {
                    if assembler.finished {
                        return Ok(None);
                    }
                    let elapsed = last_content_time.elapsed();
                    // Adjust timeout based on elapsed time since last content
                    let Some(remaining) = IDLE_TIMEOUT.checked_sub(elapsed) else {
                        tracing::error!(
                            "OpenAI SSE content stall: no content for {}s",
                            elapsed.as_secs()
                        );
                        return Err(ProviderError::Timeout(format!(
                            "Content stall: no meaningful data received for {} seconds",
                            elapsed.as_secs()
                        )));
                    };
                    match timeout(remaining, eventsource.try_next()).await {
                        Ok(Ok(Some(event))) => {
                            if event.data == "[DONE]" {
                                let items = assembler.finish_stream("[DONE]")?;
                                return Ok(Some((
                                    items,
                                    (eventsource, assembler, last_content_time),
                                )));
                            }

                            let items = assembler.process(&event.data);
                            if !items.is_empty() {
                                // Reset content timer when we actually produce items
                                return Ok(Some((
                                    items,
                                    (eventsource, assembler, tokio::time::Instant::now()),
                                )));
                            }
                            // No content produced, continue loop with same timer
                        }
                        Ok(Ok(None)) => {
                            tracing::error!("OpenAI stream ended before final finish_reason");
                            let items = assembler.finish_stream("EOF")?;
                            return Ok(Some((items, (eventsource, assembler, last_content_time))));
                        }
                        Ok(Err(e)) => {
                            tracing::error!("OpenAI SSE error: {}", e);
                            return Err(ProviderError::Sse(format!("SSE error: {e}")));
                        }
                        Err(_) => {
                            tracing::error!(
                                "OpenAI SSE idle timeout after {}s",
                                IDLE_TIMEOUT.as_secs()
                            );
                            return Err(ProviderError::Timeout(format!(
                                "SSE idle timeout: no data received for {} seconds",
                                IDLE_TIMEOUT.as_secs()
                            )));
                        }
                    }
                }
            },
        )
        .flat_map(
            |result: std::result::Result<Vec<ModelStreamItem>, ProviderError>| {
                let items: Vec<std::result::Result<ModelStreamItem, ProviderError>> = match result {
                    Ok(items) => items.into_iter().map(Ok).collect(),
                    Err(e) => vec![Err(e)],
                };
                stream::iter(items)
            },
        )
        .boxed();

        Ok(stream)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Assembles partial tool calls from SSE chunks into complete `ToolCallRequest` objects.
///
/// `OpenAI`'s streaming format sends tool calls incrementally:
/// - Each chunk may contain deltas for one or more tool calls (identified by index)
/// - Arguments arrive as partial JSON strings across multiple chunks
/// - A tool call is complete when we receive a chunk with a higher index, or at stream end
///
/// This struct tracks partial state and determines when calls are ready to emit.
struct MsgChunkAssembler {
    /// Partial tool calls by index
    partials: HashMap<usize, PartialToolCall>,
    /// The highest index we've seen so far. Used to detect when lower indices are complete.
    max_seen_index: Option<usize>,
    /// API response ID (from the first chunk that has it)
    response_id: Option<String>,
    /// Finish reason from the final chunk
    finish_reason: Option<String>,
    /// Whether the authoritative stream terminator has been processed
    finished: bool,
}

/// Accumulated state for a single tool call
#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl MsgChunkAssembler {
    fn new() -> Self {
        Self {
            partials: HashMap::new(),
            max_seen_index: None,
            response_id: None,
            finish_reason: None,
            finished: false,
        }
    }

    /// Process an SSE chunk, returning any items that can be emitted immediately.
    ///
    /// Content (text/thinking) is emitted immediately as it arrives.
    /// Tool calls are accumulated; completed calls are emitted when we detect they're finished.
    fn process(&mut self, data: &str) -> Vec<ModelStreamItem> {
        let response: OpenAIStreamResponse = match serde_json::from_str(data) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("Failed to parse SSE chunk: {e} - data: {data}");
                return Vec::new();
            }
        };

        // Capture response ID from any chunk that has it
        if let Some(id) = response.id {
            self.response_id = Some(id);
        }

        let mut items = Vec::new();

        // Process the first choice if present
        if let Some(choice) = response.choices.into_iter().next() {
            // Capture finish_reason from the final chunk
            if let Some(finish_reason) = choice.finish_reason {
                self.finish_reason = Some(finish_reason);
            }

            // Some providers put usage inside the choice
            if let Some(usage) = choice.usage {
                items.push(ModelStreamItem::TokenUsage(
                    crate::provider::TokenUsage::new(
                        usage.prompt_tokens,
                        usage.completion_tokens,
                        usage.cached_tokens(),
                    ),
                ));
            }

            if let Some(delta) = choice.delta {
                // Handle tool call deltas
                if let Some(calls) = delta.tool_calls {
                    for call in calls {
                        let index = call.index.unwrap_or(0);

                        // Update max seen index
                        self.max_seen_index =
                            Some(self.max_seen_index.map_or(index, |m| m.max(index)));

                        // Check if this is a new index - previous indices are now complete
                        if index > 0 {
                            if let Some(completed) = self.try_complete(index - 1) {
                                items.push(ModelStreamItem::ToolCall(completed));
                            }
                        }

                        // Accumulate this call's data
                        let partial = self.partials.entry(index).or_default();
                        if let Some(id) = call.id.filter(|s| !s.is_empty()) {
                            partial.id = Some(id);
                        }
                        if let Some(name) = call.function.name.filter(|s| !s.is_empty()) {
                            partial.name = Some(name);
                        }

                        // Emit incremental update for UI feedback if we have:
                        // 1. args delta in this chunk, and
                        // 2. accumulated id from previous chunks (name may come later)
                        if let Some(args) = call.function.arguments {
                            if !args.is_empty() {
                                partial.arguments.push_str(&args);
                                if let Some(id) = &partial.id {
                                    items.push(ModelStreamItem::ToolCallDelta {
                                        id: id.clone(),
                                        name: partial.name.clone().unwrap_or_default(),
                                        arguments_delta: args,
                                    });
                                }
                            }
                        }
                    }
                }

                // Handle content deltas (always emitted immediately)
                if let Some(thinking) = delta
                    .thinking
                    .or(delta.reasoning)
                    .or(delta.reasoning_content)
                {
                    items.push(ModelStreamItem::Chunk(ContentChunk::Thinking {
                        thinking,
                        signature: delta.thinking_signature,
                    }));
                }

                if delta.thinking_redacted.unwrap_or(false) {
                    items.push(ModelStreamItem::Chunk(ContentChunk::RedactedThinking));
                }

                if let Some(content) = delta.content.filter(|c| !c.is_empty()) {
                    items.push(ModelStreamItem::Chunk(ContentChunk::Text(content)));
                }
            }
        }

        // Handle top-level usage information
        if let Some(usage) = response.usage {
            items.push(ModelStreamItem::TokenUsage(
                crate::provider::TokenUsage::new(
                    usage.prompt_tokens,
                    usage.completion_tokens,
                    usage.cached_tokens(),
                ),
            ));
        }

        items
    }

    fn finish_stream(
        &mut self,
        end: &str,
    ) -> std::result::Result<Vec<ModelStreamItem>, ProviderError> {
        if self.finish_reason.is_none() {
            return Err(Self::unexpected_end(end));
        }
        self.finished = true;
        Ok(self.finish())
    }

    /// Called after an authoritative terminal marker. Returns all remaining complete tool calls and a Complete marker.
    fn finish(&mut self) -> Vec<ModelStreamItem> {
        let mut items = Vec::new();

        // Collect all remaining indices in order
        let mut indices: Vec<_> = self.partials.keys().copied().collect();
        indices.sort_unstable();

        for idx in indices {
            if let Some(completed) = self.try_complete(idx) {
                items.push(ModelStreamItem::ToolCall(completed));
            }
        }

        let finish_reason = self
            .finish_reason
            .take()
            .and_then(|s| FinishReason::from_provider_str(&s));
        if self.response_id.is_some() || finish_reason.is_some() {
            items.push(ModelStreamItem::ResponseMeta {
                response_id: self.response_id.take(),
                finish_reason,
            });
        }

        items.push(ModelStreamItem::Complete);
        items
    }

    fn unexpected_end(end: &str) -> ProviderError {
        ProviderError::Sse(format!("protocol error: {end} before final finish_reason"))
    }

    /// Try to complete a tool call at the given index.
    /// Returns Some if the call has enough data to be considered complete.
    fn try_complete(&mut self, index: usize) -> Option<ToolCallRequest> {
        let partial = self.partials.remove(&index)?;

        let id = partial.id?;
        let name = partial.name?;

        // Try to parse arguments as JSON. If it fails, treat as string.
        let arguments =
            serde_json::from_str(&partial.arguments).unwrap_or(Value::String(partial.arguments));

        Some(ToolCallRequest {
            id,
            name,
            arguments,
        })
    }
}

// OpenAI API types
#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    /// Reasoning effort for o1/o3 models (low/medium/high)
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_effort: Option<String>,
    /// Track if request contains images (for debug logging, not serialized)
    #[serde(skip)]
    has_image: bool,
}

#[derive(Debug, Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(untagged)]
enum OpenAIContent {
    /// Plain text content (simple mode)
    Text(String),
    /// Multi-modal content blocks (vision mode)
    Blocks(Vec<OpenAIContentBlock>),
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIContentBlock {
    #[serde(rename = "type")]
    type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    image_url: Option<OpenAIImageUrl>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIImageUrl {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    content: OpenAIContent,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning_content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAITool {
    #[serde(rename = "type")]
    type_: String,
    function: OpenAIFunctionDef,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionDef {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCall {
    #[serde(skip_serializing_if = "Option::is_none")]
    index: Option<usize>,
    id: Option<String>,
    #[serde(rename = "type")]
    type_: Option<String>,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunction {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIStreamResponse {
    /// Response ID from API (e.g., "chatcmpl-xxx")
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    choices: Vec<OpenAIChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OpenAIUsage>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    /// Cached tokens in `prompt_tokens_details` (OpenAI-compatible unified format)
    #[serde(skip_serializing_if = "Option::is_none")]
    prompt_tokens_details: Option<OpenAIPromptTokensDetails>,
}

impl OpenAIUsage {
    /// Get cached tokens from `prompt_tokens_details`
    fn cached_tokens(&self) -> Option<u32> {
        self.prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIPromptTokensDetails {
    cached_tokens: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIChoice {
    delta: Option<OpenAIDelta>,
    /// Some providers include usage in the choice
    #[serde(skip_serializing_if = "Option::is_none")]
    usage: Option<OpenAIUsage>,
    /// Finish reason from API (e.g., "stop", "length", "`content_filter`")
    #[serde(skip_serializing_if = "Option::is_none")]
    finish_reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIDelta {
    content: Option<String>,
    thinking: Option<String>,
    reasoning: Option<String>,
    reasoning_content: Option<String>,
    #[serde(rename = "thinking_signature")]
    thinking_signature: Option<String>,
    #[serde(rename = "thinking_redacted")]
    thinking_redacted: Option<bool>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

#[cfg(test)]
#[path = "openai_test.rs"]
mod tests;
