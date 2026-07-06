//! Token counting utilities
//!
//! Estimation strategy:
//! - 1 token ≈ 4 characters (for all text)
//! - JSON is denser: 1 token ≈ 2 characters

/// Estimate tokens from text length
/// Rough approximation: 1 token ≈ 4 characters
///
/// # Examples
/// ```
/// use kernel::utils::tokens::estimate_tokens;
///
/// assert_eq!(estimate_tokens("hello world"), 2);  // 11 / 4 = 2
/// assert_eq!(estimate_tokens("你好世界"), 3);      // 12 / 4 = 3
/// ```
pub const fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.len() / 4
}

/// Estimate tokens as f64 for accurate accumulation
#[allow(clippy::cast_precision_loss)]
pub fn estimate_tokens_f64(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    text.len() as f64 / 4.0
}

/// Estimate tokens for JSON content
/// JSON is denser (more single-char tokens like `{`, `}`, `:`, `,`)
/// Uses 2 chars/token instead of 4
pub const fn estimate_tokens_for_json(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    text.len() / 2
}

/// Estimate tokens for a collection of messages (extracts text content only)
///
/// Note: This only counts text content. Non-text content like images,
/// tool calls, and thinking blocks are not included in the estimation.
pub fn estimate_tokens_for_messages(messages: &[crate::types::Message]) -> u32 {
    let total_chars: usize = messages.iter().map(|m| m.text_content().len()).sum();
    total_chars as u32 / 4
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
