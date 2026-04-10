use crate::event::ContentChunk;
use crate::provider::{HttpError, ModelConfig, ModelProvider, ModelStream, ModelStreamItem, ToolCallRequest};
use crate::types::{Message, Role, ToolDefinition};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::{self, StreamExt, TryStreamExt};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::timeout;

// Stream transformer: SSE -> ModelStreamItem
// Accumulates tool calls, emits content immediately
// 2-minute idle timeout to detect stalled connections
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

pub struct OpenAIProvider {
    client: Client,
    name: String,
}

impl OpenAIProvider {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(std::time::Duration::from_secs(120))
                .build()?,
            name: "openai".to_string(),
        })
    }

    fn convert_messages(messages: &[Message]) -> Vec<OpenAIMessage> {
        messages
            .iter()
            .map(|m| {
                // Extract text content
                let content = if m.content.len() == 1 {
                    m.content
                        .first()
                        .and_then(|c| c.as_text())
                        .map(|t| t.to_string())
                        .unwrap_or_default()
                } else {
                    m.content
                        .iter()
                        .filter_map(|c| c.as_text())
                        .collect::<Vec<_>>()
                        .join("")
                };

                // Extract thinking content for reasoning models
                let reasoning_content = m.content.iter().find_map(|c| match c {
                    crate::types::ContentBlock::Thinking { thinking, .. } => Some(thinking.clone()),
                    _ => None,
                });

                OpenAIMessage {
                    role: match m.role {
                        Role::System => "system".to_string(),
                        Role::User => "user".to_string(),
                        Role::Assistant => "assistant".to_string(),
                        Role::Tool => "tool".to_string(),
                    },
                    content,
                    reasoning_content,
                    tool_calls: m.tool_calls.as_ref().map(|calls| {
                        calls
                            .iter()
                            .map(|c| OpenAIToolCall {
                                index: None,
                                id: Some(c.id.clone()),
                                type_: Some("function".to_string()),
                                function: OpenAIFunction {
                                    name: Some(c.name.clone()),
                                    arguments: Some(c.arguments.to_string()),
                                },
                            })
                            .collect()
                    }),
                    tool_call_id: m.tool_call_id.clone(),
                }
            })
            .collect()
    }

    fn convert_tools(tools: &[ToolDefinition]) -> Vec<OpenAITool> {
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
impl ModelProvider for OpenAIProvider {
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> Result<ModelStream> {
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

        let request_body = OpenAIRequest {
            model: config.model_id.clone(),
            messages: Self::convert_messages(messages),
            tools: if tools.is_empty() {
                None
            } else {
                Some(Self::convert_tools(tools))
            },
            stream: true,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        };
        let request = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body);
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::error!("OpenAI API error: {} - {}", status, text);
            return Err(anyhow!(HttpError(status.as_u16())));
        }

        tracing::debug!("OpenAI API response received, starting stream processing");

        let eventsource = response.bytes_stream().eventsource();

        let stream = stream::try_unfold(
            (eventsource, ToolCallAssembler::new()),
            |(mut eventsource, mut assembler)| async move {
                loop {
                    match timeout(IDLE_TIMEOUT, eventsource.try_next()).await {
                        Ok(Ok(Some(event))) => {
                            if event.data == "[DONE]" {
                                let items = assembler.finish();
                                return Ok(Some((items, (eventsource, assembler))));
                            }

                            let items = assembler.process(&event.data)?;
                            if !items.is_empty() {
                                return Ok(Some((items, (eventsource, assembler))));
                            }
                        }
                        Ok(Ok(None)) => {
                            // Stream ended normally
                            tracing::debug!("OpenAI stream ended normally");
                            let items = assembler.finish();
                            return Ok(Some((items, (eventsource, assembler))));
                        }
                        Ok(Err(e)) => {
                            tracing::error!("OpenAI SSE error: {}", e);
                            return Err(anyhow!("SSE error: {e}"));
                        }
                        Err(_) => {
                            tracing::error!(
                                "OpenAI SSE idle timeout after {}s",
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

/// Assembles partial tool calls from SSE chunks into complete `ToolCallRequest` objects.
///
/// `OpenAI`'s streaming format sends tool calls incrementally:
/// - Each chunk may contain deltas for one or more tool calls (identified by index)
/// - Arguments arrive as partial JSON strings across multiple chunks
/// - A tool call is complete when we receive a chunk with a higher index, or at stream end
///
/// This struct tracks partial state and determines when calls are ready to emit.
struct ToolCallAssembler {
    /// Partial tool calls by index
    partials: HashMap<usize, PartialToolCall>,
    /// The highest index we've seen so far. Used to detect when lower indices are complete.
    max_seen_index: Option<usize>,
}

/// Accumulated state for a single tool call
#[derive(Default)]
struct PartialToolCall {
    id: Option<String>,
    name: Option<String>,
    arguments: String,
}

impl ToolCallAssembler {
    fn new() -> Self {
        Self {
            partials: HashMap::new(),
            max_seen_index: None,
        }
    }

    /// Process an SSE chunk, returning any items that can be emitted immediately.
    ///
    /// Content (text/thinking) is emitted immediately as it arrives.
    /// Tool calls are accumulated; completed calls are emitted when we detect they're finished.
    fn process(&mut self, data: &str) -> Result<Vec<ModelStreamItem>> {
        let response: OpenAIStreamResponse = serde_json::from_str(data)
            .map_err(|e| anyhow!("Failed to parse SSE chunk: {e} - data: {data}"))?;

        let Some(choice) = response.choices.into_iter().next() else {
            return Ok(vec![]);
        };
        let Some(delta) = choice.delta else {
            return Ok(vec![]);
        };

        let mut items = Vec::new();

        // Handle tool call deltas
        if let Some(calls) = delta.tool_calls {
            for call in calls {
                let index = call.index.unwrap_or(0);

                // Update max seen index
                self.max_seen_index = Some(self.max_seen_index.map_or(index, |m| m.max(index)));

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
                if let Some(args) = call.function.arguments {
                    partial.arguments.push_str(&args);
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

        Ok(items)
    }

    /// Called when the stream ends. Returns all remaining complete tool calls and a Complete marker.
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

        items.push(ModelStreamItem::Complete);
        items
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
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    content: String,
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
    choices: Vec<OpenAIChoice>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIChoice {
    delta: Option<OpenAIDelta>,
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
mod tests {
    use super::*;

    fn create_test_response(delta: OpenAIDelta) -> OpenAIStreamResponse {
        OpenAIStreamResponse {
            choices: vec![OpenAIChoice { delta: Some(delta) }],
        }
    }

    fn create_tool_call_delta(
        index: usize,
        id: Option<&str>,
        name: Option<&str>,
        args: Option<&str>,
    ) -> OpenAIDelta {
        OpenAIDelta {
            content: None,
            thinking: None,
            reasoning: None,
            reasoning_content: None,
            thinking_signature: None,
            thinking_redacted: None,
            tool_calls: Some(vec![OpenAIToolCall {
                index: Some(index),
                id: id.map(|s| s.to_string()),
                type_: Some("function".to_string()),
                function: OpenAIFunction {
                    name: name.map(|s| s.to_string()),
                    arguments: args.map(|s| s.to_string()),
                },
            }]),
        }
    }

    #[test]
    fn test_assembler_single_tool_call() {
        let mut assembler = ToolCallAssembler::new();

        // First chunk: tool call starts
        let delta = create_tool_call_delta(0, Some("call_123"), Some("bash"), Some("{\"cmd\":\""));
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();

        // Should not emit anything yet (tool call not complete)
        assert!(items.is_empty());

        // Second chunk: arguments continue
        let delta = create_tool_call_delta(0, None, None, Some("ls"));
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();

        assert!(items.is_empty());

        // Third chunk: arguments complete
        let delta = create_tool_call_delta(0, None, None, Some("\"}"));
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();

        assert!(items.is_empty());

        // Finish should emit the completed tool call
        let items = assembler.finish();
        assert_eq!(items.len(), 2); // ToolCall + Complete

        match &items[0] {
            ModelStreamItem::ToolCall(call) => {
                assert_eq!(call.id, "call_123");
                assert_eq!(call.name, "bash");
                assert_eq!(call.arguments, serde_json::json!({"cmd":"ls"}));
            }
            _ => panic!("Expected ToolCall, got {:?}", items[0]),
        }
        assert!(matches!(items[1], ModelStreamItem::Complete));
    }

    #[test]
    fn test_assembler_multiple_tool_calls() {
        let mut assembler = ToolCallAssembler::new();

        // First tool call starts
        let delta = create_tool_call_delta(
            0,
            Some("call_1"),
            Some("read"),
            Some("{\"path\":\"file.txt\"}"),
        );
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();
        assert!(items.is_empty());

        // Second tool call starts - this should complete the first one
        let delta = create_tool_call_delta(
            1,
            Some("call_2"),
            Some("write"),
            Some("{\"path\":\"out.txt\"}"),
        );
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();

        // Should emit first tool call immediately when second starts
        assert_eq!(items.len(), 1);
        match &items[0] {
            ModelStreamItem::ToolCall(call) => {
                assert_eq!(call.id, "call_1");
                assert_eq!(call.name, "read");
            }
            _ => panic!("Expected ToolCall"),
        }

        // Finish should emit second tool call
        let items = assembler.finish();
        assert_eq!(items.len(), 2); // ToolCall + Complete
        match &items[0] {
            ModelStreamItem::ToolCall(call) => {
                assert_eq!(call.id, "call_2");
                assert_eq!(call.name, "write");
            }
            _ => panic!("Expected ToolCall"),
        }
    }

    #[test]
    fn test_assembler_text_content() {
        let mut assembler = ToolCallAssembler::new();

        let delta = OpenAIDelta {
            content: Some("Hello".to_string()),
            thinking: None,
            reasoning: None,
            reasoning_content: None,
            thinking_signature: None,
            thinking_redacted: None,
            tool_calls: None,
        };
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();

        assert_eq!(items.len(), 1);
        match &items[0] {
            ModelStreamItem::Chunk(ContentChunk::Text(text)) => {
                assert_eq!(text, "Hello");
            }
            _ => panic!("Expected Text chunk"),
        }
    }

    #[test]
    fn test_assembler_thinking_content() {
        let mut assembler = ToolCallAssembler::new();

        let delta = OpenAIDelta {
            content: None,
            thinking: Some("Let me think...".to_string()),
            reasoning: None,
            reasoning_content: None,
            thinking_signature: Some("sig123".to_string()),
            thinking_redacted: None,
            tool_calls: None,
        };
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();

        assert_eq!(items.len(), 1);
        match &items[0] {
            ModelStreamItem::Chunk(ContentChunk::Thinking {
                thinking,
                signature,
            }) => {
                assert_eq!(thinking, "Let me think...");
                assert_eq!(signature.as_deref(), Some("sig123"));
            }
            _ => panic!("Expected Thinking chunk, got {:?}", items[0]),
        }
    }

    #[test]
    fn test_assembler_reasoning_content_fallback() {
        let mut assembler = ToolCallAssembler::new();

        // Test reasoning field (used by some providers)
        let delta = OpenAIDelta {
            content: None,
            thinking: None,
            reasoning: Some("Reasoning step".to_string()),
            reasoning_content: None,
            thinking_signature: None,
            thinking_redacted: None,
            tool_calls: None,
        };
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();

        assert_eq!(items.len(), 1);
        match &items[0] {
            ModelStreamItem::Chunk(ContentChunk::Thinking { thinking, .. }) => {
                assert_eq!(thinking, "Reasoning step");
            }
            _ => panic!("Expected Thinking chunk"),
        }
    }

    #[test]
    fn test_assembler_redacted_thinking() {
        let mut assembler = ToolCallAssembler::new();

        let delta = OpenAIDelta {
            content: None,
            thinking: None,
            reasoning: None,
            reasoning_content: None,
            thinking_signature: None,
            thinking_redacted: Some(true),
            tool_calls: None,
        };
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();

        assert_eq!(items.len(), 1);
        assert!(matches!(
            items[0],
            ModelStreamItem::Chunk(ContentChunk::RedactedThinking)
        ));
    }

    #[test]
    fn test_assembler_empty_content_filtered() {
        let mut assembler = ToolCallAssembler::new();

        // Empty content should be filtered out
        let delta = OpenAIDelta {
            content: Some(String::new()),
            thinking: None,
            reasoning: None,
            reasoning_content: None,
            thinking_signature: None,
            thinking_redacted: None,
            tool_calls: None,
        };
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();

        assert!(items.is_empty());
    }

    #[test]
    fn test_assembler_no_choices() {
        let mut assembler = ToolCallAssembler::new();

        let response = OpenAIStreamResponse { choices: vec![] };
        let json = serde_json::to_string(&response).unwrap();
        let items = assembler.process(&json).unwrap();

        assert!(items.is_empty());
    }

    #[test]
    fn test_assembler_no_delta() {
        let mut assembler = ToolCallAssembler::new();

        let response = OpenAIStreamResponse {
            choices: vec![OpenAIChoice { delta: None }],
        };
        let json = serde_json::to_string(&response).unwrap();
        let items = assembler.process(&json).unwrap();

        assert!(items.is_empty());
    }

    #[test]
    fn test_assembler_invalid_json() {
        let mut assembler = ToolCallAssembler::new();

        let result = assembler.process("invalid json");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Failed to parse SSE chunk"));
    }

    #[test]
    fn test_assembler_incomplete_tool_call_finish() {
        let mut assembler = ToolCallAssembler::new();

        // Start a tool call but never complete it
        let delta = create_tool_call_delta(0, Some("call_1"), None, None); // missing name
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let _ = assembler.process(&json).unwrap();

        // Finish should not emit incomplete tool call (no name)
        let items = assembler.finish();
        assert_eq!(items.len(), 1); // Just Complete, no ToolCall
        assert!(matches!(items[0], ModelStreamItem::Complete));
    }

    #[test]
    fn test_assembler_mixed_content_and_tool() {
        let mut assembler = ToolCallAssembler::new();

        // First some text
        let delta = OpenAIDelta {
            content: Some("I'll help ".to_string()),
            thinking: None,
            reasoning: None,
            reasoning_content: None,
            thinking_signature: None,
            thinking_redacted: None,
            tool_calls: None,
        };
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();
        assert_eq!(items.len(), 1);

        // Then tool call
        let delta = create_tool_call_delta(0, Some("call_1"), Some("bash"), Some("{}"));
        let json = serde_json::to_string(&create_test_response(delta)).unwrap();
        let items = assembler.process(&json).unwrap();
        assert!(items.is_empty());

        // Finish
        let items = assembler.finish();
        assert_eq!(items.len(), 2);
        assert!(matches!(items[0], ModelStreamItem::ToolCall(_)));
        assert!(matches!(items[1], ModelStreamItem::Complete));
    }
}
