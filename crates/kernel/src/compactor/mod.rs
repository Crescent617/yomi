//! Context compression for managing long conversations
//!
//! Implements two strategies:
//! 1. Micro-compaction: Clear old tool result content (fast, no API call)
//! 2. Full summarization: Use API to generate conversation summary (Claude Code style)

mod types;

use serde::{Deserialize, Serialize};
pub use types::*;

use crate::providers::{ModelConfig, ModelStreamItem, Provider};
use crate::types::{ContentBlock, Message, Role};
use crate::utils::tokens::estimate_tokens_for_messages;
use anyhow::Result;
use futures::TryStreamExt;

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
    pub fn calculate_tokens(messages: &[Message]) -> u32 {
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
                total += estimate_tokens_for_messages(&messages[idx + 1..]);
            }
        } else {
            // No tracked usage, estimate all messages
            total += estimate_tokens_for_messages(messages);
        }

        total
    }

    /// Check if compaction should be triggered
    pub fn should_compact(&self, messages: &[Message]) -> bool {
        let tokens = Self::calculate_tokens(messages);
        tokens >= self.compact_threshold
    }

    /// Try micro-compaction: clear old tool results
    /// Returns true only if any content was actually cleared (not already cleared)
    pub fn micro_compact(&self, messages: &mut [Message]) -> bool {
        const CLEARED_MARKER: &str = "[Old tool result content cleared]";

        let keep_start = messages.len().saturating_sub(self.keep_recent);
        if keep_start == 0 {
            return false;
        }

        // Only iterate over messages that need to be cleared (not the recent ones)
        messages[..keep_start]
            .iter_mut()
            .filter(|m| m.role == Role::Tool)
            .filter(|m| {
                if let Some(ContentBlock::Text { ref text }) = m.content.first() {
                    text != CLEARED_MARKER
                } else {
                    false
                }
            })
            .map(|m| {
                m.content = vec![ContentBlock::Text {
                    text: CLEARED_MARKER.to_string(),
                }];
            })
            .count()
            > 0
    }

    /// Perform full compaction: generate summary using API
    /// Returns a summary message and the recent messages to keep
    pub async fn full_compact(
        &self,
        messages: &[Message],
        provider: &dyn Provider,
        model_config: &ModelConfig,
    ) -> Result<CompactionResult> {
        if messages.len() <= self.keep_recent {
            return Ok(CompactionResult {
                summary: None,
                keep_messages: messages.to_vec(),
                compacted_count: 0,
            });
        }

        let split_point = messages.len() - self.keep_recent;
        let to_summarize = &messages[..split_point];
        let recent = messages[split_point..].to_vec();

        // Generate summary using API
        let summary_text = generate_summary_with_api(
            to_summarize,
            provider,
            model_config,
            self.summary_max_tokens,
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
    /// Returns compaction result if compaction was performed
    pub async fn auto_compact(
        &self,
        messages: &mut Vec<Message>,
        provider: &dyn Provider,
        model_config: &ModelConfig,
    ) -> Result<Option<CompactionResult>> {
        // Check if we need to compact
        if !self.should_compact(messages) {
            return Ok(None);
        }

        // Try micro-compaction first
        if self.micro_compact(messages) {
            // Check if micro-compaction was sufficient
            if !self.should_compact(messages) {
                return Ok(Some(CompactionResult {
                    summary: None,
                    keep_messages: messages.clone(),
                    compacted_count: 0,
                }));
            }
        }

        // Micro-compaction wasn't enough, do full compaction
        let result = self.full_compact(messages, provider, model_config).await?;

        // Replace messages with compacted version
        *messages = if let Some(ref summary) = result.summary {
            let mut new_messages = vec![];
            new_messages.push(summary.clone());
            new_messages.extend(result.keep_messages.clone());
            new_messages
        } else {
            result.keep_messages.clone()
        };

        Ok(Some(result))
    }
}

/// Generate summary using API call
async fn generate_summary_with_api(
    messages: &[Message],
    provider: &dyn Provider,
    model_config: &ModelConfig,
    max_tokens: u32,
) -> Result<String> {
    // Build messages for summary generation
    let mut summary_messages = vec![Message::system(SUMMARY_PROMPT)];

    // Add conversation to summarize
    for msg in messages {
        summary_messages.push(msg.clone());
    }

    // Add final instruction
    summary_messages.push(Message::user(
        "Please provide a comprehensive summary of our conversation above.",
    ));

    // Create a config with limited max_tokens for summary
    let summary_config = ModelConfig {
        max_tokens: Some(max_tokens),
        temperature: Some(0.3), // Lower temperature for more consistent output
        ..model_config.clone()
    };

    // Call API
    let mut stream = provider
        .stream(&summary_messages, &[], &summary_config)
        .await?;

    // Collect response
    let mut summary = String::with_capacity(max_tokens as usize * 4); // Rough estimate of chars per token
    while let Some(item) = stream.try_next().await? {
        match item {
            ModelStreamItem::Chunk(crate::event::ContentChunk::Text(text)) => {
                summary.push_str(&text);
            }
            ModelStreamItem::Complete => break,
            _ => {}
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::MessageTokenUsage;

    #[test]
    fn test_calculate_tokens_with_usage() {
        let messages = vec![Message::user("Hello"), Message::assistant("Hi there"), {
            let mut msg = Message::assistant("Let me help");
            msg.token_usage = Some(MessageTokenUsage {
                prompt_tokens: 100,
                completion_tokens: 50,
                total_tokens: 150,
            });
            msg
        }];

        let tokens = Compactor::calculate_tokens(&messages);
        // Should use the actual usage (150) plus estimation for messages after
        assert!(tokens >= 150);
    }

    #[test]
    fn test_micro_compact() {
        let compactor = Compactor::new(100, 200, 2, 1000); // keep last 2 messages
        let mut messages = vec![
            Message::user("Task 1"),
            Message::tool_result("call-1", "Result 1"), // will be cleared (index 1)
            Message::user("Task 2"),
            Message::tool_result("call-2", "Result 2"), // kept (index 3, in keep_recent)
            Message::user("Current task"),              // kept (index 4)
        ];

        let compacted = compactor.micro_compact(&mut messages);
        assert!(compacted);
        // Old tool result should be cleared
        assert_eq!(
            messages[1].text_content(),
            "[Old tool result content cleared]"
        );
        // Recent tool result should be preserved (keep_recent = 2)
        assert_eq!(messages[3].text_content(), "Result 2");
        assert_eq!(messages[4].text_content(), "Current task");

        // Second compaction should return false (already cleared)
        let compacted_again = compactor.micro_compact(&mut messages);
        assert!(!compacted_again);
    }
}
