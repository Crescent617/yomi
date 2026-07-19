//! Token counting utilities
//!
//! Estimation strategy:
//! - 1 token ≈ 4 characters (for all text)
//! - JSON is denser: 1 token ≈ 2 characters

use crate::types::{ContentBlock, Message, Role, ToolDefinition};
use std::sync::Arc;

/// Fixed per-image estimate matching Claude Code's rough token counting.
/// Providers tokenize decoded image dimensions, not the URL or base64 byte length.
const IMAGE_TOKEN_ESTIMATE: u32 = 2_000;

/// Estimate tokens from UTF-8 byte length, rounding up conservatively.
/// Rough approximation: 1 token ≈ 4 bytes.
///
/// # Examples
/// ```
/// use kernel::utils::tokens::estimate_tokens;
///
/// assert_eq!(estimate_tokens("hello world"), 3); // ceil(11 / 4)
/// assert_eq!(estimate_tokens("你好世界"), 3);     // ceil(12 / 4)
/// ```
pub const fn estimate_tokens(text: &str) -> usize {
    text.len().div_ceil(4)
}

/// Estimate tokens for JSON content, rounding up conservatively.
/// JSON is denser due to punctuation, so it uses approximately 2 bytes/token.
pub const fn estimate_tokens_for_json(text: &str) -> usize {
    text.len().div_ceil(2)
}

fn estimate_message_tokens(message: &Message) -> u32 {
    let content_tokens = message.content.iter().fold(0u32, |total, block| {
        let tokens = match block {
            ContentBlock::Text { text } => estimate_tokens(text) as u32,
            ContentBlock::Thinking {
                thinking,
                signature,
            } => (estimate_tokens(thinking) as u32).saturating_add(
                signature
                    .as_deref()
                    .map_or(0, |text| estimate_tokens(text) as u32),
            ),
            ContentBlock::RedactedThinking { data } => estimate_tokens(data) as u32,
            ContentBlock::ImageUrl { .. } => IMAGE_TOKEN_ESTIMATE,
            // No current provider serializes Audio blocks; do not budget content
            // that is omitted from the actual request.
            ContentBlock::Audio { .. } => 0,
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
                .saturating_add(estimate_tokens(&call.id) as u32)
                .saturating_add(estimate_tokens(&call.name) as u32)
                .saturating_add(estimate_tokens_for_json(&call.arguments.to_string()) as u32)
                .saturating_add(8)
        });

    content_tokens
        .saturating_add(tool_call_tokens)
        .saturating_add(
            message
                .tool_call_id
                .as_deref()
                .map_or(0, |text| estimate_tokens(text) as u32),
        )
        .saturating_add(10)
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

/// Estimate the input tokens for a request using only messages providers serialize.
pub fn estimate_request_input_tokens(
    messages: &[Arc<Message>],
    tools: &[Arc<ToolDefinition>],
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

/// Estimate tokens as f64 for accurate accumulation
#[allow(clippy::cast_precision_loss)]
pub fn estimate_tokens_f64(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    text.len() as f64 / 4.0
}

/// Estimate tokens for a collection of messages (extracts text content only)
///
/// Note: This only counts text content. Non-text content like images,
/// tool calls, and thinking blocks are not included in the estimation.
pub fn estimate_tokens_for_messages(messages: &[crate::types::Message]) -> u32 {
    messages.iter().fold(0u32, |total, message| {
        total.saturating_add(estimate_tokens(&message.text_content()) as u32)
    })
}

/// Format estimated token count with ~ prefix to indicate estimation
#[allow(clippy::cast_precision_loss)]
pub fn format_estimated_tokens(count: usize) -> String {
    if count >= 1000 {
        format!("~{:.1}k", count as f64 / 1000.0)
    } else {
        format!("~{count}")
    }
}

/// Format estimated f64 token count with ~ prefix (display as integer)
pub fn format_estimated_tokens_f64(count: f64) -> String {
    let count_rounded = count.round();
    if count_rounded >= 1000.0 {
        format!("~{:.1}k", count_rounded / 1000.0)
    } else {
        format!("~{count_rounded:.0}")
    }
}

/// Format actual token count from API for display (no ~ prefix)
#[allow(clippy::cast_precision_loss)]
pub fn format_actual_tokens(count: u32) -> String {
    if count >= 1000 {
        format!("{:.1}k", f64::from(count) / 1000.0)
    } else {
        count.to_string()
    }
}

#[cfg(test)]
#[path = "tokens_test.rs"]
mod tests;
