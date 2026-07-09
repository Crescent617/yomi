//! Implementation of the `Provider` trait for `OpenAI`'s Responses API (`/v1/responses`).
//!
//! The Responses API is `OpenAI`'s newer streaming API used by GPT-5 / o-series
//! reasoning models. Compared to Chat Completions it uses:
//! - `input` items instead of `messages` (tool calls are standalone items)
//! - semantic SSE events (`response.output_text.delta`, ...) instead of `choices[].delta`
//! - a top-level `instructions` field for the system prompt
//!
//! This provider is stateless: `store: false` is always sent and the kernel
//! provides the full conversation history on every request.
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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;

// 2-minute idle timeout to detect stalled connections
const IDLE_TIMEOUT: Duration = Duration::from_mins(2);

pub struct OpenAIResponseProvider {
    client: Arc<Client>,
    name: String,
}

impl OpenAIResponseProvider {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: crate::provider::http_client(),
            name: "openai-response".to_string(),
        })
    }

    /// Extract and concatenate all system messages into the top-level `instructions` field.
    fn extract_instructions(messages: &[Arc<Message>]) -> Option<String> {
        let parts: Vec<String> = messages
            .iter()
            .filter(|m| m.role == Role::System)
            .map(|m| m.text_content())
            .filter(|s| !s.is_empty())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n\n"))
        }
    }

    /// Convert kernel messages into Responses API input items.
    ///
    /// - System messages are excluded (they go to `instructions`)
    /// - Assistant tool calls become standalone `function_call` items
    /// - Tool results become `function_call_output` items
    /// - Thinking blocks are dropped (Phase 1: no reasoning replay)
    fn convert_messages(messages: &[Arc<Message>]) -> Vec<InputItem> {
        let mut items = Vec::new();

        for m in messages {
            let m = m.as_ref();
            match m.role {
                Role::System | Role::Internal => {}
                Role::Tool => {
                    // Tool result -> function_call_output item
                    if let Some(ref call_id) = m.tool_call_id {
                        if call_id.is_empty() {
                            tracing::warn!(
                                "Tool message with empty tool_call_id, skipping for Responses API"
                            );
                            continue;
                        }
                        items.push(InputItem::FunctionCallOutput {
                            call_id: call_id.clone(),
                            output: m.text_content(),
                        });
                    } else {
                        tracing::warn!(
                            "Tool message without tool_call_id, skipping for Responses API"
                        );
                    }
                }
                Role::User => {
                    let content = Self::convert_user_content(&m.content);
                    if !content.is_empty() {
                        items.push(InputItem::Message {
                            role: "user".into(),
                            content,
                        });
                    }
                }
                Role::Assistant => {
                    let text = m.text_content();
                    if !text.is_empty() {
                        items.push(InputItem::Message {
                            role: "assistant".into(),
                            content: vec![ContentPart::OutputText { text }],
                        });
                    }
                    // Each tool call becomes a standalone function_call item
                    if let Some(ref tool_calls) = m.tool_calls {
                        for call in tool_calls {
                            items.push(InputItem::FunctionCall {
                                call_id: call.id.clone(),
                                name: call.name.clone(),
                                arguments: call.arguments.to_string(),
                            });
                        }
                    }
                }
            }
        }

        items
    }

    fn convert_user_content(blocks: &[ContentBlock]) -> Vec<ContentPart> {
        blocks
            .iter()
            .filter_map(|c| match c {
                ContentBlock::Text { text } if !text.is_empty() => {
                    Some(ContentPart::InputText { text: text.clone() })
                }
                ContentBlock::ImageUrl { image_url } => Some(ContentPart::InputImage {
                    image_url: image_url.url.clone(),
                    detail: image_url.detail.clone(),
                }),
                _ => None,
            })
            .collect()
    }

    /// Convert tool definitions to the Responses API flat format
    /// (unlike Chat Completions, there is no nested `function` object).
    fn convert_tools(tools: &[Arc<ToolDefinition>]) -> Vec<ResponsesTool> {
        tools
            .iter()
            .map(|t| ResponsesTool {
                type_: "function".to_string(),
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            })
            .collect()
    }
}

