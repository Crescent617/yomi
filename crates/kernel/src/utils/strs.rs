/// Truncate a string by character count with a custom suffix.
///
/// # Behavior
/// - If char count <= `max_chars`: returns `s` as-is (no suffix added)
/// - If char count > `max_chars`: truncates to `max_chars` chars and appends suffix
///
/// This ensures the result never exceeds `max_chars` characters (plus suffix).
pub fn truncate_by_chars(s: &str, max_chars: usize, suffix: &str) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        return s.to_string();
    }

    let mut result = String::with_capacity(max_chars + suffix.len());
    for (i, ch) in s.chars().enumerate() {
        if i >= max_chars {
            break;
        }
        result.push(ch);
    }
    result.push_str(suffix);
    result
}

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

/// Truncate a string by keeping head and tail, omitting the middle.
///
/// # Behavior
/// - If `s.len() <= max_bytes`: returns `s` as-is (no allocation)
/// - If `s.len() > max_bytes`: keeps the first ~`max_bytes/2` bytes and the
///   last ~`max_bytes/2` bytes, joined by `sep`
///
/// This is UTF-8 safe: it never splits a multi-byte character.
pub fn truncate_keep_edges(s: &str, max_bytes: usize, sep: &str) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }

    if max_bytes <= sep.len() {
        // Can't fit both content and separator, return truncated separator
        return truncate_with_suffix(sep, max_bytes, "");
    }

    let content_budget = max_bytes.saturating_sub(sep.len());
    let head_budget = content_budget / 2;
    let tail_budget = content_budget - head_budget;

    // Find head boundary (valid UTF-8)
    let mut head = 0;
    for (i, c) in s.char_indices() {
        if i + c.len_utf8() > head_budget {
            break;
        }
        head = i + c.len_utf8();
    }

    // Find tail start boundary (valid UTF-8)
    // Scan from the end backwards, expanding the tail window as long as it fits
    let mut tail_start = s.len();
    for (i, _) in s.char_indices().rev() {
        if s.len() - i <= tail_budget {
            tail_start = i;
        } else {
            break;
        }
    }

    format!("{}{}{}", &s[..head], sep, &s[tail_start..])
}

#[macro_export]
macro_rules! const_concat {
    ($a:expr $(,)?) => {
        $a
    };

    ($($args:expr),+ $(,)?) => {{
        // 1️⃣ 编译期计算总长度
        const LEN: usize = 0 $(+ $args.len())+;

        // 2️⃣ 构造 buffer
        const BYTES: [u8; LEN] = {
            let mut out = [0u8; LEN];
            let mut offset = 0;

            $(
                {
                    let (new_out, new_offset) = $crate::utils::strs::push_str(out, offset, $args);
                    out = new_out;
                    offset = new_offset;
                }
            )+

            // Silence unused_assignments warning for the final offset update
            let _ = offset;

            out
        };
        unsafe { std::str::from_utf8_unchecked(&BYTES) }
    }};
}

