use crate::event::ContentChunk;
use crate::types::{FinishReason, Message, Role, ToolDefinition};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::pin::Pin;
use thiserror::Error;

pub mod anthropic;
pub mod openai;
pub mod openai_response;

pub use anthropic::AnthropicProvider;
pub use openai::OpenAIProvider;
pub use openai_response::OpenAIResponseProvider;

/// Default output budget when the model config does not specify one.
pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8_192;
/// Headroom for provider formatting and local token-estimation error.
pub const CONTEXT_SAFETY_BUFFER_TOKENS: u32 = 4_096;

fn estimate_text_tokens(text: &str) -> u32 {
    crate::utils::tokens::estimate_tokens(text) as u32
}

fn estimate_json_tokens(value: &serde_json::Value) -> u32 {
    crate::utils::tokens::estimate_tokens_for_json(&value.to_string()) as u32
}

fn estimate_message_tokens(message: &Message) -> u32 {
    let content_tokens = message.content.iter().fold(0u32, |total, block| {
        let tokens = match block {
            crate::types::ContentBlock::Text { text } => estimate_text_tokens(text),
            crate::types::ContentBlock::Thinking {
                thinking,
                signature,
            } => estimate_text_tokens(thinking)
                .saturating_add(signature.as_deref().map_or(0, estimate_text_tokens)),
            crate::types::ContentBlock::RedactedThinking { data } => estimate_text_tokens(data),
            crate::types::ContentBlock::ImageUrl { image_url } => {
                // Provider image tokenization depends on decoded dimensions. Use a
                // conservative per-image floor and charge inline data by encoded
                // size so large payloads cannot hide behind a fixed estimate.
                4_096u32.max(estimate_text_tokens(&image_url.url))
            }
            // No current provider serializes Audio blocks; do not budget content
            // that is omitted from the actual request.
            crate::types::ContentBlock::Audio { .. } => 0,
        };
        total.saturating_add(tokens)
    });
    let tool_call_tokens = message
        .tool_calls
        .as_deref()
        .unwrap_or_default()
        .iter()
        .fold(0u32, |total, call| {
            total
                .saturating_add(estimate_text_tokens(&call.id))
                .saturating_add(estimate_text_tokens(&call.name))
                .saturating_add(estimate_json_tokens(&call.arguments))
                .saturating_add(8)
        });

    content_tokens
        .saturating_add(tool_call_tokens)
        .saturating_add(
            message
                .tool_call_id
                .as_deref()
                .map_or(0, estimate_text_tokens),
        )
        .saturating_add(10)
}

/// Estimate the input tokens for a request using only messages providers serialize.
pub fn estimate_request_input_tokens(
    messages: &[Arc<Message>],
    tools: &[Arc<ToolDefinition>],
    _config: &ModelConfig,
) -> u32 {
    let last_assistant_usage = messages
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, message)| {
            (message.role == Role::Assistant)
                .then(|| message.token_usage.as_ref().map(|usage| (index, usage)))
                .flatten()
        });

    if let Some((index, usage)) = last_assistant_usage {
        return messages[index + 1..]
            .iter()
            .filter(|message| message.role != Role::Internal)
            .fold(usage.total_tokens, |total, message| {
                total.saturating_add(estimate_message_tokens(message))
            });
    }

    let message_tokens = messages
        .iter()
        .filter(|message| message.role != Role::Internal)
        .fold(0u32, |total, message| {
            total.saturating_add(estimate_message_tokens(message))
        });
    message_tokens.saturating_add(estimate_tools_tokens(tools))
}

fn estimate_tools_tokens(tools: &[Arc<ToolDefinition>]) -> u32 {
    tools.iter().fold(0u32, |total, tool| {
        total.saturating_add(if tool.estimated_tokens > 0 {
            tool.estimated_tokens
        } else {
            tool.estimated_tokens()
        })
    })
}

/// Resolve a request-specific model config at the call site.
///
/// Providers must not mutate or infer output limits. Callers resolve the limit
/// once from the exact messages/tools they are about to send, then pass the
/// returned config unchanged to the provider.
pub fn resolve_request_config(
    messages: &[Arc<Message>],
    tools: &[Arc<ToolDefinition>],
    config: &ModelConfig,
) -> Result<ModelConfig, ProviderError> {
    let input_tokens = estimate_request_input_tokens(messages, tools, config);
    let available_output = config
        .context_window
        .saturating_sub(input_tokens)
        .saturating_sub(CONTEXT_SAFETY_BUFFER_TOKENS);
    if available_output == 0 {
        return Err(ProviderError::Config(format!(
            "Insufficient context for model output: context_window={}, estimated_input={}, safety_buffer={}",
            config.context_window, input_tokens, CONTEXT_SAFETY_BUFFER_TOKENS
        )));
    }

    let resolved_max_tokens = config
        .max_tokens
        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS)
        .min(available_output);
    if resolved_max_tokens == 0 {
        return Err(ProviderError::Config(
            "Resolved max_tokens must be greater than 0".to_string(),
        ));
    }
    if config.provider == crate::config::ModelProvider::Anthropic
        && config.thinking.enabled
        && config.thinking.budget_tokens >= resolved_max_tokens
    {
        return Err(ProviderError::Config(format!(
            "Anthropic thinking budget ({}) must be smaller than resolved max_tokens ({resolved_max_tokens})",
            config.thinking.budget_tokens
        )));
    }

    let mut resolved = config.clone();
    resolved.max_tokens = Some(resolved_max_tokens);
    Ok(resolved)
}

