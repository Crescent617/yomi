//! Context compression for managing long conversations
//!
//! Implements two strategies:
//! 1. Micro-compaction: Clear old tool result content (fast, no API call)
//! 2. Full summarization: Use API to generate conversation summary

use crate::provider::{ModelConfig, ModelStreamItem, Provider};
use crate::types::{ContentBlock, FinishReason, Message, Role};
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Default threshold ratio to trigger compaction (80% of context window)
pub const DEFAULT_THRESHOLD_RATIO: f32 = 0.8;
/// Default context window size
pub const DEFAULT_CONTEXT_WINDOW: u32 = 131_072; // 128k
/// Number of recent messages to keep during full compaction
const KEEP_RECENT_MESSAGES: usize = 0;
/// Number of recent messages whose tool results survive micro-compaction
const KEEP_RECENT_TOOL_RESULTS: usize = 5;
/// Max tokens for summary generation
const SUMMARY_MAX_TOKENS: u32 = 8192; // 8k tokens for summary

/// Compaction result containing compacted messages and token usage
#[derive(Debug, Clone)]
pub struct CompactionResult {
    pub messages: Vec<Arc<Message>>,
    pub token_usage: crate::provider::TokenUsage,
}

impl CompactionResult {
    /// Create a new compaction result
    pub fn new(messages: Vec<Arc<Message>>, token_usage: crate::provider::TokenUsage) -> Self {
        Self {
            messages,
            token_usage,
        }
    }
}

/// Summary prompt for full compaction
const SUMMARY_PROMPT: &str = include_str!("summary_prompt.txt");
/// Errors that can occur during compaction
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("Compaction was cancelled")]
    Cancelled,
    #[error("API error: {0}")]
    Api(String),
}

impl From<crate::provider::ProviderError> for CompactionError {
    fn from(e: crate::provider::ProviderError) -> Self {
        CompactionError::Api(e.to_string())
    }
}

/// Helper to estimate tokens for Arc-wrapped messages
fn estimate_tokens_for_arc_messages(messages: &[Arc<Message>]) -> u32 {
    messages
        .iter()
        .map(|m| estimate_tokens_for_message(m))
        .sum()
}

/// Estimate tokens for a single message
fn estimate_tokens_for_message(msg: &Message) -> u32 {
    // Simple estimation: ~4 characters per token
    let content_len: usize = msg
        .content
        .iter()
        .map(|c| match c {
            crate::types::ContentBlock::Text { text } => text.len(),
            _ => 0,
        })
        .sum();
    // Use saturating arithmetic to prevent overflow
    content_len
        .saturating_div(4)
        .saturating_add(10)
        .min(u32::MAX as usize) as u32
}

/// Estimate total tokens for messages and set usage on the last message.
/// This allows `calculate_tokens` to use this as a baseline for future calculations.
fn set_token_usage_on_last(messages: &mut [Arc<Message>]) {
    if messages.is_empty() {
        return;
    }

    let total_tokens = estimate_tokens_for_arc_messages(messages);

    // Get the last message and set its token_usage
    if let Some(last) = messages.last_mut() {
        Arc::make_mut(last).token_usage = Some(crate::types::MessageTokenUsage {
            prompt_tokens: total_tokens,
            completion_tokens: 0,
            total_tokens,
        });
    }
}

/// Compactor for managing conversation context
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Compactor {
    /// Ratio (0.0–1.0) of the context window at which compaction is triggered
    pub threshold_ratio: f32,
    /// Number of recent messages to preserve during full compaction
    pub keep_recent_messages: usize,
    /// Number of recent messages whose tool results survive micro-compaction
    pub keep_recent_tool_results: usize,
    /// Max tokens for summary
    pub summary_max_tokens: u32,
}

impl Default for Compactor {
    fn default() -> Self {
        Self {
            threshold_ratio: DEFAULT_THRESHOLD_RATIO,
            keep_recent_messages: KEEP_RECENT_MESSAGES,
            keep_recent_tool_results: KEEP_RECENT_TOOL_RESULTS,
            summary_max_tokens: SUMMARY_MAX_TOKENS,
        }
    }
}

impl Compactor {
    /// Create a new compactor with custom settings
    pub const fn new(
        threshold_ratio: f32,
        keep_recent_messages: usize,
        keep_recent_tool_results: usize,
        summary_max_tokens: u32,
    ) -> Self {
        Self {
            threshold_ratio,
            keep_recent_messages,
            keep_recent_tool_results,
            summary_max_tokens,
        }
    }

    /// Compute the absolute token threshold from the ratio and context window.
    #[allow(clippy::cast_precision_loss)]
    pub fn threshold(&self, context_window: u32) -> u32 {
        (context_window as f32 * self.threshold_ratio) as u32
    }

