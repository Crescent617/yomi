//! Implementation of the `Provider` trait for Anthropic's API
//! TODO: not implemented fully yet - need to handle thinking content, tool results, and other content types
use crate::event::ContentChunk;
use crate::providers::{
    HttpError, ModelConfig, ModelStream, ModelStreamItem, Provider, ToolCallRequest,
};
use crate::types::{ContentBlock, Message, Role, ToolDefinition};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use std::sync::Arc;
use eventsource_stream::Eventsource;
use futures::stream::{self, StreamExt, TryStreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;
use tokio::time::timeout;

const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub struct AnthropicProvider {
    client: Client,
    name: String,
}

impl AnthropicProvider {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(120))
                .build()?,
            name: "anthropic".to_string(),
        })
    }

    fn convert_messages(messages: &[Arc<Message>]) -> Vec<AnthropicMessage> {
        messages
            .iter()
            .filter_map(|m| {
                // Skip messages with empty content
                if m.content.is_empty() {
                    return None;
                }

                let role = match m.role {
                    Role::System => return None, // System is handled separately
                    Role::User | Role::Tool => "user",
                    Role::Assistant => "assistant",
                };

                let content = Self::convert_content_blocks(&m.content);

                Some(AnthropicMessage {
                    role: role.to_string(),
                    content,
                })
            })
            .collect()
    }

    fn convert_content_blocks(blocks: &[ContentBlock]) -> Vec<AnthropicContent> {
        let mut content = Vec::new();

        // Add content blocks
        for block in blocks {
            match block {
                ContentBlock::Text { text } if !text.is_empty() => {
                    content.push(AnthropicContent::Text { text: text.clone() });
                }
                ContentBlock::Thinking { thinking, .. } if !thinking.is_empty() => {
                    // Thinking blocks are not sent back to Anthropic
                }
                ContentBlock::ImageUrl { image_url } => {
                    content.push(AnthropicContent::Image {
                        source: AnthropicImageSource {
                            type_: "base64".to_string(),
                            media_type: "image/png".to_string(),
                            data: image_url.url.clone(),
                        },
                    });
                }
                _ => {}
            }
        }

        // Tool results are handled separately via Message::tool_call_id

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
    ) -> Result<ModelStream> {
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
        let messages = Self::convert_messages(messages);

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
            return Err(anyhow!(HttpError(status.as_u16())));
        }

        tracing::debug!("Anthropic API response received, starting stream processing");

        let eventsource = response.bytes_stream().eventsource();

        let stream = stream::try_unfold(
            (eventsource, AnthropicStreamState::new()),
            |(mut eventsource, mut state)| async move {
                loop {
                    match timeout(IDLE_TIMEOUT, eventsource.try_next()).await {
                        Ok(Ok(Some(event))) => {
                            if event.data == "[DONE]" {
                                let items = state.finish();
                                return Ok(Some((items, (eventsource, state))));
                            }

                            let items = state.process(&event.data)?;
                            if !items.is_empty() {
                                return Ok(Some((items, (eventsource, state))));
                            }
                        }
                        Ok(Ok(None)) => {
                            tracing::debug!("Anthropic stream ended normally");
                            let items = state.finish();
                            return Ok(Some((items, (eventsource, state))));
                        }
                        Ok(Err(e)) => {
                            tracing::error!("Anthropic SSE error: {}", e);
                            return Err(anyhow!("SSE error: {e}"));
                        }
                        Err(_) => {
                            tracing::error!(
                                "Anthropic SSE idle timeout after {}s",
                                IDLE_TIMEOUT.as_secs()
                            );
                            return Err(anyhow!(
                                "SSE idle timeout: no data received for {} seconds",
                                IDLE_TIMEOUT.as_secs()
                            ));
                        }
                    }
                }
            },
        )
        .flat_map(|result: Result<Vec<ModelStreamItem>>| {
            let items: Vec<Result<ModelStreamItem>> = match result {
                Ok(items) => items.into_iter().map(Ok).collect(),
                Err(e) => vec![Err(e)],
            };
            stream::iter(items)
        })
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
        }
    }

    fn process(&mut self, data: &str) -> Result<Vec<ModelStreamItem>> {
        let event: AnthropicStreamEvent = serde_json::from_str(data)
            .map_err(|e| anyhow!("Failed to parse SSE chunk: {e} - data: {data}"))?;

        let mut items = Vec::new();

        match event {
            AnthropicStreamEvent::MessageStart { .. } | AnthropicStreamEvent::Ping => {}
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
                            tool.input_json.push_str(&partial_json);
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
                if let Some(usage) = usage {
                    items.push(ModelStreamItem::TokenUsage {
                        prompt_tokens: usage.input_tokens,
                        completion_tokens: usage.output_tokens,
                    });
                }
            }
            AnthropicStreamEvent::MessageStop => {
                items.push(ModelStreamItem::Complete);
            }
            AnthropicStreamEvent::Error { error } => {
                return Err(anyhow!("Anthropic API error: {}", error.message));
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
        _message: AnthropicMessageStart,
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
    _usage: AnthropicUsage,
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

        // Input JSON delta
        let event = r#"{"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"cmd\":\"ls\"}"}}"#;
        let items = state.process(event).unwrap();
        assert!(items.is_empty());

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
}