#[async_trait]
impl Provider for OpenAIResponseProvider {
    async fn stream(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
    ) -> std::result::Result<ModelStream, ProviderError> {
        let url = if config.endpoint.is_empty() {
            "https://api.openai.com/v1/responses".to_string()
        } else {
            format!("{}/responses", config.endpoint.trim_end_matches('/'))
        };

        tracing::debug!(
            "OpenAI Responses API request: model={}, messages={}, tools={}, thinking={}",
            config.model_id,
            messages.len(),
            tools.len(),
            config.thinking.enabled
        );

        let reasoning = if config.thinking.enabled {
            Some(ReasoningParam {
                effort: config
                    .thinking
                    .effort
                    .clone()
                    .unwrap_or_else(|| "medium".to_string()),
                summary: "auto".to_string(),
            })
        } else {
            None
        };

        // Reasoning models reject the temperature parameter (400).
        // Drop it silently when thinking is enabled.
        let temperature = if config.thinking.enabled {
            if config.temperature.is_some() {
                tracing::debug!(
                    "Dropping temperature for reasoning model {} (thinking enabled)",
                    config.model_id
                );
            }
            None
        } else {
            config.temperature
        };

        let request_body = ResponsesRequest {
            model: config.model_id.clone(),
            input: Self::convert_messages(messages),
            instructions: Self::extract_instructions(messages),
            tools: if tools.is_empty() {
                None
            } else {
                Some(Self::convert_tools(tools))
            },
            stream: true,
            store: false,
            max_output_tokens: config.max_tokens.or(Some(8192)),
            temperature,
            reasoning,
        };

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
            let truncated = if text.len() > 200 {
                format!("{}... (truncated)", &text[..200])
            } else {
                text
            };
            tracing::error!("OpenAI Responses API error: {} - {}", status, truncated);
            return Err(ProviderError::Http(HttpError(status.as_u16())));
        }

        tracing::debug!("OpenAI Responses API response received, starting stream processing");

        let eventsource = response.bytes_stream().eventsource();

