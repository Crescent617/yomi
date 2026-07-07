//! Implementation of the `Provider` trait for Anthropic's API
use crate::event::ContentChunk;
use crate::provider::{
    HttpError, ModelConfig, ModelStream, ModelStreamItem, Provider, ProviderError, ToolCallRequest,
};
use crate::types::{ContentBlock, FinishReason, Message, Result, Role, ToolDefinition};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::{self, StreamExt, TryStreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

const IDLE_TIMEOUT: Duration = Duration::from_mins(2);

pub struct AnthropicProvider {
    client: Client,
    name: String,
}

impl AnthropicProvider {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .build()?,
            name: "anthropic".to_string(),
        })
    }

    fn convert_messages(messages: &[Arc<Message>]) -> Vec<AnthropicMessage> {
        messages
            .iter()
            .filter_map(|m| {
                let role = match m.role {
                    Role::System | Role::Internal => return None, // System is handled separately
                    Role::User | Role::Tool => "user",
                    Role::Assistant => "assistant",
                };

                // Handle tool result messages - wrap content in tool_result blocks
                let content = if let Some(ref tool_call_id) = m.tool_call_id {
                    if tool_call_id.is_empty() {
                        tracing::warn!("Tool result message has empty tool_call_id, treating as regular user message");
                        Self::convert_content_blocks(&m.content)
                    } else {
                        tracing::debug!("Converting tool result message with tool_call_id: {}", tool_call_id);
                        let text_content = m.text_content();
                        vec![AnthropicContent::ToolResult {
                            tool_use_id: tool_call_id.clone(),
                            content: text_content,
                        }]
                    }
                } else {
                    let mut content = Self::convert_content_blocks(&m.content);

                    // For assistant messages, add tool_calls as tool_use blocks
                    if m.role == Role::Assistant {
                        if let Some(ref tool_calls) = m.tool_calls {
                            for tool_call in tool_calls {
                                content.push(AnthropicContent::ToolUse {
                                    id: tool_call.id.clone(),
                                    name: tool_call.name.clone(),
                                    input: tool_call.arguments.clone(),
                                });
                            }
                        }
                    }

                    content
                };

                // Skip if still empty after processing
                if content.is_empty() {
                    return None;
                }

                Some(AnthropicMessage {
                    role: role.to_string(),
                    content,
                })
            })
            .collect()
    }

    /// Parse a data URL to extract media type and base64 data
    /// Format: data:image/{format};base64,{data}
    fn parse_data_url(url: &str) -> Option<(String, String)> {
        if !url.starts_with("data:image/") {
            // Not a data URL, skip
            return None;
        }

        // Remove "data:image/" prefix
        let without_prefix = &url[11..];

        // Find the semicolon separating media type from base64
        let semicolon_pos = without_prefix.find(';')?;
        let media_type = format!("image/{}", &without_prefix[..semicolon_pos]);

        // Check for base64 marker
        let after_semicolon = &without_prefix[semicolon_pos + 1..];
        if !after_semicolon.starts_with("base64,") {
            return None;
        }

        // Extract base64 data
        let base64_data = &after_semicolon[7..]; // Skip "base64,"

        Some((media_type, base64_data.to_string()))
    }

    fn convert_content_blocks(blocks: &[ContentBlock]) -> Vec<AnthropicContent> {
        let mut content = Vec::new();

        // Add content blocks
        for block in blocks {
            match block {
                ContentBlock::Text { text } if !text.is_empty() => {
                    content.push(AnthropicContent::Text { text: text.clone() });
                }
                ContentBlock::Thinking {
                    thinking,
                    signature: Some(ref sig),
                } if !thinking.is_empty() && !sig.is_empty() => {
                    // Preserve thinking blocks for conversation continuity,
                    // but only if we have a valid signature. Anthropic rejects
                    // thinking blocks with missing or empty signatures.
                    content.push(AnthropicContent::Thinking {
                        thinking: thinking.clone(),
                        signature: sig.clone(),
                    });
                }
                ContentBlock::RedactedThinking { data } => {
                    content.push(AnthropicContent::RedactedThinking { data: data.clone() });
                }
                ContentBlock::ImageUrl { image_url } => {
                    // Parse data URL to extract media type and base64 data
                    // Format: data:image/{format};base64,{data}
                    if let Some((media_type, base64_data)) = Self::parse_data_url(&image_url.url) {
                        content.push(AnthropicContent::Image {
                            source: AnthropicImageSource {
                                type_: "base64".to_string(),
                                media_type,
                                data: base64_data,
                            },
                        });
                    }
                }
                // ContentBlock::Audio is not supported by Anthropic API, skip it
                // ContentBlock::Text with empty text is intentionally skipped
                _ => {}
            }
        }

        content
    }

    fn convert_tools(tools: &[Arc<ToolDefinition>]) -> Vec<AnthropicTool> {
        tools
            .iter()
            .map(|t| AnthropicTool {
                name: t.name.clone(),
                description: t.description.clone(),
                input_schema: t.parameters.clone(),
            })
            .collect()
    }

    fn extract_system_message(messages: &[Arc<Message>]) -> Option<String> {
        messages.iter().find_map(|m| {
            if m.role == Role::System {
                Some(m.text_content())
            } else {
                None
            }
        })
    }
}

