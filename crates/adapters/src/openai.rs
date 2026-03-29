use anyhow::{anyhow, Result};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::StreamExt;
use nekoclaw_core::event::ContentChunk;
use nekoclaw_core::provider::{ModelConfig, ModelProvider, ModelStream, ModelStreamItem, ToolCallRequest};
use nekoclaw_shared::types::{Message, Role, ToolDefinition};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub struct OpenAIProvider {
    client: Client,
    name: String,
}

impl OpenAIProvider {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()?,
            name: "openai".to_string(),
        })
    }

    fn convert_messages(&self, messages: &[Message]) -> Vec<OpenAIMessage> {
        messages.iter().map(|m| {
            let content = if m.content.len() == 1 {
                m.content.first().and_then(|c| c.as_text()).map(|t| t.to_string())
                    .unwrap_or_default()
            } else {
                m.content.iter().filter_map(|c| c.as_text()).collect::<Vec<_>>().join("")
            };
            OpenAIMessage {
                role: match m.role {
                    Role::System => "system".to_string(),
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                    Role::Tool => "tool".to_string(),
                },
                content,
                tool_calls: m.tool_calls.as_ref().map(|calls| {
                    calls.iter().map(|c| OpenAIToolCall {
                        id: c.id.clone(),
                        type_: "function".to_string(),
                        function: OpenAIFunction {
                            name: c.name.clone(),
                            arguments: c.arguments.to_string(),
                        },
                    }).collect()
                }),
                tool_call_id: m.tool_call_id.clone(),
            }
        }).collect()
    }

    fn convert_tools(&self, tools: &[ToolDefinition]) -> Vec<OpenAITool> {
        tools.iter().map(|t| OpenAITool {
            type_: "function".to_string(),
            function: OpenAIFunctionDef {
                name: t.name.clone(),
                description: t.description.clone(),
                parameters: t.parameters.clone(),
            },
        }).collect()
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
        let request_body = OpenAIRequest {
            model: config.model_id.clone(),
            messages: self.convert_messages(messages),
            tools: if tools.is_empty() { None } else { Some(self.convert_tools(tools)) },
            stream: true,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        };
        let request = self.client.post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body);
        let response = request.send().await?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI API error: {} - {}", status, text));
        }
        let stream = response.bytes_stream()
            .eventsource()
            .filter_map(move |event| {
                let result = match event {
                    Ok(event) => {
                        if event.data == "[DONE]" {
                            Some(Ok(ModelStreamItem::Complete))
                        } else {
                            match parse_sse_chunk(&event.data) {
                                Ok(Some(item)) => Some(Ok(item)),
                                Ok(None) => None,
                                Err(e) => Some(Err(e)),
                            }
                        }
                    }
                    Err(e) => Some(Err(anyhow!("SSE error: {}", e))),
                };
                async move { result }
            })
            .boxed();
        Ok(stream)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn parse_sse_chunk(data: &str) -> Result<Option<ModelStreamItem>> {
    let response: OpenAIStreamResponse = serde_json::from_str(data)
        .map_err(|e| anyhow!("Failed to parse SSE chunk: {} - data: {}", e, data))?;
    if let Some(choice) = response.choices.first() {
        if let Some(delta) = &choice.delta {
            if let Some(thinking) = &delta.thinking {
                return Ok(Some(ModelStreamItem::Chunk(ContentChunk::Thinking {
                    thinking: thinking.clone(),
                    signature: delta.thinking_signature.clone(),
                })));
            }
            if delta.thinking_redacted.unwrap_or(false) {
                return Ok(Some(ModelStreamItem::Chunk(ContentChunk::RedactedThinking)));
            }
            if let Some(content) = &delta.content {
                if !content.is_empty() {
                    return Ok(Some(ModelStreamItem::Chunk(ContentChunk::Text(content.clone()))));
                }
            }
            if let Some(tool_calls) = &delta.tool_calls {
                if let Some(call) = tool_calls.first() {
                    let id = call.id.clone();
                    let name = call.function.name.clone();
                    let args = call.function.arguments.clone();
                    if !id.is_empty() && !name.is_empty() {
                        let args_json: Value = serde_json::from_str(&args)
                            .unwrap_or_else(|_| Value::String(args.to_string()));
                        return Ok(Some(ModelStreamItem::ToolCall(ToolCallRequest {
                            id,
                            name,
                            arguments: args_json,
                        })));
                    }
                }
            }
        }
    }
    Ok(None)
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
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAITool {
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
    id: String,
    #[serde(rename = "type")]
    type_: String,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamResponse {
    choices: Vec<OpenAIChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    delta: Option<OpenAIDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAIDelta {
    content: Option<String>,
    thinking: Option<String>,
    #[serde(rename = "thinking_signature")]
    thinking_signature: Option<String>,
    #[serde(rename = "thinking_redacted")]
    thinking_redacted: Option<bool>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}
