use crate::event::ContentChunk;
use anyhow::Result;
use async_trait::async_trait;
use nekoclaw_shared::types::{Message, ToolDefinition};
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// Stream of model events
pub type ModelStream = Pin<Box<dyn futures::Stream<Item = Result<ModelStreamItem>> + Send>>;

/// Items emitted by model stream
#[derive(Debug, Clone)]
pub enum ModelStreamItem {
    Chunk(ContentChunk),
    ToolCall(ToolCallRequest),
    Complete,
    Fallback { from: String, to: String },
}

/// Tool call request from model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
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
        }
    }
}

/// Core trait for model providers
#[async_trait]
pub trait ModelProvider: Send + Sync {
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> Result<ModelStream>;

    fn supports_streaming(&self) -> bool {
        true
    }

    fn name(&self) -> &str;
}

/// Wrapper that adds rate limit retry with exponential backoff
pub struct RetryingProvider<P: ModelProvider> {
    inner: P,
    max_retries: u32,
    base_delay_ms: u64,
}

impl<P: ModelProvider> RetryingProvider<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            max_retries: 3,
            base_delay_ms: 1000,
        }
    }

    pub fn with_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

#[async_trait]
impl<P: ModelProvider> ModelProvider for RetryingProvider<P> {
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> Result<ModelStream> {
        let mut attempt = 0;
        loop {
            match self.inner.stream(messages, tools, config).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    attempt += 1;
                    if attempt > self.max_retries {
                        return Err(e);
                    }
                    let err_str = e.to_string();
                    if err_str.contains("429") || err_str.contains("rate limit") {
                        let delay = self.base_delay_ms * 2_u64.pow(attempt - 1);
                        tracing::warn!(
                            "Rate limited, retrying in {}ms (attempt {}/{})",
                            delay,
                            attempt,
                            self.max_retries
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