#[async_trait]
impl Provider for AnthropicProvider {
    async fn stream(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
    ) -> std::result::Result<ModelStream, ProviderError> {
        let url = if config.endpoint.is_empty() {
            "https://api.anthropic.com/v1/messages".to_string()
        } else {
            format!("{}/messages", config.endpoint.trim_end_matches('/'))
        };

        tracing::debug!(
            "Anthropic API request: model={}, messages={}, tools={}, thinking={}",
            config.model_id,
            messages.len(),
            tools.len(),
            config.thinking.enabled
        );

        let system = Self::extract_system_message(messages);

        // Debug: log original messages before conversion
        tracing::debug!(
            "Anthropic original messages: {:?}",
            messages
                .iter()
                .map(|m| {
                    (
                        m.role,
                        m.tool_call_id.clone(),
                        m.tool_calls
                            .as_ref()
                            .map(|tc| tc.iter().map(|t| t.id.clone()).collect::<Vec<_>>()),
                    )
                })
                .collect::<Vec<_>>()
        );

        let messages = Self::convert_messages(messages);

        // Debug: log converted messages to verify tool result formatting
        tracing::debug!(
            "Anthropic converted messages: {}",
            serde_json::to_string_pretty(&messages).unwrap_or_default()
        );

        // Set a default max_tokens if not provided
        let max_tokens = config.max_tokens.or(Some(8192));
        let mut request_body = AnthropicRequest {
            model: config.model_id.clone(),
            max_tokens,
            messages,
            system,
            tools: if tools.is_empty() {
                None
            } else {
                Some(Self::convert_tools(tools))
            },
            stream: true,
            temperature: config.temperature,
            thinking: None,
            output_config: None,
        };

        // Enable thinking if configured
        if config.thinking.enabled {
            request_body.thinking = Some(AnthropicThinking {
                type_: "adaptive".to_string(),
                budget_tokens: config.thinking.budget_tokens,
            });
        }

        // Set output_config effort if configured
        if let Some(ref effort) = config.thinking.effort {
            request_body.output_config = Some(AnthropicOutputConfig {
                effort: effort.clone(),
            });
        }

        let request = self
            .client
            .post(&url)
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&request_body);

        // Inject custom headers from config
        let request = config
            .headers
            .iter()
            .fold(request, |req, (k, v)| req.header(k, v));

