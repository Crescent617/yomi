use std::fmt::Write;

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
        writeln!(result, "{line_num:>num_width$}\t{line}").unwrap();
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
#[path = "line_numbers_test.rs"]
mod tests;
