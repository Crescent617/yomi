//! Token counting utilities
//!
//! Estimation strategy:
//! - 1 token ≈ 4 characters (for all text)
//! - JSON is denser: 1 token ≈ 2 characters

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
