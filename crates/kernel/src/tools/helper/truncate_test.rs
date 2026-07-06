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
