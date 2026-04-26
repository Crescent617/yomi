//! Text preprocessing utilities for TUI rendering

/// Preprocess text for display by:
/// - Converting tabs to 2 spaces for consistent width
pub fn preprocess(text: impl AsRef<str>) -> String {
    text.as_ref().replace('\t', "  ")
}

/// Get byte index from character index (Unicode-safe)
/// Returns the byte position corresponding to the `char_idx`-th character
pub fn char_idx_to_byte_idx(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(byte_idx, _)| byte_idx)
}

/// Extract substring by character indices (Unicode-safe)
/// Returns the substring from `start_char` to `end_char` (in characters, not bytes)
pub fn substring_by_chars(text: &str, start_char: usize, end_char: usize) -> String {
    text.chars()
        .skip(start_char)
        .take(end_char.saturating_sub(start_char))
        .collect()
}

/// Truncate text to max character count (Unicode-safe)
/// Returns the truncated string with "..." suffix if truncated
pub fn truncate_by_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        text.to_string()
    } else if max_chars <= 3 {
        // If max is 3 or less, just return "..."
        "...".to_string()
    } else {
        let truncated: String = text.chars().take(max_chars - 3).collect();
        format!("{truncated}...")
    }
}

/// Truncate text by display width (accounts for CJK characters being 2 columns).
/// Returns the truncated string with suffix appended if truncated.
///
/// # Arguments
/// * `text` - The input string
/// * `max_width` - Maximum display width in columns
/// * `suffix` - Suffix to append when truncated (e.g., "...")
///
/// # Behavior
/// - If `text` display width <= `max_width`: returns `text` as-is (no suffix)
/// - If `max_width <= suffix width`: returns truncated suffix
/// - Otherwise: truncates to fit `text + suffix` within `max_width`
pub fn truncate_by_width(text: &str, max_width: usize, suffix: &str) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    let text_width = text.width_cjk();
    let suffix_width = suffix.width_cjk();

    if text_width <= max_width {
        return text.to_string();
    }

    if max_width <= suffix_width {
        // Not enough space for suffix, truncate suffix itself
        let mut result = String::new();
        let mut current_width = 0;
        for ch in suffix.chars() {
            let ch_width = ch.width_cjk().unwrap_or(0);
            if current_width + ch_width > max_width {
                break;
            }
            result.push(ch);
            current_width += ch_width;
        }
        return result;
    }

    // Build truncated text to fit within max_width - suffix_width
    let target_width = max_width - suffix_width;
    let mut result = String::new();
    let mut current_width = 0;

    for ch in text.chars() {
        let ch_width = ch.width_cjk().unwrap_or(0);
        if current_width + ch_width > target_width {
            break;
        }
        result.push(ch);
        current_width += ch_width;
    }

    result.push_str(suffix);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_unicode_ascii() {
        assert_eq!(truncate_unicode("hello world", 20), "hello world");
        assert_eq!(truncate_unicode("hello world", 8), "hello...");
        assert_eq!(truncate_unicode("hello", 5), "hello");
        assert_eq!(truncate_unicode("hello", 4), "h...");
        assert_eq!(truncate_unicode("hello", 3), "...");
    }

    #[test]
    fn test_truncate_unicode_multibyte() {
        // Chinese characters (11 chars: 这是一个很长的中文句子)
        let chinese = "这是一个很长的中文句子";
        assert_eq!(truncate_unicode(chinese, 20), chinese);
        assert_eq!(truncate_unicode(chinese, 11), chinese);
        assert_eq!(truncate_unicode(chinese, 10), "这是一个很长的...");

        // Emoji (10 chars total, each emoji is 1 char though multiple bytes in UTF-8)
        let emoji = "🎉🎊🎁🎄🎃🎅🤶🧑‍🎄";
        assert_eq!(truncate_unicode(emoji, 10), emoji);
        assert_eq!(truncate_unicode(emoji, 5), "🎉🎊..."); // 5-3=2 chars preserved

        // Mixed (12 chars: Hello世界🎉Test)
        let mixed = "Hello世界🎉Test";
        assert_eq!(truncate_unicode(mixed, 20), mixed);
        assert_eq!(truncate_unicode(mixed, 10), "Hello世界..."); // 10-3=7 chars preserved
    }

    #[test]
    fn test_truncate_unicode_edge_cases() {
        assert_eq!(truncate_unicode("", 10), "");
        assert_eq!(truncate_unicode("ab", 3), "ab");
        assert_eq!(truncate_unicode("abc", 3), "abc");
        assert_eq!(truncate_unicode("abcd", 3), "...");
    }

    #[test]
    fn test_truncate_by_width_ascii() {
        // No truncation needed
        assert_eq!(truncate_by_width("hello", 10, "..."), "hello");
        // Truncation with suffix
        assert_eq!(truncate_by_width("hello world", 8, "..."), "hello...");
        // Exact fit
        assert_eq!(truncate_by_width("hello...", 8, "..."), "hello...");
    }

    #[test]
    fn test_truncate_by_width_cjk() {
        // CJK chars are 2 columns wide
        let chinese = "你好世界"; // 4 chars, 8 columns
        assert_eq!(truncate_by_width(chinese, 10, "..."), chinese);
        // Need to truncate: width=8, text_width=8, fits exactly, no truncation
        assert_eq!(truncate_by_width(chinese, 8, "..."), chinese);
        // target = 7 - 3 = 4, "你"=2, "你好"=4 fits exactly
        assert_eq!(truncate_by_width(chinese, 7, "..."), "你好...");
        // Very narrow
        assert_eq!(truncate_by_width(chinese, 3, "..."), "...");
        assert_eq!(truncate_by_width(chinese, 2, ".."), "..");
        assert_eq!(truncate_by_width(chinese, 1, "..."), ".");
    }

    #[test]
    fn test_truncate_by_width_mixed() {
        // Mixed ASCII and CJK
        let mixed = "Hello世界"; // 5 + 4 = 9 columns
        assert_eq!(truncate_by_width(mixed, 10, "..."), mixed);
        // width=9, text_width=9, fits exactly
        assert_eq!(truncate_by_width(mixed, 9, "..."), mixed);
        // target = 8 - 3 = 5, "Hello"=5 fits exactly
        assert_eq!(truncate_by_width(mixed, 8, "..."), "Hello...");
    }

    #[test]
    fn test_truncate_by_width_emoji() {
        // Emoji are typically 2 columns wide
        let emoji = "🎉🎊🎁"; // 3 chars, 6 columns
        assert_eq!(truncate_by_width(emoji, 8, "..."), emoji);
        // width=6, text_width=6, fits exactly
        assert_eq!(truncate_by_width(emoji, 6, "..."), emoji);
        // target = 5 - 3 = 2, "🎉"=2 fits exactly
        assert_eq!(truncate_by_width(emoji, 5, "..."), "🎉...");
    }

    #[test]
    fn test_truncate_by_width_edge_cases() {
        assert_eq!(truncate_by_width("", 10, "..."), "");
        // Empty suffix
        assert_eq!(truncate_by_width("hello", 3, ""), "hel");
        // Suffix longer than max_width
        assert_eq!(truncate_by_width("hello", 2, "..."), "..");
    }
}
