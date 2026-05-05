//! Text truncation utilities for tool outputs
//!
//! Provides UTF-8 safe text truncation with customizable suffix messages.

use crate::utils::strs;

/// Default truncation message
pub const TRUNCATION_MESSAGE: &str = "\n\n[Output truncated due to limit]";

/// Truncate text if it exceeds max length, adding a notice with the line number.
/// Used by tools that handle their own truncation (like read tool).
pub fn maybe_truncate_output(text: String, max_len: usize, offset: usize) -> String {
    if text.len() <= max_len {
        return text;
    }

    // Truncate at a safe UTF-8 boundary near the limit
    let truncate_at = find_utf8_boundary(&text, max_len);
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

/// Find a valid UTF-8 boundary at or before the target byte position.
fn find_utf8_boundary(text: &str, target: usize) -> usize {
    text.char_indices()
        .rev()
        .find(|&(i, _)| i <= target)
        .map_or(0, |(i, _)| i)
}

/// Truncate output if it exceeds max length (UTF-8 safe)
/// Uses the strs utility for consistent truncation.
pub fn truncate_output(text: &str, max_len: usize, suffix: &str) -> String {
    strs::truncate_with_suffix(text, max_len, suffix)
}

/// Truncate output with the default truncation message.
pub fn truncate_with_message(text: &str, max_len: usize) -> String {
    truncate_output(text, max_len, TRUNCATION_MESSAGE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_output_no_truncation_needed() {
        let text = "short text";
        let result = truncate_with_message(text, 100);
        assert_eq!(result, "short text");
    }

    #[test]
    fn test_truncate_output_truncate() {
        let text = "a".repeat(1000);
        let result = truncate_with_message(&text, 100);
        assert!(result.len() <= 100 + TRUNCATION_MESSAGE.len());
        assert!(result.ends_with(TRUNCATION_MESSAGE));
    }

    #[test]
    fn test_maybe_truncate_output_with_offset() {
        let text = "line1\nline2\nline3".to_string();
        // Set max_len to smaller than text length (17 chars) to trigger truncation
        let result = maybe_truncate_output(text.clone(), 10, 1);

        // Should include truncation notice with line number
        assert!(result.contains("Content truncated at line"));
        assert!(result.contains("Use offset/limit to read more"));
    }

    #[test]
    fn test_maybe_truncate_output_no_truncation() {
        let text = "short".to_string();
        let result = maybe_truncate_output(text.clone(), 100, 1);
        assert_eq!(result, "short");
    }

    #[test]
    fn test_find_utf8_boundary() {
        let text = "Hello, 世界!";
        // "世界" is 6 bytes total (3 bytes each)
        let boundary = find_utf8_boundary(text, 9);
        // Should find a valid UTF-8 boundary
        assert!(text.is_char_boundary(boundary));
    }
}
