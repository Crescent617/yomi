use crate::event::ContentChunk;
use crate::types::{FinishReason, Message, ToolDefinition};
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

/// Estimate request input tokens using the shared token utilities.
pub fn estimate_request_input_tokens(
    messages: &[Arc<Message>],
    tools: &[Arc<ToolDefinition>],
    _config: &ModelConfig,
) -> u32 {
    crate::utils::tokens::estimate_request_input_tokens(messages, tools)
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
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ThinkingConfig {
    /// Enable thinking/reasoning output
    pub enabled: bool,
    /// Maximum tokens for thinking (Anthropic `budget_tokens`)
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
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct ModelConfig {
    /// 模型的唯一标识名（如 "`claude_sonnet"、"gpt4o`"）
    pub name: String,
    pub provider: crate::config::ModelProvider,
    /// Model identifier sent to the provider API
    pub model_id: String,
    /// API base URL of the provider
    pub endpoint: String,
    /// API key for the provider
    pub api_key: String,
    /// Maximum output tokens per request
    pub max_tokens: Option<u32>,
    /// Sampling temperature
    pub temperature: Option<f32>,
    /// SSE stream read timeout in seconds
    pub sse_timeout_secs: u64,
    pub thinking: ThinkingConfig,
    /// Extra HTTP headers sent with every request
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
#[error("HTTP error {status}")]
pub struct HttpError {
    pub status: u16,
    /// Server-provided `Retry-After` hint (seconds form), when present.
    pub retry_after: Option<std::time::Duration>,
}

impl HttpError {
    pub const fn new(status: u16, retry_after: Option<std::time::Duration>) -> Self {
        Self {
            status,
            retry_after,
        }
    }

    /// Returns true if this error is retryable
    /// Retryable: 5xx, 429 rate limit
    /// Not retryable: other 4xx
    pub const fn is_retryable(&self) -> bool {
        matches!(self.status, 429 | 500..=599)
    }
}

/// Parse the `Retry-After` response header (seconds form; the HTTP-date
/// form is not emitted by LLM APIs and is ignored).
pub(crate) fn parse_retry_after(
    headers: &reqwest::header::HeaderMap,
) -> Option<std::time::Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(std::time::Duration::from_secs)
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

    /// Server-provided retry delay hint (`Retry-After` header), when present.
    pub const fn retry_after(&self) -> Option<std::time::Duration> {
        match self {
            ProviderError::Http(e) => e.retry_after,
            _ => None,
        }
    }
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            ProviderError::Timeout(format!("Request timeout: {e}"))
        } else if let Some(status) = e.status() {
            // reqwest::Error carries no response headers — no Retry-After.
            ProviderError::Http(HttpError::new(status.as_u16(), None))
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
