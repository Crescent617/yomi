/// Add line numbers to file content
///
/// Format matches claude-code: line number prefix followed by tab character.
/// Line numbers are right-aligned and padded with spaces.
pub fn add_line_numbers(content: &str, start_line: usize) -> String {
    if content.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = content.lines().collect();
    let total_lines = start_line + lines.len() - 1;

    // Calculate the width needed for the largest line number
    let num_width = num_digits(total_lines);

    let mut result = String::new();
    for (i, line) in lines.iter().enumerate() {
        let line_num = start_line + i;
        // Format: right-aligned line number, padded with spaces, followed by tab
        result.push_str(&format!("{line_num:>num_width$}\t{line}"));
        result.push('\n');
    }

    // Remove trailing newline if original content didn't have one
    if !content.ends_with('\n') && !result.is_empty() {
        result.pop();
    }

    result
}

/// Count the number of digits in a number
fn num_digits(n: usize) -> usize {
    if n == 0 {
        return 1;
    }
    n.checked_ilog10().unwrap_or(0) as usize + 1
}

/// Format file content with line numbers for display
///
/// This is the main entry point for formatting file content with line numbers.
/// It handles the line number prefix format used throughout the codebase.
pub fn format_file_lines(content: &str, start_line: usize) -> String {
    add_line_numbers(content, start_line)
}

/// Remove line number prefixes from content
///
/// This is used when extracting the actual content from formatted output
/// for use in edit operations.
pub fn strip_line_numbers(formatted: &str) -> String {
    let mut result = String::new();

    for line in formatted.lines() {
        // Find the tab character that separates the line number from content
        if let Some(tab_pos) = line.find('\t') {
            result.push_str(&line[tab_pos + 1..]);
        } else {
            // No tab found, try to find a space after a number
            // This handles the "1  |line content" format
            if let Some(pipe_pos) = line.find(" |") {
                result.push_str(&line[pipe_pos + 2..]);
            } else {
                // Fallback: return the line as-is
                result.push_str(line);
            }
        }
        result.push('\n');
    }

    // Remove trailing newline if input didn't have one
    if !formatted.ends_with('\n') && !result.is_empty() {
        result.pop();
    }

    result
}

#[cfg(test)]
mod tests {
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
        assert!(result.contains(" 1\tline 1"), "Expected ' 1' padding, got: {}", result);
        assert!(result.contains("10\tline 10"), "Expected '10' no padding, got: {}", result);
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
}
