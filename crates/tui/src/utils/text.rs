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
        .map(|(byte_idx, _)| byte_idx)
        .unwrap_or(text.len())
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
pub fn truncate_unicode(text: &str, max_chars: usize) -> String {
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
}
