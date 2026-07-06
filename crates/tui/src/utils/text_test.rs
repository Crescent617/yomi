use super::*;

#[test]
fn test_truncate_by_chars_ascii() {
    assert_eq!(truncate_by_chars("hello world", 20), "hello world");
    assert_eq!(truncate_by_chars("hello world", 8), "hello...");
    assert_eq!(truncate_by_chars("hello", 5), "hello");
    assert_eq!(truncate_by_chars("hello", 4), "h...");
    assert_eq!(truncate_by_chars("hello", 3), "...");
}

#[test]
fn test_truncate_by_chars_multibyte() {
    // Chinese characters (11 chars: 这是一个很长的中文句子)
    let chinese = "这是一个很长的中文句子";
    assert_eq!(truncate_by_chars(chinese, 20), chinese);
    assert_eq!(truncate_by_chars(chinese, 11), chinese);
    assert_eq!(truncate_by_chars(chinese, 10), "这是一个很长的...");

    // Emoji (10 chars total, each emoji is 1 char though multiple bytes in UTF-8)
    let emoji = "🎉🎊🎁🎄🎃🎅🤶🧑‍🎄";
    assert_eq!(truncate_by_chars(emoji, 10), emoji);
    assert_eq!(truncate_by_chars(emoji, 5), "🎉🎊..."); // 5-3=2 chars preserved

    // Mixed (12 chars: Hello世界🎉Test)
    let mixed = "Hello世界🎉Test";
    assert_eq!(truncate_by_chars(mixed, 20), mixed);
    assert_eq!(truncate_by_chars(mixed, 10), "Hello世界..."); // 10-3=7 chars preserved
}

#[test]
fn test_truncate_by_chars_edge_cases() {
    assert_eq!(truncate_by_chars("", 10), "");
    assert_eq!(truncate_by_chars("ab", 3), "ab");
    assert_eq!(truncate_by_chars("abc", 3), "abc");
    assert_eq!(truncate_by_chars("abcd", 3), "...");
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