    /// Calculate total tokens from message history
    /// Uses actual token usage from API responses when available
    pub fn calculate_tokens(messages: &[Arc<Message>]) -> u32 {
        let mut total = 0u32;
        let mut last_usage_idx: Option<usize> = None;

        // Walk backwards to find the last message with token usage
        for (i, msg) in messages.iter().enumerate().rev() {
            if msg.token_usage.is_some() {
                last_usage_idx = Some(i);
                break;
            }
        }

        if let Some(idx) = last_usage_idx {
            // Use the actual token usage from the last API response
            if let Some(usage) = messages[idx].token_usage {
                total += usage.total_tokens;
                // Add rough estimation for messages after the last tracked usage
                total += estimate_tokens_for_arc_messages(&messages[idx + 1..]);
            }
        } else {
            // No tracked usage, estimate all messages
            total += estimate_tokens_for_arc_messages(messages);
        }

        total
    }

    /// Check if compaction should be triggered
    pub fn should_compact(&self, messages: &[Arc<Message>], context_window: u32) -> bool {
        let tokens = Self::calculate_tokens(messages);
        tokens >= self.threshold(context_window)
    }

    /// Try micro-compaction: clear old tool results
    /// Returns `Some(new_messages)` if compaction was performed, None otherwise
    pub fn micro_compact(&self, messages: &[Arc<Message>]) -> Option<Vec<Arc<Message>>> {
        const CLEARED_MARKER: &str = "[Old tool result content cleared]";

        let keep_start = messages.len().saturating_sub(self.keep_recent_tool_results);
        if keep_start == 0 {
            return None;
        }

        let mut modified = false;
        let mut result = Vec::with_capacity(messages.len());

        for (idx, msg) in messages.iter().enumerate() {
            if idx < keep_start
                && msg.role == Role::Tool
                && msg.content.first().is_some_and(|c| {
                    if let ContentBlock::Text { text } = c {
                        text != CLEARED_MARKER
                    } else {
                        false
                    }
                })
            {
                // Need to clear this message
                let mut new_msg = (**msg).clone();
                new_msg.content = vec![ContentBlock::Text {
                    text: CLEARED_MARKER.to_string(),
                }];
                result.push(Arc::new(new_msg));
                modified = true;
            } else {
                result.push(Arc::clone(msg));
            }
        }

        if modified {
            // Estimate total tokens and set on the last message for accurate future calculations
            set_token_usage_on_last(&mut result);
            Some(result)
        } else {
            None
        }
    }

