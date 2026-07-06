use super::*;

#[test]
fn test_estimate_tokens_empty() {
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn test_estimate_tokens_ascii() {
    // ~4 chars per token
    assert_eq!(estimate_tokens("hello"), 1); // 5 / 4 = 1
    assert_eq!(estimate_tokens("hello world"), 2); // 11 / 4 = 2
    assert_eq!(estimate_tokens("this is a test string"), 5); // 21 / 4 = 5
}

#[test]
fn test_estimate_tokens_cjk() {
    // CJK chars are 3 bytes each in UTF-8
    let cjk_text = "你好世界"; // 12 bytes (3 * 4)
    assert_eq!(estimate_tokens(cjk_text), 3); // 12 / 4 = 3
}

#[test]
fn test_estimate_tokens_for_json() {
    let json = r#"{"key": "value", "num": 123}"#;
    // 28 bytes, / 2 = 14 tokens (denser)
    assert_eq!(estimate_tokens_for_json(json), 14);
}

#[test]
fn test_format_estimated_tokens() {
    assert_eq!(format_estimated_tokens(100), "~100");
    assert_eq!(format_estimated_tokens(1500), "~1.5k");
    assert_eq!(format_estimated_tokens(10000), "~10.0k");
}

#[test]
fn test_estimate_tokens_boundary() {
    // Test boundary conditions (4 chars per token)
    assert_eq!(estimate_tokens("a"), 0); // 1 / 4 = 0
    assert_eq!(estimate_tokens("abcd"), 1); // 4 / 4 = 1
    assert_eq!(estimate_tokens("abcde"), 1); // 5 / 4 = 1
    assert_eq!(estimate_tokens("abcdefgh"), 2); // 8 / 4 = 2
}

#[test]
fn test_estimate_tokens_unicode() {
    // Unicode characters have different byte lengths
    // ASCII: 1 byte, CJK: 3 bytes, Emoji: 4 bytes
    assert_eq!(estimate_tokens("🎉"), 1); // 4 bytes
    assert_eq!(estimate_tokens("🎉🎊"), 2); // 8 bytes
    assert_eq!(estimate_tokens("α"), 0); // Greek 2 bytes
    assert_eq!(estimate_tokens("αβγδ"), 2); // Greek 8 bytes = 2 tokens
}

#[test]
fn test_estimate_tokens_for_json_boundary() {
    // JSON uses 2 chars per token
    assert_eq!(estimate_tokens_for_json("{}"), 1); // 2 / 2 = 1
    assert_eq!(estimate_tokens_for_json("[]"), 1); // 2 / 2 = 1
                                                   // "{\"a\":1}" is 7 bytes: { (1) + " (1) + a (1) + " (1) + : (1) + 1 (1) + } (1)
    assert_eq!(estimate_tokens_for_json("{\"a\":1}"), 3); // 7 / 2 = 3
}

#[test]
fn test_format_estimated_tokens_boundaries() {
    assert_eq!(format_estimated_tokens(0), "~0");
    assert_eq!(format_estimated_tokens(1), "~1");
    assert_eq!(format_estimated_tokens(999), "~999");
    assert_eq!(format_estimated_tokens(1000), "~1.0k");
    assert_eq!(format_estimated_tokens(9999), "~10.0k"); // Actually ~10.0k
    assert_eq!(format_estimated_tokens(100_000), "~100.0k");
}

#[test]
fn test_estimate_tokens_for_messages_empty() {
    let messages: Vec<crate::types::Message> = vec![];
    assert_eq!(estimate_tokens_for_messages(&messages), 0);
}

#[test]
fn test_estimate_tokens_whitespace() {
    // Whitespace counts as characters
    assert_eq!(estimate_tokens("    "), 1); // 4 spaces = 1 token
    assert_eq!(estimate_tokens("\n\n\n\n"), 1); // 4 newlines = 1 token
    assert_eq!(estimate_tokens("\t\t\t\t"), 1); // 4 tabs = 1 token
}
