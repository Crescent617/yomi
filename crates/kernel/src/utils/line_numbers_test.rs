use super::*;

#[test]
fn test_add_line_numbers_simple() {
    let content = "line 1\nline 2\nline 3";
    let result = add_line_numbers(content, 1);
    assert_eq!(result, "1\tline 1\n2\tline 2\n3\tline 3");
}

#[test]
fn test_add_line_numbers_with_offset() {
    let content = "line 10\nline 11";
    let result = add_line_numbers(content, 10);
    assert_eq!(result, "10\tline 10\n11\tline 11");
}

#[test]
fn test_add_line_numbers_padding() {
    // Content with 10 lines to trigger padding for single digits
    let content = "line 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\nline 8\nline 9\nline 10";
    let result = add_line_numbers(content, 1);
    // Line numbers should be aligned
    assert!(
        result.contains(" 1\tline 1"),
        "Expected ' 1' padding, got: {result}"
    );
    assert!(
        result.contains("10\tline 10"),
        "Expected '10' no padding, got: {result}"
    );
}

#[test]
fn test_add_line_numbers_empty() {
    let result = add_line_numbers("", 1);
    assert_eq!(result, "");
}

#[test]
fn test_add_line_numbers_no_trailing_newline() {
    let content = "line 1\nline 2";
    let result = add_line_numbers(content, 1);
    assert!(!result.ends_with('\n'));
}

#[test]
fn test_num_digits() {
    assert_eq!(num_digits(0), 1);
    assert_eq!(num_digits(9), 1);
    assert_eq!(num_digits(10), 2);
    assert_eq!(num_digits(99), 2);
    assert_eq!(num_digits(100), 3);
    assert_eq!(num_digits(1000), 4);
}

#[test]
fn test_strip_line_numbers() {
    let formatted = "1\tline 1\n2\tline 2";
    let result = strip_line_numbers(formatted);
    assert_eq!(result, "line 1\nline 2");
}

#[test]
fn test_strip_line_numbers_with_pipe() {
    let formatted = "1  |line 1\n2  |line 2";
    let result = strip_line_numbers(formatted);
    assert_eq!(result, "line 1\nline 2");
}