#[cfg(test)]
#[path = "provider_test.rs"]
mod tests;

/// Global shared HTTP client for all providers.
static HTTP_CLIENT: std::sync::LazyLock<std::sync::Arc<reqwest::Client>> =
    std::sync::LazyLock::new(|| {
        std::sync::Arc::new(
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_mins(2))
                .build()
                .expect("failed to build global HTTP client"),
        )
    });

/// Get the global shared HTTP client used by all providers.
pub fn http_client() -> std::sync::Arc<reqwest::Client> {
    std::sync::Arc::clone(&HTTP_CLIENT)
}

/// Stream of model events
pub type ModelStream =
    Pin<Box<dyn futures::Stream<Item = Result<ModelStreamItem, ProviderError>> + Send>>;

/// Token usage information from API response
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    /// Cached tokens (for providers that support prompt caching)
    pub cached_tokens: Option<u32>,
}

impl TokenUsage {
    /// Create a new `TokenUsage`
    pub const fn new(
        prompt_tokens: u32,
        completion_tokens: u32,
        cached_tokens: Option<u32>,
    ) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            cached_tokens,
        }
    }

    /// Get total tokens (prompt + completion)
    pub const fn total_tokens(&self) -> u32 {
        self.prompt_tokens + self.completion_tokens
    }
}

/// Items emitted by model stream
#[derive(Debug, Clone)]
pub enum ModelStreamItem {
    Chunk(ContentChunk),
    /// Incremental tool call update (for UI feedback during argument streaming)
    /// Only contains the newly added fragment, not the accumulated arguments.
    ToolCallDelta {
        id: String,
        name: String,
        /// Newly added argument fragment (delta), not the full accumulated string
        arguments_delta: String,
    },
    /// Complete tool call (final)
    ToolCall(ToolCallRequest),
    Complete,
    Fallback {
        from: String,
        to: String,
    },
    TokenUsage(TokenUsage),
    /// API response metadata (id, `finish_reason`, etc.)
    /// Emitted when the stream ends with the final chunk's metadata
    ResponseMeta {
        /// API response ID (e.g., "chatcmpl-xxx"), if provided by the API
        response_id: Option<String>,
        /// Finish reason (normalized across providers)
        finish_reason: Option<FinishReason>,
    },
}

/// Tool call request from model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Thinking configuration for supported models
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ThinkingConfig {
    pub enabled: bool,
    pub budget_tokens: u32,
    /// Reasoning effort / output quality level (low/medium/high)
    /// Used for `OpenAI` `reasoning_effort` and `Anthropic` `output_config.effort`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            budget_tokens: 2048,
            effort: None,
        }
    }
}

/// Model configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// 模型的唯一标识名（如 "`claude_sonnet"、"gpt4o`"）
    pub name: String,
    pub provider: crate::config::ModelProvider,
    pub model_id: String,
    pub endpoint: String,
    pub api_key: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub fallback_model_id: Option<String>,
    pub sse_timeout_secs: u64,
    pub thinking: ThinkingConfig,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub headers: HashMap<String, String>,
    /// 该模型对应的上下文窗口大小
    pub context_window: u32,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            provider: crate::config::ModelProvider::default(),
            model_id: String::new(),
            endpoint: String::new(),
            api_key: String::new(),
            max_tokens: None,
            temperature: None,
            fallback_model_id: None,
            sse_timeout_secs: 30,
            thinking: ThinkingConfig::default(),
            headers: HashMap::new(),
            context_window: 131_072, // 128k
        }
    }
}

impl ModelConfig {
    /// 检查 API key 是否配置
    #[inline]
    pub const fn has_api_key(&self) -> bool {
        !self.api_key.is_empty()
    }
}

/// HTTP error with status code for retry decisions
#[derive(Error, Debug, Clone)]
#[error("HTTP error {0}")]
pub struct HttpError(pub u16);