        let stream = stream::try_unfold(
            (
                eventsource,
                ResponseAssembler::new(),
                tokio::time::Instant::now(),
            ),
            |(mut eventsource, mut assembler, last_content_time)| async move {
                loop {
                    if assembler.done {
                        return Ok(None);
                    }
                    let elapsed = last_content_time.elapsed();
                    let Some(remaining) = IDLE_TIMEOUT.checked_sub(elapsed) else {
                        tracing::error!(
                            "OpenAI Responses SSE content stall: no content for {}s",
                            elapsed.as_secs()
                        );
                        return Err(ProviderError::Timeout(format!(
                            "Content stall: no meaningful data received for {} seconds",
                            elapsed.as_secs()
                        )));
                    };
                    match timeout(remaining, eventsource.try_next()).await {
                        Ok(Ok(Some(event))) => {
                            // Responses API has no "[DONE]" sentinel by spec, but some
                            // proxies add it; treat it as end-of-stream.
                            if event.data == "[DONE]" {
                                let items = assembler.finish();
                                return Ok(Some((
                                    items,
                                    (eventsource, assembler, last_content_time),
                                )));
                            }

                            let items = assembler.process(&event.data)?;
                            if !items.is_empty() {
                                return Ok(Some((
                                    items,
                                    (eventsource, assembler, tokio::time::Instant::now()),
                                )));
                            }
                            // No content produced, continue loop with same timer
                        }
                        Ok(Ok(None)) => {
                            tracing::debug!("OpenAI Responses stream ended");
                            let items = assembler.finish();
                            return Ok(Some((items, (eventsource, assembler, last_content_time))));
                        }
                        Ok(Err(e)) => {
                            tracing::error!("OpenAI Responses SSE error: {}", e);
                            return Err(ProviderError::Sse(format!("SSE error: {e}")));
                        }
                        Err(_) => {
                            tracing::error!(
                                "OpenAI Responses SSE idle timeout after {}s",
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

/// Assembles Responses API SSE events into `ModelStreamItem`s.
///
/// Unlike Chat Completions, the Responses API emits explicit lifecycle events
/// for each output item (`response.output_item.added` / `.done`), so tool call
/// completion does not need to be inferred heuristically.
struct ResponseAssembler {
    /// Partial function calls keyed by `output_index`
    partial_calls: HashMap<u64, PartialCall>,
    /// API response ID (e.g., "`resp_xxx`")
    response_id: Option<String>,
    /// True once any `function_call` item completed (used to synthesize `FinishReason::ToolCalls`)
    saw_function_call: bool,
    /// True once terminal event (`response.completed` / `.incomplete` / `.failed`) was seen
    done: bool,
    /// True once `finish()` emitted the final items (guards against double-finish)
    finished: bool,
    /// Finish reason determined by terminal event
    finish_reason: Option<FinishReason>,
    /// Usage captured from terminal event
    usage: Option<crate::provider::TokenUsage>,
}

/// Accumulated state for a single function call item
#[derive(Default)]
struct PartialCall {
    call_id: String,
    name: String,
    arguments: String,
}

impl ResponseAssembler {
    fn new() -> Self {
        Self {
            partial_calls: HashMap::new(),
            response_id: None,
            saw_function_call: false,
            done: false,
            finished: false,
            finish_reason: None,
            usage: None,
        }
    }

    /// Process one SSE event's data payload.
    fn process(&mut self, data: &str) -> std::result::Result<Vec<ModelStreamItem>, ProviderError> {
        let event: ResponsesStreamEvent = serde_json::from_str(data).map_err(|e| {
            ProviderError::Parse(format!("Failed to parse SSE event: {e} - data: {data}"))
        })?;

        let mut items = Vec::new();

        match event.type_.as_str() {
            "response.created" => {
                if let Some(resp) = event.response {
                    if let Some(id) = resp.id {
                        self.response_id = Some(id);
                    }
                }
            }
            "response.output_text.delta" => {
                if let Some(delta) = event.delta.filter(|d| !d.is_empty()) {
                    items.push(ModelStreamItem::Chunk(ContentChunk::Text(delta)));
                }
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                if let Some(delta) = event.delta.filter(|d| !d.is_empty()) {
                    items.push(ModelStreamItem::Chunk(ContentChunk::Thinking {
                        thinking: delta,
                        signature: None,
                    }));
                }
            }
            "response.output_item.added" => {
                if let Some(item) = event.item {
                    if item.type_.as_deref() == Some("function_call") {
                        let index = event.output_index.unwrap_or(0);
                        let call_id = item.call_id.unwrap_or_default();
                        let name = item.name.unwrap_or_default();
                        // Notify UI that a tool call started (empty delta)
                        if !call_id.is_empty() {
                            items.push(ModelStreamItem::ToolCallDelta {
                                id: call_id.clone(),
                                name: name.clone(),
                                arguments_delta: String::new(),
                            });
                        }
                        self.partial_calls.insert(
                            index,
                            PartialCall {
                                call_id,
                                name,
                                arguments: String::new(),
                            },
                        );
                    }
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(delta) = event.delta.filter(|d| !d.is_empty()) {
                    let index = event.output_index.unwrap_or(0);
                    let partial = self.partial_calls.entry(index).or_default();
                    partial.arguments.push_str(&delta);
                    if !partial.call_id.is_empty() {
                        items.push(ModelStreamItem::ToolCallDelta {
                            id: partial.call_id.clone(),
                            name: partial.name.clone(),
                            arguments_delta: delta,
                        });
                    }
                }
            }
            "response.output_item.done" => {
                if let Some(item) = event.item {
                    if item.type_.as_deref() == Some("function_call") {
                        let index = event.output_index.unwrap_or(0);
                        if let Some(call) = self.complete_call(index, item) {
                            items.push(ModelStreamItem::ToolCall(call));
                            self.saw_function_call = true;
                        }
                    }
                }
            }
            "response.completed" | "response.incomplete" => {
                self.done = true;
                if let Some(resp) = event.response {
                    if let Some(id) = resp.id {
                        self.response_id = Some(id);
                    }
                    if let Some(usage) = resp.usage {
                        self.usage = Some(crate::provider::TokenUsage::new(
                            usage.input_tokens,
                            usage.output_tokens,
                            usage.cached_tokens(),
                        ));
                    }
                    self.finish_reason = Some(self.normalize_finish_reason(
                        resp.status.as_deref(),
                        resp.incomplete_details.as_ref(),
                    ));
                }
                items.extend(self.finish());
            }
            "response.failed" | "error" => {
                let message = event
                    .response
                    .and_then(|r| r.error)
                    .map(|e| e.message)
                    .or(event.message)
                    .unwrap_or_else(|| "unknown error".to_string());
                tracing::error!("OpenAI Responses API stream error: {}", message);
                return Err(ProviderError::Sse(format!("Response failed: {message}")));
            }
            // Known-but-ignored lifecycle events: response.in_progress,
            // response.output_text.done, response.content_part.*,
            // response.reasoning_summary_part.*, response.function_call_arguments.done, ...
            _ => {}
        }

        Ok(items)
    }

    /// Complete a function call from its `output_item.done` event.
    /// Prefers the final item payload (authoritative), falling back to accumulated deltas.
    fn complete_call(&mut self, index: u64, item: OutputItem) -> Option<ToolCallRequest> {
        let partial = self.partial_calls.remove(&index).unwrap_or_default();

        let call_id = item
            .call_id
            .filter(|s| !s.is_empty())
            .unwrap_or(partial.call_id);
        let name = item.name.filter(|s| !s.is_empty()).unwrap_or(partial.name);
        let arguments_str = item
            .arguments
            .filter(|s| !s.is_empty())
            .unwrap_or(partial.arguments);

        if call_id.is_empty() || name.is_empty() {
            tracing::warn!(
                "Incomplete function_call item (call_id or name missing), dropping: index={index}"
            );
            return None;
        }

        // Parse arguments as JSON, fall back to raw string
        let arguments =
            serde_json::from_str(&arguments_str).unwrap_or(Value::String(arguments_str));

        Some(ToolCallRequest {
            id: call_id,
            name,
            arguments,
        })
    }

    fn normalize_finish_reason(
        &self,
        status: Option<&str>,
        incomplete: Option<&IncompleteDetails>,
    ) -> FinishReason {
        match status {
            Some("completed") => {
                if self.saw_function_call {
                    // Responses API has no "tool_calls" finish reason; synthesize it
                    // so the agent loop continues to tool execution.
                    FinishReason::ToolCalls
                } else {
                    FinishReason::Stop
                }
            }
            Some("incomplete") => match incomplete.and_then(|d| d.reason.as_deref()) {
                Some("max_output_tokens") => FinishReason::MaxTokens,
                Some("content_filter") => FinishReason::ContentFilter,
                other => {
                    tracing::warn!("Unknown incomplete reason: {other:?}");
                    FinishReason::Unknown
                }
            },
            other => {
                tracing::warn!("Unknown response status: {other:?}");
                FinishReason::Unknown
            }
        }
    }

    /// Emit final items: any leftover tool calls, usage, response meta, and Complete.
    fn finish(&mut self) -> Vec<ModelStreamItem> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        // Ensure the unfold loop terminates on the next iteration
        self.done = true;

        let mut items = Vec::new();

        // Flush any tool calls that never got an output_item.done (stream cut short)
        let mut indices: Vec<_> = self.partial_calls.keys().copied().collect();
        indices.sort_unstable();
        for idx in indices {
            if let Some(partial) = self.partial_calls.remove(&idx) {
                if partial.call_id.is_empty() || partial.name.is_empty() {
                    continue;
                }
                let arguments = serde_json::from_str(&partial.arguments)
                    .unwrap_or(Value::String(partial.arguments));
                items.push(ModelStreamItem::ToolCall(ToolCallRequest {
                    id: partial.call_id,
                    name: partial.name,
                    arguments,
                }));
                self.saw_function_call = true;
            }
        }

        if let Some(usage) = self.usage.take() {
            items.push(ModelStreamItem::TokenUsage(usage));
        }

        if let Some(response_id) = self.response_id.take() {
            items.push(ModelStreamItem::ResponseMeta {
                response_id,
                finish_reason: self.finish_reason.take(),
            });
        }

        items.push(ModelStreamItem::Complete);
        items
    }
}

// ==== Request types ====

#[derive(Debug, Serialize)]
struct ResponsesRequest {
    model: String,
    input: Vec<InputItem>,
    #[serde(skip_serializing_if = "Option::is_none")]
    instructions: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<ResponsesTool>>,
    stream: bool,
    store: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reasoning: Option<ReasoningParam>,
}

#[derive(Debug, Serialize)]
struct ReasoningParam {
    effort: String,
    summary: String,
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum InputItem {
    Message {
        role: String,
        content: Vec<ContentPart>,
    },
    FunctionCall {
        call_id: String,
        name: String,
        /// JSON-encoded arguments string
        arguments: String,
    },
    FunctionCallOutput {
        call_id: String,
        output: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ContentPart {
    InputText {
        text: String,
    },
    OutputText {
        text: String,
    },
    InputImage {
        image_url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
}

/// Flat tool definition (no nested `function` object, unlike Chat Completions)
#[derive(Debug, Serialize)]
struct ResponsesTool {
    #[serde(rename = "type")]
    type_: String,
    name: String,
    description: String,
    parameters: Value,
}

// ==== SSE event types ====

#[derive(Debug, Deserialize)]
struct ResponsesStreamEvent {
    #[serde(rename = "type")]
    type_: String,
    /// Text delta for `*.delta` events
    delta: Option<String>,
    /// Output item for `output_item.added` / `output_item.done`
    item: Option<OutputItem>,
    /// Index of the output item this event belongs to
    output_index: Option<u64>,
    /// Full response snapshot for lifecycle events (created/completed/...)
    response: Option<ResponseObject>,
    /// Error message for top-level `error` events
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OutputItem {
    #[serde(rename = "type")]
    type_: Option<String>,
    call_id: Option<String>,
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseObject {
    id: Option<String>,
    status: Option<String>,
    usage: Option<ResponsesUsage>,
    incomplete_details: Option<IncompleteDetails>,
    error: Option<ResponseError>,
}

#[derive(Debug, Deserialize)]
struct IncompleteDetails {
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ResponseError {
    message: String,
}

#[derive(Debug, Deserialize)]
struct ResponsesUsage {
    #[serde(default)]
    input_tokens: u32,
    #[serde(default)]
    output_tokens: u32,
    input_tokens_details: Option<InputTokensDetails>,
}

impl ResponsesUsage {
    fn cached_tokens(&self) -> Option<u32> {
        self.input_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens)
    }
}

#[derive(Debug, Deserialize)]
struct InputTokensDetails {
    cached_tokens: Option<u32>,
}

#[cfg(test)]
#[path = "openai_response_test.rs"]
mod tests;
