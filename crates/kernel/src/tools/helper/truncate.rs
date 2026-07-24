//! Text truncation utilities for tool outputs
//!
//! Provides UTF-8 safe text truncation with customizable suffix messages.

use crate::utils::strs;

/// Default maximum tool output length shared across tools (40 KB)
pub const DEFAULT_MAX_TOOL_OUTPUT_LENGTH: usize = 40_000;

/// Default truncation message
pub const TRUNCATION_MESSAGE: &str = "\n\n[Output truncated due to limit]";

/// Truncate text if it exceeds max length, adding a notice with the line number.
/// Used by tools that handle their own truncation (like read tool).
pub fn maybe_truncate_output(text: String, max_len: usize, offset: usize) -> String {
    if text.len() <= max_len {
        return text;
    }

    // Truncate at a safe UTF-8 boundary near the limit
    let truncate_at = strs::floor_char_boundary(&text, max_len);
    let mut result = text;
    result.truncate(truncate_at);

    // Calculate line number at truncation point
    let lines_count = result.lines().count();
    let truncation_line = offset + lines_count.saturating_sub(1);

    let notice = format!(
        "\n\n[Content truncated at line {truncation_line}. Use offset/limit to read more.]"
    );
    result.push_str(&notice);
    result
}

/// Truncate output if it exceeds max length (UTF-8 safe)
/// Uses the strs utility for consistent truncation.
pub fn truncate_output(text: &str, max_len: usize, suffix: &str) -> String {
    strs::truncate_with_suffix(text, max_len, suffix)
}

/// Truncate text keeping both head and tail, joined by a separator.
/// Useful for shell output where you want to see both beginning and end.
pub fn truncate_keep_edges(text: &str, max_len: usize, sep: &str) -> String {
    strs::truncate_keep_edges(text, max_len, sep)
}

/// Truncate output with the default truncation message.
pub fn truncate_with_message(text: &str, max_len: usize) -> String {
    truncate_output(text, max_len, TRUNCATION_MESSAGE)
}

#[cfg(test)]
#[path = "truncate_test.rs"]
mod tests;
