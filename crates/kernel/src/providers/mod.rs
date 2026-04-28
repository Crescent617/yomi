use crate::event::ContentChunk;
use crate::types::{Message, ToolDefinition};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::pin::Pin;
use thiserror::Error;

pub mod anthropic;
pub mod openai;

pub use anthropic::AnthropicProvider;
pub use openai::OpenAIProvider;

/// Stream of model events
pub type ModelStream =
    Pin<Box<dyn futures::Stream<Item = Result<ModelStreamItem, ProviderError>> + Send>>;

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
    TokenUsage {
        prompt_tokens: u32,
        completion_tokens: u32,
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
pub struct ThinkingConfig {
    pub enabled: bool,
    pub budget_tokens: u32,
    /// Reasoning effort level for `OpenAI` o1/o3 models (low/medium/high)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl Default for ThinkingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            budget_tokens: 1024,
            effort: None,
        }
    }
}

/// Model configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model_id: String,
    pub endpoint: String,
    pub api_key: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub fallback_model_id: Option<String>,
    pub sse_timeout_secs: u64,
    pub thinking: ThinkingConfig,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            model_id: String::new(),
            endpoint: String::new(),
            api_key: String::new(),
            max_tokens: None,
            temperature: None,
            fallback_model_id: None,
            sse_timeout_secs: 30,
            thinking: ThinkingConfig::default(),
        }
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
    #[error("HTTP error: {0}")]
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

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),
}

impl ProviderError {
    /// Returns true if this error is retryable
    pub const fn is_retryable(&self) -> bool {
        match self {
            ProviderError::Http(e) => e.is_retryable(),
            ProviderError::Timeout(_) | ProviderError::Request(_) | ProviderError::Sse(_) => true,
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