pub const fn push_str<const N: usize>(
    mut out: [u8; N],
    offset: usize,
    s: &str,
) -> ([u8; N], usize) {
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut off = offset;

    while i < bytes.len() {
        out[off] = bytes[i];
        off += 1;
        i += 1;
    }

    (out, off)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_keep_edges_no_truncation() {
        assert_eq!(truncate_keep_edges("hello", 10, "..."), "hello");
        assert_eq!(truncate_keep_edges("hello", 5, "..."), "hello");
    }

    #[test]
    fn test_truncate_keep_edges_basic() {
        let text = "0123456789abcdefghij"; // 20 chars
        let result = truncate_keep_edges(text, 10, "|");
        // content_budget = 9, head_budget = 4, tail_budget = 5
        // head = "0123", tail = "fghij" (starts at byte 15)
        assert_eq!(result, "0123|fghij");
        assert!(result.len() <= 10);
    }

    #[test]
    fn test_truncate_keep_edges_uneven_budget() {
        let text = "0123456789ABCDEF"; // 16 chars
        let result = truncate_keep_edges(text, 9, "|");
        // content_budget = 8, head_budget = 4, tail_budget = 4
        assert_eq!(result, "0123|CDEF");
        assert!(result.len() <= 9);
    }

    #[test]
    fn test_truncate_keep_edges_boundary() {
        // max_bytes exactly equals separator length
        assert_eq!(truncate_keep_edges("hello", 3, "..."), "...");
        // max_bytes smaller than separator
        assert_eq!(truncate_keep_edges("hello", 2, "..."), "..");
    }

    #[test]
    fn test_truncate_keep_edges_unicode() {
        // CJK: 3 bytes each
        let text = "你好世界欢迎"; // 18 bytes, 6 chars
        let result = truncate_keep_edges(text, 10, "|");
        // head_budget = 4 bytes -> "你" (3 bytes) fits, "你" + "好" = 6 > 4, so head = "你"
        // tail_budget = 5 bytes -> "迎" (3 bytes) fits, "迎" + "欢" = 6 > 5, so tail = "迎"
        assert_eq!(result, "你|迎");
        assert!(result.len() <= 10);
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn test_truncate_keep_edges_emoji() {
        // Emoji: 4 bytes each
        let text = "🎉🎊🎁🎂🎃"; // 20 bytes
        let result = truncate_keep_edges(text, 10, "|");
        // head_budget = 4 bytes -> "🎉" fits exactly (4 bytes)
        // tail_budget = 5 bytes -> "🎃" fits (4 bytes)
        assert_eq!(result, "🎉|🎃");
        assert!(result.len() <= 10);
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    }

    #[test]
    fn test_truncate_keep_edges_mixed_unicode() {
        let text = "Hello世界World宇宙"; // "Hello" (5) + "世界" (6) + "World" (5) + "宇宙" (6) = 22 bytes
        let result = truncate_keep_edges(text, 12, "...");
        // sep = "..." (3 bytes), content_budget = 9, head_budget = 4, tail_budget = 5
        // head: "Hell" = 4 bytes fits, "Hello" = 5 > 4, so head = "Hell"
        // tail: start at byte >= 22-5=17, "宙" starts at 19, so tail = "宙" (3 bytes)
        assert_eq!(result, "Hell...宙");
        assert!(result.len() <= 12);
    }

    #[test]
    fn test_truncate_keep_edges_length_constraints() {
        let test_cases = vec![
            ("hello world", 8, "|"),
            ("你好世界", 6, "|"),
            ("🎉🎊🎁", 7, "|"),
            ("αβγδ", 5, "|"),
            ("", 10, "|"),
            ("test", 100, "|"),
        ];

        for (text, max_bytes, sep) in test_cases {
            let result = truncate_keep_edges(text, max_bytes, sep);
            assert!(
                result.len() <= max_bytes,
                "Result '{}' ({} bytes) exceeds max_bytes {} for input '{}'",
                result,
                result.len(),
                max_bytes,
                text
            );
            assert!(std::str::from_utf8(result.as_bytes()).is_ok());
        }
    }

    #[test]
    fn test_truncate_keep_edges_tail_with_wide_char() {
        // Bug scenario: wide char at end with small tail_budget.
        // "abcdefg🎉" = 11 bytes; max=10, sep="..." (3) -> content=7, head=3, tail=4
        // Fixed scan should keep the 4-byte emoji tail.
        let result = truncate_keep_edges("abcdefg🎉", 10, "...");
        assert_eq!(result, "abc...🎉");
        assert!(result.len() <= 10);

        // Same string but max=8: tail_budget=3, emoji (4 bytes) doesn't fit -> tail dropped
        let result = truncate_keep_edges("abcdefg🎉", 8, "...");
        assert_eq!(result, "ab...");
        assert!(result.len() <= 8);

        // Multiple trailing wide chars: "Hello🎉🎊" = 13 bytes
        let result = truncate_keep_edges("Hello🎉🎊", 10, "...");
        // content=7, head=3, tail=4. tail should fit exactly one emoji
        assert_eq!(result, "Hel...🎊");
        assert!(result.len() <= 10);

        // CJK trailing: "abcdefg你好" = 13 bytes. max=10, sep="..." (3)
        // content=7, head=3, tail=4. tail should fit one CJK char (3 bytes)
        let result = truncate_keep_edges("abcdefg你好", 10, "...");
        assert_eq!(result, "abc...好");
        assert!(result.len() <= 10);
    }

    #[test]
    fn test_truncate_keep_edges_exact_fit() {
        // When content exactly fits max_bytes
        assert_eq!(truncate_keep_edges("hello", 5, "..."), "hello");
        // When content + sep exactly fits... but content is exactly max_bytes, so no truncation
        assert_eq!(truncate_keep_edges("hello", 5, "..."), "hello");
    }

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
        assert_eq!(
            truncate_with_suffix("hello world", 11, "..."),
            "hello world"
        );
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
        assert!(
            result.len() <= 10,
            "Result too long: {} bytes",
            result.len()
        );
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
        assert_eq!(
            truncate_with_suffix("hello world", 8, " [truncated]"),
            " [truncated]"
        );
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
