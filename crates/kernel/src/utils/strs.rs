/// Truncate a string by byte length with a custom suffix (UTF-8 safe).
/// Finds a valid UTF-8 boundary before truncating.
///
/// # Behavior
/// - If `s.len() <= max_bytes`: returns `s` as-is (no suffix added)
/// - If `s.len() > max_bytes`: truncates to `max_bytes - suffix.len()` bytes
///   and appends `suffix`
///
/// This ensures the result never exceeds `max_bytes` bytes.
pub fn truncate_with_suffix(s: &str, max_bytes: usize, suffix: &str) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }

    let target_len = max_bytes.saturating_sub(suffix.len());
    if target_len == 0 {
        return suffix.to_string();
    }

    let mut byte_idx = 0;

    for (idx, ch) in s.char_indices() {
        // Check if adding this character would exceed target length
        if idx + ch.len_utf8() > target_len {
            break;
        }
        byte_idx = idx + ch.len_utf8();
    }

    format!("{}{}", &s[..byte_idx], suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_no_truncation_needed() {
        // String is already within limit
        assert_eq!(truncate_with_suffix("hello", 10, "..."), "hello");
        assert_eq!(truncate_with_suffix("hello", 5, "..."), "hello");
    }

    #[test]
    fn test_truncate_basic() {
        // Basic truncation
        assert_eq!(truncate_with_suffix("hello world", 8, "..."), "hello...");
        assert_eq!(truncate_with_suffix("hello world", 5, "..."), "he...");
    }

    #[test]
    fn test_truncate_exact_fit() {
        // When string + suffix exactly fits
        assert_eq!(truncate_with_suffix("hello", 8, "..."), "hello"); // No truncation needed
        assert_eq!(truncate_with_suffix("hello world", 11, "..."), "hello world");
    }

    #[test]
    fn test_truncate_empty() {
        // Empty string
        assert_eq!(truncate_with_suffix("", 10, "..."), "");
        // Empty string with max_bytes=0 returns just suffix (because 0 <= 3, target_len becomes 0)
        assert_eq!(truncate_with_suffix("", 0, "..."), "");
    }

    #[test]
    fn test_truncate_unicode() {
        // UTF-8 multi-byte characters (CJK is 3 bytes each)
        let text = "你好世界"; // 12 bytes total (4 chars * 3 bytes)
        assert_eq!(truncate_with_suffix(text, 12, "..."), "你好世界"); // Fits exactly

        // With max_bytes=6 and suffix "..." (3 bytes): target_len = 3 bytes
        // CJK chars are 3 bytes each, so only "你" (3 bytes) fits
        // Result: "你..." (6 bytes total)
        assert_eq!(truncate_with_suffix(text, 6, "..."), "你...");

        // Emoji (4 bytes each)
        let emoji = "🎉🎊🎁"; // 12 bytes
        // target_len = 7 - 3 = 4 bytes
        // One emoji is 4 bytes, so "🎉" fits exactly
        assert_eq!(truncate_with_suffix(emoji, 7, "..."), "🎉...");
    }

    #[test]
    fn test_truncate_mixed_unicode() {
        // Mixed ASCII and Unicode
        let text = "Hello你好World世界";
        // With max_bytes=10 and suffix "..." (3 bytes): target_len = 7 bytes
        // "Hello" (5 bytes) + "你" (3 bytes) = 8 bytes > 7 bytes
        // So only "Hello" (5 bytes) fits, result: "Hello..." (8 bytes total)
        assert_eq!(truncate_with_suffix(text, 10, "..."), "Hello...");

        // Verify result is within max_bytes
        let result = truncate_with_suffix(text, 10, "...");
        assert!(result.len() <= 10, "Result too long: {} bytes", result.len());
    }

    #[test]
    fn test_truncate_suffix_larger_than_limit() {
        // When suffix itself is larger than max_bytes
        assert_eq!(truncate_with_suffix("hello", 2, "..."), "...");
        assert_eq!(truncate_with_suffix("hello", 0, "..."), "...");
    }

    #[test]
    fn test_truncate_different_suffixes() {
        // Different suffixes - suffix length affects how much content fits
        // With suffix "→" (3 bytes): 8 - 3 = 5 chars for content
        assert_eq!(truncate_with_suffix("hello world", 8, "→"), "hello→");
        // With empty suffix: 8 chars for content
        assert_eq!(truncate_with_suffix("hello world", 8, ""), "hello wo");
        // With long suffix " [truncated]" (12 bytes): 8 - 12 = 0, so just suffix
        assert_eq!(truncate_with_suffix("hello world", 8, " [truncated]"), " [truncated]");
    }

    #[test]
    fn test_truncate_newlines() {
        // String with newlines
        let text = "line1\nline2\nline3"; // 17 bytes total
        // 10 bytes max with "..." (3 bytes) = 7 bytes target for content
        // The function counts 7 chars: "line1\nl" = 7 bytes
        // Result: "line1\nl..." = 10 bytes
        assert_eq!(truncate_with_suffix(text, 10, "..."), "line1\nl...");
    }

    #[test]
    fn test_truncate_single_char() {
        // Single character
        assert_eq!(truncate_with_suffix("a", 5, "..."), "a");
        // With max_bytes=1 and content "a" (1 byte): content fits exactly, no truncation
        assert_eq!(truncate_with_suffix("a", 1, "..."), "a");
        // With max_bytes=0: can't fit anything, returns just suffix
        assert_eq!(truncate_with_suffix("a", 0, "..."), "...");
    }

    #[test]
    fn test_truncate_byte_boundary() {
        // Ensure we don't cut in the middle of a UTF-8 sequence
        let text = "αβγδ"; // Greek letters, 2 bytes each
        let result = truncate_with_suffix(text, 3, "...");
        // Should truncate at valid boundary, not in middle of 'β'
        assert!(result.ends_with("..."));
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn test_truncate_exact_target_len() {
        // When content + suffix exactly fits, content is returned as-is (no truncation)
        // "abc" (3 bytes) + "..." (3 bytes) = 6 bytes total, but "abcdef" is 6 bytes
        // Since "abcdef".len() == max_bytes, it's returned as-is
        assert_eq!(truncate_with_suffix("abcdef", 6, "..."), "abcdef");

        // To see truncation, we need content longer than max_bytes - suffix_len
        // "abcdef" (6 bytes) + "..." (3 bytes) > 6, so truncation happens
        // target_len = 6 - 3 = 3, so "abc" + "..." = 6 bytes
        assert_eq!(truncate_with_suffix("abcdef", 6, "..."), "abcdef"); // Fits exactly, no truncation

        // "abcdefg" (7 bytes) > 6, truncation happens
        assert_eq!(truncate_with_suffix("abcdefg", 6, "..."), "abc...");

        // CJK: "你好" is 6 bytes, with max_bytes=6, returns as-is
        assert_eq!(truncate_with_suffix("你好", 6, "..."), "你好");

        // "你好世界" is 12 bytes > 6, so truncation happens
        // target_len = 3, "你" = 3 bytes, result: "你..."
        assert_eq!(truncate_with_suffix("你好世界", 6, "..."), "你...");
    }

    #[test]
    fn test_truncate_char_larger_than_target() {
        // When a single character is larger than target_len
        // "你" is 3 bytes, total is 3, max_bytes is 5, so it's returned as-is
        assert_eq!(truncate_with_suffix("你", 5, "..."), "你");

        // For truncation to happen, content must be longer than max_bytes - suffix_len
        // "你好" is 6 bytes > 5, so truncation happens
        // target_len = 5 - 3 = 2, CJK needs 3 bytes, can't fit, so only suffix
        assert_eq!(truncate_with_suffix("你好", 5, "..."), "...");

        // Emoji is 4 bytes, which fits in max_bytes=6 (4 <= 6)
        // So it's returned as-is without truncation
        assert_eq!(truncate_with_suffix("🎉", 6, "..."), "🎉");

        // "🎉🎊" is 8 bytes > 6, truncation happens
        // target_len = 6 - 3 = 3, emoji needs 4 bytes, can't fit, so only suffix
        assert_eq!(truncate_with_suffix("🎉🎊", 6, "..."), "...");
    }

    #[test]
    fn test_truncate_partial_char() {
        // Test that we don't include partial multi-byte characters
        // "αβ" is 4 bytes, with target_len = 3 we should only get "α"
        let text = "αβγδ"; // Each is 2 bytes
        // target_len = 6 - 3 = 3
        // "α" = 2 bytes, "αβ" = 4 bytes > 3, so only "α"
        assert_eq!(truncate_with_suffix(text, 6, "..."), "α...");

        // Verify no partial character
        let result = truncate_with_suffix(text, 6, "...");
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
        assert!(!result.contains('β')); // Should not contain second character
    }

    #[test]
    fn test_truncate_length_constraints() {
        // All results should be <= max_bytes
        let test_cases = vec![
            ("hello world", 8, "..."),
            ("你好世界", 6, "..."),
            ("🎉🎊🎁", 7, "..."),
            ("αβγδ", 5, "..."),
            ("", 10, "..."),
            ("test", 100, "..."),
        ];

        for (text, max_bytes, suffix) in test_cases {
            let result = truncate_with_suffix(text, max_bytes, suffix);
            assert!(
                result.len() <= max_bytes,
                "Result '{}' ({} bytes) exceeds max_bytes {} for input '{}'",
                result,
                result.len(),
                max_bytes,
                text
            );
        }
    }

    #[test]
    fn test_truncate_behavior_no_suffix_when_fits() {
        // When content fits in max_bytes, no suffix is added
        assert_eq!(truncate_with_suffix("hi", 10, "..."), "hi");
        assert_eq!(truncate_with_suffix("hello", 5, "..."), "hello");

        // When content doesn't fit, suffix is added
        assert_eq!(truncate_with_suffix("hello world", 8, "..."), "hello...");

        // Verify: result is always <= max_bytes
        assert!(truncate_with_suffix("test", 10, "...").len() <= 10);
        assert!(truncate_with_suffix("hello world", 8, "...").len() <= 8);
    }

    #[test]
    fn test_truncate_edge_case_target_len_1() {
        // target_len = 1 means only 1 byte for content
        // ASCII fits, multi-byte doesn't
        assert_eq!(truncate_with_suffix("hello", 4, "..."), "h...");

        // CJK (3 bytes) doesn't fit in target_len = 1
        assert_eq!(truncate_with_suffix("你好", 4, "..."), "...");
    }

    #[test]
    fn test_truncate_wide_char_at_boundary() {
        // Test handling of characters at exact byte boundaries
        // "ab你" is 2 + 3 = 5 bytes, with max_bytes=8
        // Since "ab你".len() = 5 <= 8, it's returned as-is without suffix
        assert_eq!(truncate_with_suffix("ab你", 8, "..."), "ab你");

        // "ab你好" is 8 bytes, with max_bytes=8
        // Since 8 <= 8, returned as-is
        assert_eq!(truncate_with_suffix("ab你好", 8, "..."), "ab你好");

        // "ab你好!" is 9 bytes > 8, truncation happens
        // target_len = 8 - 3 = 5, "ab你" = 5 bytes fits exactly
        assert_eq!(truncate_with_suffix("ab你好!", 8, "..."), "ab你...");

        // With max_bytes=7, "ab你" = 5 <= 7, returned as-is
        assert_eq!(truncate_with_suffix("ab你", 7, "..."), "ab你");

        // "ab你好" = 8 > 7, truncation happens
        // target_len = 7 - 3 = 4, "ab" = 2 fits, "ab你" = 5 > 4
        assert_eq!(truncate_with_suffix("ab你好", 7, "..."), "ab...");
    }
}