    /// Perform full compaction: generate summary using API.
    ///
    /// Returns `CompactionResult` containing messages in order: [summary] + recent
    /// Note: System messages are NOT included in the returned result - they are
    /// recreated by the agent on session restore to avoid duplication.
    ///
    /// Supports cancellation via `cancel_token`.
    pub async fn full_compact(
        &self,
        messages: &[Arc<Message>],
        provider: Arc<dyn Provider>,
        model_config: &ModelConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Result<CompactionResult, CompactionError> {
        // Separate system messages from the rest
        let (_system_msgs, non_system): (Vec<_>, Vec<_>) = messages
            .iter()
            .cloned()
            .partition(|m| m.role == Role::System);

        if non_system.len() <= self.keep_recent_messages {
            // Not enough non-system messages to compact, keep everything as-is
            // Note: We still filter out system messages here
            return Ok(CompactionResult::new(
                non_system,
                crate::provider::TokenUsage::default(),
            ));
        }

        let split_point = non_system.len() - self.keep_recent_messages;
        let to_summarize = &non_system[..split_point];
        let recent: Vec<Arc<Message>> = non_system[split_point..].to_vec();

        // Generate summary using API
        let (summary_text, token_usage) = generate_summary(
            to_summarize,
            provider,
            model_config,
            self.summary_max_tokens,
            cancel_token,
        )
        .await?;

        // Create summary message as user role so it survives session restore
        let summary = Message::user(summary_text);
        // Reconstruct: summary + recent (system_msgs NOT included)
        let mut result: Vec<Arc<Message>> =
            std::iter::once(Arc::new(summary)).chain(recent).collect();

        // Estimate total tokens and set on the last message for accurate future calculations
        set_token_usage_on_last(&mut result);
        Ok(CompactionResult::new(result, token_usage))
    }

    /// Auto-compact: try micro first, then full if needed.
    ///
    /// Returns `Some(CompactionResult)` if compaction was performed, `None` otherwise.
    /// Supports cancellation via `cancel_token`.
    pub async fn auto_compact(
        &self,
        messages: &[Arc<Message>],
        provider: Arc<dyn Provider>,
        model_config: &ModelConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Result<Option<CompactionResult>, CompactionError> {
        if !self.should_compact(messages, model_config.context_window) {
            return Ok(None);
        }

        // Try micro-compaction first
        if let Some(after_micro) = self.micro_compact(messages) {
            // Check if micro-compaction was sufficient
            if !self.should_compact(&after_micro, model_config.context_window) {
                return Ok(Some(CompactionResult::new(
                    after_micro,
                    crate::provider::TokenUsage::default(),
                )));
            }
            // Need full compaction on top of micro results
            return self
                .full_compact(
                    &after_micro,
                    Arc::clone(&provider),
                    model_config,
                    cancel_token,
                )
                .await
                .map(Some);
        }

        // No micro-compaction possible, do full compaction directly
        self.full_compact(messages, Arc::clone(&provider), model_config, cancel_token)
            .await
            .map(Some)
    }
}

/// Generate summary using API call.
/// Returns (summary, `token_usage`) or Err if cancelled or API fails.
#[allow(clippy::semicolon_if_nothing_returned)]
async fn generate_summary(
    messages: &[Arc<Message>],
    provider: Arc<dyn Provider>,
    model_config: &ModelConfig,
    summary_max_tokens: u32,
    cancel_token: Option<CancellationToken>,
) -> Result<(String, crate::provider::TokenUsage), CompactionError> {
    use crate::agent::MessageBuffer;

    let mut msg_buf = MessageBuffer::from_arc_messages(messages);
    msg_buf.sanitize();
    let messages = msg_buf.messages();

    // Build messages for summary generation
    let mut summary_messages: Vec<Arc<Message>> = vec![Arc::new(Message::system(SUMMARY_PROMPT))];
    summary_messages.extend(messages.iter().cloned());
    summary_messages.push(Arc::new(Message::user(
        "Please provide a comprehensive summary of our conversation above.",
    )));

    // Create a config with limited max_tokens for summary
    let summary_config = ModelConfig {
        max_tokens: Some(summary_max_tokens),
        ..model_config.clone()
    };

    // Spawn provider request in a separate task to allow cancellation
    let summary_messages_clone = summary_messages;
    let summary_config_clone = summary_config;
    let provider_clone = Arc::clone(&provider);
    let stream_task = tokio::spawn(async move {
        provider_clone
            .stream(&summary_messages_clone, &[], &summary_config_clone)
            .await
    });
    let abort_handle = stream_task.abort_handle();

    // Call API with cancellation support
    let mut stream = tokio::select! {
        biased;
        () = async {
            if let Some(ref t) = cancel_token {
                t.cancelled().await
            } else {
                std::future::pending().await
            }
        } => {
            abort_handle.abort();
            return Err(CompactionError::Cancelled);
        }
        result = stream_task => match result {
            Ok(Ok(stream)) => stream,
            Ok(Err(e)) => return Err(e.into()),
            Err(e) if e.is_cancelled() => return Err(CompactionError::Cancelled),
            Err(e) => return Err(CompactionError::Api(format!("Summary stream task panicked: {e}"))),
        }
    };

    // Collect response with cancellation check
    let mut summary = String::new();
    let mut token_usage = crate::provider::TokenUsage::default();
    let mut finish_reason = None;

    loop {
        let item = tokio::select! {
            biased;
            () = async {
                if let Some(ref t) = cancel_token {
                    t.cancelled().await
                } else {
                    std::future::pending().await
                }
            } => {
                return Err(CompactionError::Cancelled);
            }
            item = stream.try_next() => match item {
                Ok(Some(item)) => item,
                Ok(None) => break,
                Err(e) => return Err(e.into()),
            }
        };

        match item {
            ModelStreamItem::Chunk(crate::event::ContentChunk::Text(text)) => {
                summary.push_str(&text);
            }
            ModelStreamItem::TokenUsage(usage) => {
                token_usage = usage;
            }
            ModelStreamItem::ResponseMeta {
                finish_reason: reason,
                ..
            } => {
                finish_reason = reason;
            }
            ModelStreamItem::Complete => break,
            _ => {}
        }
    }
    if finish_reason != Some(FinishReason::Stop) {
        return Err(CompactionError::Api(format!(
            "Summary generation did not finish normally: {finish_reason:?}"
        )));
    }
    if summary.trim().is_empty() {
        return Err(CompactionError::Api(
            "Summary generation returned an empty summary".to_string(),
        ));
    }
    Ok((summary, token_usage))
}

#[cfg(test)]
mod tests;
