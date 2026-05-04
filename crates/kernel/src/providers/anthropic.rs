//! Implementation of the `Provider` trait for Anthropic's API
use crate::event::ContentChunk;
use crate::providers::{
    HttpError, ModelConfig, ModelStream, ModelStreamItem, Provider, ProviderError, ToolCallRequest,
};
use crate::types::{ContentBlock, Message, Result, Role, ToolDefinition};
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
                    Role::System => return None, // System is handled separately
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
                    signature,
                } if !thinking.is_empty() => {
                    // Preserve thinking blocks for conversation continuity
                    content.push(AnthropicContent::Thinking {
                        thinking: thinking.clone(),
                        signature: signature.clone().unwrap_or_default(),
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
            format!("{}/v1/messages", config.endpoint.trim_end_matches('/'))
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

        // Build request body
        let mut request_body = AnthropicRequest {
            model: config.model_id.clone(),
            max_tokens: config.max_tokens.unwrap_or(4096),
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
        };

        // Enable thinking if configured
        if config.thinking.enabled {
            request_body.thinking = Some(AnthropicThinking {
                type_: "enabled".to_string(),
                budget_tokens: config.thinking.budget_tokens,
            });
        }

        let request = self
            .client
            .post(&url)
            .header("x-api-key", &config.api_key)
            .header("anthropic-version", "2023-06-01")
            .header("Content-Type", "application/json")
            .json(&request_body);

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
        }
    }

    fn process(&mut self, data: &str) -> std::result::Result<Vec<ModelStreamItem>, ProviderError> {
        let event: AnthropicStreamEvent = serde_json::from_str(data).map_err(|e| {
            ProviderError::Parse(format!("Failed to parse SSE chunk: {e} - data: {data}"))
        })?;

        let mut items = Vec::new();

        match event {
            AnthropicStreamEvent::MessageStart { message } => {
                // Store input tokens from message_start event
                self.input_tokens = Some(message.usage.input_tokens);
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
            AnthropicStreamEvent::MessageDelta { usage, .. } => {
                // Extract token usage from the message delta
                // Note: message_delta contains output_tokens, input_tokens should come from message_start
                if let Some(usage) = usage {
                    let prompt_tokens = self.input_tokens.unwrap_or(usage.input_tokens);
                    items.push(ModelStreamItem::TokenUsage(
                        crate::providers::TokenUsage::new(
                            prompt_tokens,
                            usage.output_tokens,
                            None, // Anthropic doesn't support prompt caching in this format
                        ),
                    ));
                }
            }
            AnthropicStreamEvent::MessageStop => {
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
    max_tokens: u32,
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
        #[serde(rename = "delta")]
        _delta: AnthropicMessageDelta,
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
    #[serde(rename = "id")]
    _id: String,
    #[serde(rename = "type")]
    _type_: String,
    #[serde(rename = "role")]
    _role: String,
    #[serde(rename = "content")]
    _content: Vec<AnthropicContent>,
    #[serde(rename = "model")]
    _model: String,
    #[serde(rename = "stop_reason")]
    _stop_reason: Option<String>,
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
    #[serde(rename = "stop_reason")]
    _stop_reason: Option<String>,
    #[serde(rename = "stop_sequence")]
    _stop_sequence: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AnthropicUsage {
    input_tokens: u32,
    output_tokens: u32,
}

#[derive(Debug, Deserialize)]
struct AnthropicError {
    #[serde(rename = "type")]
    _type_: String,
    #[serde(rename = "message")]
    message: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AudioData, ToolCall};
    use chrono::Utc;

    #[test]
    fn test_extract_system_message() {
        let messages: Vec<Arc<Message>> = vec![
            Arc::new(Message::system("You are a helpful assistant")),
            Arc::new(Message::user("Hello")),
        ];

        let system = AnthropicProvider::extract_system_message(&messages);
        assert_eq!(system, Some("You are a helpful assistant".to_string()));
    }

    #[test]
    fn test_convert_messages_filters_system() {
        let messages: Vec<Arc<Message>> = vec![
            Arc::new(Message::system("System prompt")),
            Arc::new(Message::user("Hello")),
            Arc::new(Message::assistant("Hi there")),
        ];

        let converted = AnthropicProvider::convert_messages(&messages);
        assert_eq!(converted.len(), 2);
        assert_eq!(converted[0].role, "user");
        assert_eq!(converted[1].role, "assistant");
    }

    #[test]
    fn test_stream_state_text_content() {
        let mut state = AnthropicStreamState::new();

        // Simulate content block start
        let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":"Hello"}}"#;
        let items = state.process(event).unwrap();
        assert!(items.is_empty());

        // Simulate delta
        let event = r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":" world"}}"#;
        let items = state.process(event).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            ModelStreamItem::Chunk(ContentChunk::Text(text)) => {
                assert_eq!(text, " world");
            }
            _ => panic!("Expected text chunk"),
        }
    }

    #[test]
    fn test_stream_state_thinking_content() {
        let mut state = AnthropicStreamState::new();

        let event = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Let me think..."}}"#;
        let items = state.process(event).unwrap();

        assert_eq!(items.len(), 1);
        match &items[0] {
            ModelStreamItem::Chunk(ContentChunk::Thinking { thinking, .. }) => {
                assert_eq!(thinking, "Let me think...");
            }
            _ => panic!("Expected thinking chunk"),
        }
    }

    #[test]
    fn test_stream_state_tool_use() {
        let mut state = AnthropicStreamState::new();

        // Tool use starts
        let event = r#"{"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"tool_123","name":"bash","input":{}}}"#;
        let items = state.process(event).unwrap();
        assert!(items.is_empty());

        // Input JSON delta - emits ToolCallDelta for UI feedback
        let event = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#;
        let items = state.process(event).unwrap();
        assert_eq!(items.len(), 1);
        assert!(
            matches!(&items[0], ModelStreamItem::ToolCallDelta { id, arguments_delta, .. } if id == "tool_123" && arguments_delta == "{\"cmd\":\"ls\"}")
        );

        // Content block stop - should emit tool call
        let event = r#"{"type":"content_block_stop","index":0}"#;
        let items = state.process(event).unwrap();

        assert_eq!(items.len(), 1);
        match &items[0] {
            ModelStreamItem::ToolCall(call) => {
                assert_eq!(call.id, "tool_123");
                assert_eq!(call.name, "bash");
                assert_eq!(call.arguments, serde_json::json!({"cmd":"ls"}));
            }
            _ => panic!("Expected tool call"),
        }
    }

    #[test]
    fn test_stream_state_message_stop() {
        let mut state = AnthropicStreamState::new();

        let event = r#"{"type":"message_stop"}"#;
        let items = state.process(event).unwrap();

        assert_eq!(items.len(), 1);
        assert!(matches!(items[0], ModelStreamItem::Complete));
    }

    #[test]
    fn test_convert_tools() {
        use std::sync::Arc;
        let tools: Vec<Arc<ToolDefinition>> = vec![Arc::new(ToolDefinition {
            name: "bash".to_string(),
            description: "Execute bash commands".to_string(),
            parameters: serde_json::json!({
                "type": "object",
                "properties": {
                    "cmd": {"type": "string"}
                }
            }),
        })];

        let converted = AnthropicProvider::convert_tools(&tools);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].name, "bash");
        assert_eq!(converted[0].description, "Execute bash commands");
    }

    #[test]
    fn test_convert_tool_result_message() {
        // Create a tool result message
        let messages: Vec<Arc<Message>> = vec![Arc::new(Message {
            role: Role::Tool,
            content: vec![ContentBlock::Text {
                text: "File contents here".to_string(),
            }],
            tool_calls: None,
            tool_call_id: Some("tool_123".to_string()),
            created_at: Utc::now(),
            token_usage: None,
        })];

        let converted = AnthropicProvider::convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "user");
        assert_eq!(converted[0].content.len(), 1);

        // Check that it's a ToolResult content block
        match &converted[0].content[0] {
            AnthropicContent::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "tool_123");
                assert_eq!(content, "File contents here");
            }
            _ => panic!(
                "Expected ToolResult content block, got {:?}",
                converted[0].content[0]
            ),
        }

        // Verify JSON serialization has correct field names
        let json = serde_json::to_string(&converted[0].content[0]).unwrap();
        assert!(
            json.contains("tool_use_id"),
            "JSON should contain 'tool_use_id' field, got: {json}"
        );
        assert!(
            json.contains("tool_123"),
            "JSON should contain the tool ID, got: {json}"
        );
        assert!(
            json.contains("\"type\":\"tool_result\""),
            "JSON should have correct type, got: {json}"
        );
    }

    #[test]
    fn test_convert_assistant_with_tool_calls() {
        // Create an assistant message with tool_calls
        let messages: Vec<Arc<Message>> = vec![Arc::new(Message {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: "I'll check that file for you.".to_string(),
            }],
            tool_calls: Some(vec![ToolCall {
                id: "tool_456".to_string(),
                name: "read".to_string(),
                arguments: serde_json::json!({"path": "/tmp/test.txt"}),
            }]),
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
        })];

        let converted = AnthropicProvider::convert_messages(&messages);
        assert_eq!(converted.len(), 1);
        assert_eq!(converted[0].role, "assistant");
        assert_eq!(converted[0].content.len(), 2); // Text + ToolUse

        // First block should be text
        match &converted[0].content[0] {
            AnthropicContent::Text { text } => {
                assert_eq!(text, "I'll check that file for you.");
            }
            _ => panic!("Expected Text content block"),
        }

        // Second block should be ToolUse
        match &converted[0].content[1] {
            AnthropicContent::ToolUse { id, name, input } => {
                assert_eq!(id, "tool_456");
                assert_eq!(name, "read");
                assert_eq!(input, &serde_json::json!({"path": "/tmp/test.txt"}));
            }
            _ => panic!("Expected ToolUse content block"),
        }
    }

    #[test]
    fn test_convert_redacted_thinking() {
        let blocks = vec![ContentBlock::RedactedThinking {
            data: "redacted_data_123".to_string(),
        }];

        let converted = AnthropicProvider::convert_content_blocks(&blocks);
        assert_eq!(converted.len(), 1);

        match &converted[0] {
            AnthropicContent::RedactedThinking { data } => {
                assert_eq!(data, "redacted_data_123");
            }
            _ => panic!("Expected RedactedThinking content block"),
        }
    }

    #[test]
    fn test_convert_thinking_preserved() {
        let blocks = vec![ContentBlock::Thinking {
            thinking: "Let me analyze this...".to_string(),
            signature: Some("sig_abc".to_string()),
        }];

        let converted = AnthropicProvider::convert_content_blocks(&blocks);
        assert_eq!(converted.len(), 1);

        match &converted[0] {
            AnthropicContent::Thinking {
                thinking,
                signature,
            } => {
                assert_eq!(thinking, "Let me analyze this...");
                assert_eq!(signature, "sig_abc");
            }
            _ => panic!("Expected Thinking content block, got {:?}", converted[0]),
        }
    }

    #[test]
    fn test_convert_audio_skipped() {
        let blocks = vec![
            ContentBlock::Text {
                text: "Hello".to_string(),
            },
            ContentBlock::Audio {
                audio: AudioData {
                    data: "base64audio".to_string(),
                    format: "mp3".to_string(),
                },
            },
        ];

        let converted = AnthropicProvider::convert_content_blocks(&blocks);
        assert_eq!(converted.len(), 1);

        match &converted[0] {
            AnthropicContent::Text { text } => assert_eq!(text, "Hello"),
            _ => panic!("Expected only Text content block, audio should be skipped"),
        }
    }

    #[test]
    fn test_multi_turn_tool_conversation() {
        // Simulate a full tool use conversation flow
        let messages: Vec<Arc<Message>> = vec![
            // User asks a question
            Arc::new(Message::user("What's the weather?")),
            // Assistant responds with a tool call
            Arc::new(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "I'll check the weather for you.".to_string(),
                }],
                tool_calls: Some(vec![ToolCall {
                    id: "weather_1".to_string(),
                    name: "get_weather".to_string(),
                    arguments: serde_json::json!({"location": "New York"}),
                }]),
                tool_call_id: None,
                created_at: Utc::now(),
                token_usage: None,
            }),
            // Tool result
            Arc::new(Message {
                role: Role::Tool,
                content: vec![ContentBlock::Text {
                    text: "72°F and sunny".to_string(),
                }],
                tool_calls: None,
                tool_call_id: Some("weather_1".to_string()),
                created_at: Utc::now(),
                token_usage: None,
            }),
            // Final assistant response
            Arc::new(Message::assistant("It's 72°F and sunny in New York!")),
        ];

        let converted = AnthropicProvider::convert_messages(&messages);
        assert_eq!(converted.len(), 4);

        // Check user message
        assert_eq!(converted[0].role, "user");
        assert!(matches!(
            converted[0].content[0],
            AnthropicContent::Text { .. }
        ));

        // Check assistant with tool_use
        assert_eq!(converted[1].role, "assistant");
        assert_eq!(converted[1].content.len(), 2);
        assert!(matches!(
            converted[1].content[0],
            AnthropicContent::Text { .. }
        ));
        assert!(matches!(
            converted[1].content[1],
            AnthropicContent::ToolUse { .. }
        ));

        // Check tool result
        assert_eq!(converted[2].role, "user");
        assert!(matches!(
            converted[2].content[0],
            AnthropicContent::ToolResult { .. }
        ));

        // Check final assistant response
        assert_eq!(converted[3].role, "assistant");
        assert!(matches!(
            converted[3].content[0],
            AnthropicContent::Text { .. }
        ));
    }

    #[test]
    fn test_full_request_serialization() {
        // Test the full request body serialization to catch any JSON structure issues
        use std::sync::Arc;

        let messages: Vec<Arc<Message>> = vec![
            Arc::new(Message::user("What's the weather?")),
            Arc::new(Message {
                role: Role::Assistant,
                content: vec![ContentBlock::Text {
                    text: "I'll check that for you.".to_string(),
                }],
                tool_calls: Some(vec![ToolCall {
                    id: "toolu_01D7FLrfh4GYq7yT1ULFeyMV".to_string(),
                    name: "get_weather".to_string(),
                    arguments: serde_json::json!({"location": "NYC"}),
                }]),
                tool_call_id: None,
                created_at: Utc::now(),
                token_usage: None,
            }),
            Arc::new(Message::tool_result(
                "toolu_01D7FLrfh4GYq7yT1ULFeyMV",
                "72°F and sunny",
            )),
        ];

        let anthropic_messages = AnthropicProvider::convert_messages(&messages);
        let request = AnthropicRequest {
            model: "claude-3-sonnet-20240229".to_string(),
            max_tokens: 4096,
            messages: anthropic_messages,
            system: None,
            tools: None,
            stream: true,
            temperature: None,
            thinking: None,
        };

        let json = serde_json::to_string_pretty(&request).unwrap();

        // Debug: print the JSON
        println!("Request JSON:\n{json}");

        // Verify the JSON structure (with spaces as serde_json pretty-prints)
        assert!(
            json.contains("\"type\": \"tool_use\""),
            "Should contain tool_use block"
        );
        assert!(
            json.contains("\"type\": \"tool_result\""),
            "Should contain tool_result block"
        );
        assert!(
            json.contains("\"tool_use_id\": \"toolu_01D7FLrfh4GYq7yT1ULFeyMV\""),
            "Should contain tool_use_id with correct value"
        );
        assert!(
            json.contains("\"id\": \"toolu_01D7FLrfh4GYq7yT1ULFeyMV\""),
            "Should contain tool_use id"
        );

        // Ensure no empty tool_use_id
        assert!(
            !json.contains("\"tool_use_id\": \"\""),
            "Should not contain empty tool_use_id"
        );
        assert!(
            !json.contains("\"tool_use_id\": null"),
            "Should not contain null tool_use_id"
        );
    }

    #[test]
    fn test_stream_state_token_usage() {
        let mut state = AnthropicStreamState::new();

        // Simulate message_start with input_tokens
        let event = r#"{"type":"message_start","message":{"id":"msg_123","type":"message","role":"assistant","content":[],"model":"claude-3","stop_reason":null,"stop_sequence":null,"usage":{"input_tokens":100,"output_tokens":1}}}"#;
        let items = state.process(event).unwrap();
        assert!(items.is_empty()); // No items emitted on message_start

        // Simulate message_delta with output_tokens
        let event = r#"{"type":"message_delta","delta":{"stop_reason":"end_turn","stop_sequence":null},"usage":{"input_tokens":0,"output_tokens":55}}"#;
        let items = state.process(event).unwrap();

        assert_eq!(items.len(), 1);
        match &items[0] {
            ModelStreamItem::TokenUsage(usage) => {
                // prompt_tokens should come from message_start (100), not message_delta (0)
                assert_eq!(
                    usage.prompt_tokens, 100,
                    "prompt_tokens should be from message_start"
                );
                assert_eq!(
                    usage.completion_tokens, 55,
                    "completion_tokens should be from message_delta"
                );
            }
            _ => panic!("Expected TokenUsage item, got {:?}", items[0]),
        }
    }
}
