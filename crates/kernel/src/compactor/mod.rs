//! Context compression for managing long conversations
//!
//! Implements two strategies:
//! 1. Micro-compaction: Clear old tool result content (fast, no API call)
//! 2. Full summarization: Use API to generate conversation summary

mod types;

use serde::{Deserialize, Serialize};
pub use types::*;

use crate::providers::{ModelConfig, ModelStreamItem, Provider};
use crate::types::{ContentBlock, Message, Role};
use anyhow::Result;
use futures::TryStreamExt;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Errors that can occur during compaction
#[derive(Debug, thiserror::Error)]
pub enum CompactionError {
    #[error("Compaction was cancelled")]
    Cancelled,
    #[error("API error: {0}")]
    Api(#[from] anyhow::Error),
}

impl From<crate::providers::ProviderError> for CompactionError {
    fn from(e: crate::providers::ProviderError) -> Self {
        CompactionError::Api(e.into())
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
    ((content_len / 4) + 10) as u32 // +10 for overhead
}

/// Default token threshold to trigger compaction (80% of context window)
pub const DEFAULT_COMPACT_THRESHOLD: u32 = 104_857; // 80% of 131,072
/// Default context window size
pub const DEFAULT_CONTEXT_WINDOW: u32 = 131_072; // 128k
/// Number of recent messages to keep during compaction
const KEEP_RECENT_MESSAGES: usize = 6;
/// Max tokens for summary generation
const SUMMARY_MAX_TOKENS: u32 = 4000;

/// Summary prompt for full compaction
const SUMMARY_PROMPT: &str = include_str!("summary_prompt.txt");

/// Compactor for managing conversation context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Compactor {
    /// Token threshold to trigger compaction
    pub compact_threshold: u32,
    /// Total context window size
    pub context_window: u32,
    /// Number of recent messages to preserve
    pub keep_recent: usize,
    /// Max tokens for summary
    pub summary_max_tokens: u32,
}

impl Default for Compactor {
    fn default() -> Self {
        Self {
            compact_threshold: DEFAULT_COMPACT_THRESHOLD,
            context_window: DEFAULT_CONTEXT_WINDOW,
            keep_recent: KEEP_RECENT_MESSAGES,
            summary_max_tokens: SUMMARY_MAX_TOKENS,
        }
    }
}

impl Compactor {
    /// Create a new compactor with custom settings
    pub const fn new(
        compact_threshold: u32,
        context_window: u32,
        keep_recent: usize,
        summary_max_tokens: u32,
    ) -> Self {
        Self {
            compact_threshold,
            context_window,
            keep_recent,
            summary_max_tokens,
        }
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
    pub fn should_compact(&self, messages: &[Arc<Message>]) -> bool {
        let tokens = Self::calculate_tokens(messages);
        tokens >= self.compact_threshold
    }

    /// Try micro-compaction: clear old tool results
    /// Returns `Some(new_messages)` if compaction was performed, None otherwise
    pub fn micro_compact(&self, messages: &[Arc<Message>]) -> Option<Vec<Arc<Message>>> {
        const CLEARED_MARKER: &str = "[Old tool result content cleared]";

        let keep_start = messages.len().saturating_sub(self.keep_recent);
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
            Some(result)
        } else {
            None
        }
    }

    /// Perform full compaction: generate summary using API
    /// Returns a summary message and the recent messages to keep
    /// Supports cancellation via `cancel_rx`
    pub async fn full_compact(
        &self,
        messages: &[Arc<Message>],
        provider: &dyn Provider,
        model_config: &ModelConfig,
        cancel_rx: Option<&mut oneshot::Receiver<()>>,
    ) -> Result<CompactionResult, CompactionError> {
        if messages.len() <= self.keep_recent {
            return Ok(CompactionResult {
                summary: None,
                keep_messages: messages.iter().map(|m| (**m).clone()).collect(),
                compacted_count: 0,
            });
        }

        let split_point = messages.len() - self.keep_recent;
        let to_summarize = &messages[..split_point];
        let recent: Vec<Message> = messages[split_point..]
            .iter()
            .map(|m| (**m).clone())
            .collect();

        // Generate summary using API
        let summary_text = generate_summary_with_api(
            to_summarize,
            provider,
            model_config,
            self.summary_max_tokens,
            cancel_rx,
        )
        .await?;

        // Create summary message
        let summary_message = Message::system(summary_text);

        Ok(CompactionResult {
            summary: Some(summary_message),
            keep_messages: recent,
            compacted_count: to_summarize.len(),
        })
    }

