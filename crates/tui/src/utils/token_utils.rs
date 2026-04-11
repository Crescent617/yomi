//! Token counting utilities

/// Count tokens using a better estimation
/// For English: 1 token ≈ 4 characters
/// For CJK: 1 token ≈ 1-1.5 characters
pub fn count_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }

    // Count different character types
    let mut ascii_count = 0;
    let mut cjk_count = 0;
    let mut other_count = 0;

    for c in text.chars() {
        if c.is_ascii() {
            ascii_count += 1;
        } else if is_cjk(c) {
            cjk_count += 1;
        } else {
            other_count += 1;
        }
    }

    // ASCII: ~4 chars per token, CJK: ~1.5 chars per token, Other: ~2 chars per token
    let ascii_tokens = ascii_count / 4;
    let cjk_tokens = (cjk_count * 2) / 3; // 1/1.5 ≈ 2/3
    let other_tokens = other_count / 2;

    (ascii_tokens + cjk_tokens + other_tokens).max(1)
}

/// Simple token estimation (English only, ~4 chars per token)
/// Use this when you need a quick estimate without CJK detection overhead
pub fn estimate_tokens_simple(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    (text.len() / 4).max(1)
}

/// Check if a character is CJK (Chinese, Japanese, Korean)
const fn is_cjk(c: char) -> bool {
    // CJK ranges
    matches!(
        c,
        '\u{4e00}'..='\u{9fff}' | // CJK Unified Ideographs
        '\u{3040}'..='\u{309f}' | // Hiragana
        '\u{30a0}'..='\u{30ff}' | // Katakana
        '\u{ac00}'..='\u{d7af}' | // Hangul Syllables
        '\u{3400}'..='\u{4dbf}' | // CJK Extension A
        '\u{f900}'..='\u{faff}'   // CJK Compatibility Ideographs
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_count_tokens_empty() {
        assert_eq!(count_tokens(""), 0);
        assert_eq!(estimate_tokens_simple(""), 0);
    }

    #[test]
    fn test_count_tokens_ascii() {
        // ~4 chars per token
        assert_eq!(count_tokens("hello"), 1);
        assert_eq!(count_tokens("hello world"), 2);
        assert_eq!(count_tokens("this is a test string"), 5);
    }

    #[test]
    fn test_count_tokens_cjk() {
        // CJK chars count more heavily
        let cjk_text = "你好世界"; // 4 CJK chars
        assert!(count_tokens(cjk_text) > 1);
    }

    #[test]
    fn test_estimate_tokens_simple() {
        assert_eq!(estimate_tokens_simple("test"), 1);
        assert_eq!(estimate_tokens_simple("testing"), 1);
        assert_eq!(estimate_tokens_simple("hello world test"), 4);
    }
}