        tracing::debug!("Sending request to Anthropic API at {}", url);
        tracing::debug!("Request body: {:?}", request_body);

        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::error!("Anthropic API error: {} - {}", status, text);
            return Err(ProviderError::Http(HttpError(status.as_u16())));
        }

        tracing::debug!("Anthropic API response received, starting stream processing");

        let eventsource = response.bytes_stream().eventsource();

        let stream = stream::try_unfold(
            (
                eventsource,
                AnthropicStreamState::new(),
                tokio::time::Instant::now(),
            ),
            |(mut eventsource, mut state, last_content_time)| async move {
                loop {
                    let elapsed = last_content_time.elapsed();
                    // Adjust timeout based on elapsed time since last content
                    let Some(remaining) = IDLE_TIMEOUT.checked_sub(elapsed) else {
                        tracing::error!(
                            "Anthropic SSE content stall: no content for {}s",
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
                                let items = state.finish();
                                return Ok(Some((items, (eventsource, state, last_content_time))));
                            }

                            let items = state.process(&event.data)?;
                            if !items.is_empty() {
                                // Reset content timer when we actually produce items
                                return Ok(Some((
                                    items,
                                    (eventsource, state, tokio::time::Instant::now()),
                                )));
                            }
                            // No content produced, continue loop with same timer
                        }
                        Ok(Ok(None)) => {
                            tracing::debug!("Anthropic stream ended normally");
                            let items = state.finish();
                            return Ok(Some((items, (eventsource, state, last_content_time))));
                        }
                        Ok(Err(e)) => {
                            tracing::error!("Anthropic SSE error: {}", e);
                            return Err(ProviderError::Sse(format!("SSE error: {e}")));
                        }
                        Err(_) => {
                            tracing::error!(
                                "Anthropic SSE idle timeout after {}s",
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

/// Tracks the state of an Anthropic streaming response
struct AnthropicStreamState {
    current_tool_call: Option<PartialToolCall>,
    accumulated_text: String,
    accumulated_thinking: String,
    input_tokens: Option<u32>,
    cache_read_input_tokens: Option<u32>,
    output_tokens: Option<u32>,
    /// API response ID (from `message_start` event)
    response_id: Option<String>,
    /// Stop reason from `message_delta` (e.g., "`end_turn`", "`max_tokens`", "`stop_sequence`")
    stop_reason: Option<String>,
    /// Whether `TokenUsage` has been emitted
    token_usage_emitted: bool,
}

struct PartialToolCall {
    id: String,
    name: String,
    input_json: String,
}

impl AnthropicStreamState {
    const fn new() -> Self {
        Self {
            current_tool_call: None,
            accumulated_text: String::new(),
            accumulated_thinking: String::new(),
            input_tokens: None,
            cache_read_input_tokens: None,
            output_tokens: None,
            response_id: None,
            stop_reason: None,
            token_usage_emitted: false,
        }
    }

    fn process(&mut self, data: &str) -> std::result::Result<Vec<ModelStreamItem>, ProviderError> {
        let event: AnthropicStreamEvent = serde_json::from_str(data).map_err(|e| {
            ProviderError::Parse(format!("Failed to parse SSE chunk: {e} - data: {data}"))
        })?;

        let mut items = Vec::new();

        match event {
            AnthropicStreamEvent::MessageStart { message } => {
                // Store input tokens and cache read tokens from message_start event
                self.input_tokens = Some(message.usage.input_tokens);
                self.cache_read_input_tokens = Some(message.usage.cache_read_input_tokens);
                // Capture response ID from message_start
                self.response_id = Some(message.id);
                // Capture stop_reason if already set (usually null at start)
                self.stop_reason = message.stop_reason;
            }
            AnthropicStreamEvent::Ping => {}
            AnthropicStreamEvent::ContentBlockStart { content_block, .. } => match content_block {
                AnthropicContent::Text { text } => {
                    self.accumulated_text = text;
                }
                AnthropicContent::ToolUse { id, name, .. } => {
                    self.current_tool_call = Some(PartialToolCall {
                        id,
                        name,
                        input_json: String::new(),
                    });
                }
                _ => {}
            },
            AnthropicStreamEvent::ContentBlockDelta { delta, .. } => {
                match delta {
                    AnthropicDelta::TextDelta { text } => {
                        self.accumulated_text.push_str(&text);
                        items.push(ModelStreamItem::Chunk(ContentChunk::Text(text)));
                    }
                    AnthropicDelta::ThinkingDelta { thinking } => {
                        self.accumulated_thinking.push_str(&thinking);
                        items.push(ModelStreamItem::Chunk(ContentChunk::Thinking {
                            thinking,
                            signature: None,
                        }));
                    }
                    AnthropicDelta::SignatureDelta { .. } => {
                        // Signature is stored but not emitted as a separate event
                    }
                    AnthropicDelta::InputJsonDelta { partial_json } => {
                        if let Some(ref mut tool) = self.current_tool_call {
                            // `partial_json` is the delta fragment from SSE
                            tool.input_json.push_str(&partial_json);
                            items.push(ModelStreamItem::ToolCallDelta {
                                id: tool.id.clone(),
                                name: tool.name.clone(),
                                arguments_delta: partial_json,
                            });
                        }
                    }
                }
            }
            AnthropicStreamEvent::ContentBlockStop { .. } => {
                // Emit accumulated thinking if any
                if !self.accumulated_thinking.is_empty() {
                    self.accumulated_thinking.clear();
                }

                // Emit tool call if we have one
                if let Some(tool) = self.current_tool_call.take() {
                    let arguments = serde_json::from_str(&tool.input_json)
                        .unwrap_or(Value::String(tool.input_json));

                    items.push(ModelStreamItem::ToolCall(ToolCallRequest {
                        id: tool.id,
                        name: tool.name,
                        arguments,
                    }));
                }
            }
            AnthropicStreamEvent::MessageDelta { delta, usage } => {
                // Capture stop_reason from message_delta
                if let Some(reason) = delta.stop_reason {
                    self.stop_reason = Some(reason);
                }
                // Extract token usage from the message delta
                // Note: message_start provides input_tokens and cache tokens;
                // message_delta provides output_tokens (and may repeat cache info)
                if let Some(usage) = usage {
                    self.output_tokens = Some(usage.output_tokens);
                    self.token_usage_emitted = true;
                    let input_tokens = self.input_tokens.unwrap_or(usage.input_tokens);
                    let cache_read = self
                        .cache_read_input_tokens
                        .unwrap_or(usage.cache_read_input_tokens);
                    // Total input = uncached input + cache read
                    let prompt_tokens = input_tokens + cache_read;
                    let cached_tokens = if cache_read > 0 {
                        Some(cache_read)
                    } else {
                        None
                    };
                    items.push(ModelStreamItem::TokenUsage(
                        crate::provider::TokenUsage::new(
                            prompt_tokens,
                            usage.output_tokens,
                            cached_tokens,
                        ),
                    ));
                } else if self.input_tokens.is_some() {
                    // message_delta without usage - fallback to stored values
                    let cache_read = self.cache_read_input_tokens.unwrap_or(0);
                    let prompt_tokens = self.input_tokens.unwrap_or(0) + cache_read;
                    let cached_tokens = if cache_read > 0 {
                        Some(cache_read)
                    } else {
                        None
                    };
                    items.push(ModelStreamItem::TokenUsage(
                        crate::provider::TokenUsage::new(
                            prompt_tokens,
                            self.output_tokens.unwrap_or(0),
                            cached_tokens,
                        ),
                    ));
                }
            }
            AnthropicStreamEvent::MessageStop => {
                // Emit response metadata before Complete (if available)
                if let Some(response_id) = self.response_id.take() {
                    let finish_reason = self
                        .stop_reason
                        .take()
                        .and_then(|s| FinishReason::from_provider_str(&s));
                    items.push(ModelStreamItem::ResponseMeta {
                        response_id,
                        finish_reason,
                    });
                }
                items.push(ModelStreamItem::Complete);
            }
            AnthropicStreamEvent::Error { error } => {
                return Err(ProviderError::Request(format!(
                    "Anthropic API error: {}",
                    error.message
                )));
            }
        }

        Ok(items)
    }

    fn finish(&mut self) -> Vec<ModelStreamItem> {
        let mut items = Vec::new();

        // Emit any pending tool call
        if let Some(tool) = self.current_tool_call.take() {
            let arguments =
                serde_json::from_str(&tool.input_json).unwrap_or(Value::String(tool.input_json));

            items.push(ModelStreamItem::ToolCall(ToolCallRequest {
                id: tool.id,
                name: tool.name,
                arguments,
            }));
        }

        // Emit token usage if never emitted (e.g. message_delta had no usage)
        if let Some(input_tokens) = self.input_tokens {
            if !self.token_usage_emitted {
                let cache_read = self.cache_read_input_tokens.unwrap_or(0);
                let prompt_tokens = input_tokens + cache_read;
                let cached_tokens = if cache_read > 0 {
                    Some(cache_read)
                } else {
                    None
                };
                items.push(ModelStreamItem::TokenUsage(
                    crate::provider::TokenUsage::new(
                        prompt_tokens,
                        self.output_tokens.unwrap_or(0),
                        cached_tokens,
                    ),
                ));
            }
        }

        // Emit response metadata if we have response_id
        if let Some(response_id) = self.response_id.take() {
            let finish_reason = self
                .stop_reason
                .take()
                .and_then(|s| FinishReason::from_provider_str(&s));
            items.push(ModelStreamItem::ResponseMeta {
                response_id,
                finish_reason,
            });
        }

        if !items.iter().any(|i| matches!(i, ModelStreamItem::Complete)) {
            items.push(ModelStreamItem::Complete);
        }

        items
    }
}

// Anthropic API types
#[derive(Debug, Serialize)]
struct AnthropicRequest {
    model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    messages: Vec<AnthropicMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<AnthropicTool>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<AnthropicThinking>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<AnthropicOutputConfig>,
}

#[derive(Debug, Serialize)]
struct AnthropicOutputConfig {
    effort: String,
}

#[derive(Debug, Serialize)]
struct AnthropicThinking {
    #[serde(rename = "type")]
    type_: String,
    budget_tokens: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicMessage {
    role: String,
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicContent {
    Text {
        text: String,
    },
    Image {
        source: AnthropicImageSource,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    ToolResult {
        #[serde(rename = "tool_use_id")]
        tool_use_id: String,
        content: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct AnthropicImageSource {
    #[serde(rename = "type")]
    type_: String,
    #[serde(rename = "media_type")]
    media_type: String,
    data: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnthropicTool {
    name: String,
    description: String,
    #[serde(rename = "input_schema")]
    input_schema: Value,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum AnthropicStreamEvent {
    MessageStart {
        #[serde(rename = "message")]
        message: AnthropicMessageStart,
    },
    ContentBlockStart {
        #[serde(rename = "index")]
        _index: usize,
        content_block: AnthropicContent,
    },
    ContentBlockDelta {
        #[serde(rename = "index")]
        _index: usize,
        delta: AnthropicDelta,
    },
    ContentBlockStop {
        #[serde(rename = "index")]
        _index: usize,
    },
    MessageDelta {
        delta: AnthropicMessageDelta,
        usage: Option<AnthropicUsage>,
    },
    MessageStop,
    Ping,
    Error {
        error: AnthropicError,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageStart {
    id: String,
    #[serde(rename = "type")]
    _type_: String,
    #[serde(rename = "role")]
    _role: String,
    #[serde(rename = "content")]
    _content: Vec<AnthropicContent>,
    #[serde(rename = "model")]
    _model: String,
    #[serde(rename = "stop_reason")]
    stop_reason: Option<String>,
    #[serde(rename = "stop_sequence")]
    _stop_sequence: Option<String>,
    #[serde(rename = "usage")]
    usage: AnthropicUsage,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::enum_variant_names)]
enum AnthropicDelta {
    TextDelta {
        text: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        #[serde(rename = "signature")]
        _signature: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
}

#[derive(Debug, Deserialize)]
struct AnthropicMessageDelta {
    stop_reason: Option<String>,
    #[serde(rename = "stop_sequence")]
    _stop_sequence: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    #[serde(default)]
    cache_read_input_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct AnthropicError {
    #[serde(rename = "type")]
    _type_: String,
    #[serde(rename = "message")]
    message: String,
}

#[cfg(test)]
#[path = "anthropic_test.rs"]
mod tests;