    /// Auto-compact: try micro first, then full if needed
    /// Returns compacted messages if compaction was performed
    /// Supports cancellation via `cancel_rx`
    pub async fn auto_compact(
        &self,
        messages: &[Arc<Message>],
        provider: &dyn Provider,
        model_config: &ModelConfig,
        cancel_rx: Option<&mut oneshot::Receiver<()>>,
    ) -> Result<Option<Vec<Arc<Message>>>, CompactionError> {
        // Check if we need to compact
        if !self.should_compact(messages) {
            return Ok(None);
        }

        // Try micro-compaction first
        if let Some(compacted) = self.micro_compact(messages) {
            // Check if micro-compaction was sufficient
            if !self.should_compact(&compacted) {
                return Ok(Some(compacted));
            }

            // Micro-compaction wasn't enough, do full compaction
            let result = self
                .full_compact(&compacted, provider, model_config, cancel_rx)
                .await?;

            // Build new message list
            let new_messages = if let Some(ref summary) = result.summary {
                let mut msgs = vec![Arc::new(summary.clone())];
                msgs.extend(result.keep_messages.iter().map(|m| Arc::new(m.clone())));
                msgs
            } else {
                result
                    .keep_messages
                    .iter()
                    .map(|m| Arc::new(m.clone()))
                    .collect()
            };

            return Ok(Some(new_messages));
        }

        // Micro-compaction didn't help, do full compaction
        let result = self
            .full_compact(messages, provider, model_config, cancel_rx)
            .await?;

        // Build new message list
        let new_messages = if let Some(ref summary) = result.summary {
            let mut msgs = vec![Arc::new(summary.clone())];
            msgs.extend(result.keep_messages.iter().map(|m| Arc::new(m.clone())));
            msgs
        } else {
            result
                .keep_messages
                .iter()
                .map(|m| Arc::new(m.clone()))
                .collect()
        };

        Ok(Some(new_messages))
    }
}

/// Generate summary using API call
/// Returns Err if cancelled or API fails
async fn generate_summary_with_api(
    messages: &[Arc<Message>],
    provider: &dyn Provider,
    model_config: &ModelConfig,
    max_tokens: u32,
    mut cancel_rx: Option<&mut oneshot::Receiver<()>>,
) -> Result<String, CompactionError> {
    // Build messages for summary generation
    let mut summary_messages: Vec<Arc<Message>> = vec![Arc::new(Message::system(SUMMARY_PROMPT))];

    // Add conversation to summarize
    for msg in messages {
        summary_messages.push(Arc::clone(msg));
    }

    // Add final instruction
    summary_messages.push(Arc::new(Message::user(
        "Please provide a comprehensive summary of our conversation above.",
    )));

    // Create a config with limited max_tokens for summary
    let summary_config = ModelConfig {
        max_tokens: Some(max_tokens),
        ..model_config.clone()
    };

    // Call API
    let mut stream = provider
        .stream(&summary_messages, &[], &summary_config)
        .await?;

    // Collect response with cancellation check
    let mut summary = String::with_capacity(max_tokens as usize * 4); // Rough estimate of chars per token
    loop {
        // Check if cancelled (non-blocking check)
        if let Some(ref mut rx) = cancel_rx {
            match rx.try_recv() {
                Ok(()) => return Err(CompactionError::Cancelled),
                Err(
                    tokio::sync::oneshot::error::TryRecvError::Closed
                    | tokio::sync::oneshot::error::TryRecvError::Empty,
                ) => {}
            }
        }

        match tokio::time::timeout(std::time::Duration::from_millis(100), stream.try_next()).await {
            Ok(Ok(Some(item))) => match item {
                ModelStreamItem::Chunk(crate::event::ContentChunk::Text(text)) => {
                    summary.push_str(&text);
                }
                ModelStreamItem::Complete => break,
                _ => {} // Ignore other item types
            },
            Ok(Ok(None)) => break,
            Ok(Err(e)) => return Err(e.into()),
            // Timeout, continue loop to check cancellation
            Err(_) => {}
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessageTokenUsage;
    use std::sync::Arc;

    #[test]
    fn test_calculate_tokens_with_usage() {
        let messages: Vec<Arc<Message>> = vec![
            Arc::new(Message::user("Hello")),
            Arc::new(Message::assistant("Hi there")),
            {
                let mut msg = Message::assistant("Let me help");
                msg.token_usage = Some(MessageTokenUsage {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                    total_tokens: 150,
                });
                Arc::new(msg)
            },
        ];

        let tokens = Compactor::calculate_tokens(&messages);
        // Should use the actual usage (150) plus estimation for messages after
        assert!(tokens >= 150);
    }

    #[test]
    fn test_micro_compact() {
        use std::sync::Arc;

        let compactor = Compactor::new(100, 200, 2, 1000); // keep last 2 messages
        let messages: Vec<Arc<Message>> = vec![
            Arc::new(Message::user("Task 1")),
            Arc::new(Message::tool_result("call-1", "Result 1")), // will be cleared (index 1)
            Arc::new(Message::user("Task 2")),
            Arc::new(Message::tool_result("call-2", "Result 2")), // kept (index 3, in keep_recent)
            Arc::new(Message::user("Current task")),              // kept (index 4)
        ];

        let compacted = compactor.micro_compact(&messages);
        assert!(compacted.is_some());
        let new_messages = compacted.unwrap();
        // Old tool result should be cleared
        assert_eq!(
            new_messages[1].text_content(),
            "[Old tool result content cleared]"
        );
        // Recent tool result should be preserved (keep_recent = 2)
        assert_eq!(new_messages[3].text_content(), "Result 2");
        assert_eq!(new_messages[4].text_content(), "Current task");

        // Second compaction should return None (already cleared)
        let compacted_again = compactor.micro_compact(&new_messages);
        assert!(compacted_again.is_none());
    }
}