impl HttpError {
    /// Returns true if this error is retryable
    /// Retryable: 5xx, 429 rate limit
    /// Not retryable: other 4xx
    pub const fn is_retryable(&self) -> bool {
        matches!(self.0, 429 | 500..=599)
    }
}

/// Provider error type using thiserror
#[derive(Error, Debug, Clone)]
pub enum ProviderError {
    /// HTTP error with status code (retryable based on code)
    #[error("{0}")]
    Http(#[from] HttpError),

    /// Request building or sending failed
    #[error("Request failed: {0}")]
    Request(String),

    /// SSE/streaming error
    #[error("SSE error: {0}")]
    Sse(String),

    /// Timeout error
    #[error("Timeout: {0}")]
    Timeout(String),

    /// JSON parse error
    #[error("Parse error: {0}")]
    Parse(String),

    /// API response error with provider code and explicit retry classification
    #[error("API error ({code:?}): {message}")]
    Api {
        code: Option<String>,
        message: String,
        retryable: bool,
    },

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),
}

impl ProviderError {
    /// Returns true when the provider rejected the request because the input exceeded its context window.
    pub fn is_context_overflow(&self) -> bool {
        let contains_overflow = |message: &str| {
            let message = message.to_ascii_lowercase();
            message.contains("context_length_exceeded")
                || message.contains("input_too_long")
                || message.contains("context window")
                || message.contains("context length")
                || message.contains("maximum context")
                || message.contains("max context")
                || message.contains("input tokens exceed")
                || message.contains("input exceeds")
                || message.contains("prompt is too long")
                || message.contains("insufficient context")
        };
        match self {
            ProviderError::Api { code, message, .. } => {
                code.as_deref().is_some_and(|code| {
                    code.eq_ignore_ascii_case("context_length_exceeded")
                        || code.eq_ignore_ascii_case("input_too_long")
                        || code.eq_ignore_ascii_case("prompt_too_long")
                }) || contains_overflow(message)
            }
            ProviderError::Parse(message) | ProviderError::Config(message) => {
                contains_overflow(message)
            }
            _ => false,
        }
    }

    /// Returns true if this error is retryable
    pub const fn is_retryable(&self) -> bool {
        match self {
            ProviderError::Http(e) => e.is_retryable(),
            ProviderError::Timeout(_) | ProviderError::Request(_) | ProviderError::Sse(_) => true,
            ProviderError::Api { retryable, .. } => *retryable,
            ProviderError::Parse(_) | ProviderError::Config(_) => false,
        }
    }
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            ProviderError::Timeout(format!("Request timeout: {e}"))
        } else if let Some(status) = e.status() {
            ProviderError::Http(HttpError(status.as_u16()))
        } else {
            ProviderError::Request(format!("Request failed: {e}"))
        }
    }
}

use std::sync::Arc;

/// Core trait for model providers
#[async_trait]
pub trait Provider: Send + Sync {
    async fn stream(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
    ) -> Result<ModelStream, ProviderError>;

    fn supports_streaming(&self) -> bool {
        true
    }

    fn name(&self) -> &str;
}

/// A provider that always returns a configuration error (no API key).
/// Used so the GUI can start even when the API key is missing.
#[derive(Debug)]
pub struct NoKeyProvider;

#[async_trait]
impl Provider for NoKeyProvider {
    async fn stream(
        &self,
        _messages: &[Arc<Message>],
        _tools: &[Arc<ToolDefinition>],
        _config: &ModelConfig,
    ) -> Result<ModelStream, ProviderError> {
        tracing::error!("NoKeyProvider.stream called — API key not configured");
        Err(ProviderError::Config(
            "API key not configured. Please set it via the config editor or environment variable."
                .into(),
        ))
    }

    fn name(&self) -> &'static str {
        "no-key"
    }
}

/// Wrapper that adds rate limit retry with exponential backoff
pub struct RetryingProvider<P: Provider> {
    inner: P,
    max_retries: u32,
    base_delay_ms: u64,
}

impl<P: Provider> RetryingProvider<P> {
    pub const fn new(inner: P) -> Self {
        Self {
            inner,
            max_retries: 3,
            base_delay_ms: 1000,
        }
    }

    #[must_use]
    pub const fn with_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

#[async_trait]
impl<P: Provider> Provider for RetryingProvider<P> {
    async fn stream(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
    ) -> Result<ModelStream, ProviderError> {
        let mut attempt = 0;
        loop {
            match self.inner.stream(messages, tools, config).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    attempt += 1;
                    if attempt > self.max_retries {
                        return Err(e);
                    }
                    if e.is_retryable() {
                        let delay = self.base_delay_ms * 2_u64.pow(attempt - 1);
                        tracing::warn!(
                            "Provider error (retryable), retrying in {}ms (attempt {}/{}): {}",
                            delay,
                            attempt,
                            self.max_retries,
                            e
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}
