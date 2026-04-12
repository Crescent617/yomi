//! Utility functions for the Edit tool
//!
//! These functions handle diff generation and edit validation.

use std::fmt::Write;

/// Find the actual string in file content.
///
/// Returns `Some(search_string)` if found, None otherwise.
/// This is a simple wrapper that may be extended for quote normalization in the future.
pub fn find_actual_string(file_content: &str, search_string: &str) -> Option<String> {
    if file_content.contains(search_string) {
        Some(search_string.to_string())
    } else {
        None
    }
}


/// Split content into lines, preserving trailing newline info
///
/// Returns a tuple of (lines, `has_trailing_newline`)
fn split_lines(content: &str) -> (Vec<&str>, bool) {
    if content.is_empty() {
        return (Vec::new(), false);
    }
    let has_trailing_newline = content.ends_with('\n');
    let lines: Vec<&str> = content.split('\n').collect();
    // split('\n') on "a\nb\n" gives ["a", "b", ""], we want ["a", "b"] with has_trailing_newline=true
    let mut lines = lines;
    if has_trailing_newline && lines.last() == Some(&"") {
        lines.pop();
    }
    (lines, has_trailing_newline)
}

/// Generate a simple diff between two strings
///
/// Returns a unified diff-style output showing the changes.
pub fn generate_diff(original: &str, modified: &str, context_lines: usize) -> String {
    let (original_lines, orig_has_newline) = split_lines(original);
    let (modified_lines, mod_has_newline) = split_lines(modified);

    // Find the first difference
    let mut first_diff = None;
    for (i, (orig, modif)) in original_lines.iter().zip(modified_lines.iter()).enumerate() {
        if orig != modif {
            first_diff = Some(i);
            break;
        }
    }

    // If no difference found in common length, check for additions/removals at end
    let first_diff = first_diff.unwrap_or_else(|| original_lines.len().min(modified_lines.len()));

    // Find the last difference by comparing from the end
    let mut last_diff_orig = original_lines.len();
    let mut last_diff_mod = modified_lines.len();

    while last_diff_orig > first_diff && last_diff_mod > first_diff {
        if original_lines[last_diff_orig - 1] == modified_lines[last_diff_mod - 1] {
            last_diff_orig -= 1;
            last_diff_mod -= 1;
        } else {
            break;
        }
    }

    // Check if trailing newline status differs
    let newline_differs = orig_has_newline != mod_has_newline;

    if first_diff == last_diff_orig
        && first_diff == last_diff_mod
        && !newline_differs
    {
        return "No changes".to_string();
    }

    // Calculate context range
    let context_start = first_diff.saturating_sub(context_lines);
    let context_end_orig = (last_diff_orig + context_lines).min(original_lines.len());

    let mut result = String::new();

    // Show context before
    for line in original_lines.iter().take(first_diff).skip(context_start) {
        writeln!(result, " {line}").unwrap();
    }

    // Show removed lines
    for line in original_lines
        .iter()
        .take(last_diff_orig)
        .skip(first_diff)
    {
        writeln!(result, "-{line}").unwrap();
    }

    // Show added lines
    for line in modified_lines
        .iter()
        .take(last_diff_mod)
        .skip(first_diff)
    {
        writeln!(result, "+{line}").unwrap();
    }

    // Show context after
    for line in original_lines
        .iter()
        .take(context_end_orig)
        .skip(last_diff_orig)
    {
        writeln!(result, " {line}").unwrap();
    }

    result
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_actual_string() {
        let content = "hello world";
        assert_eq!(
            find_actual_string(content, "hello"),
            Some("hello".to_string())
        );
        assert_eq!(find_actual_string(content, "foo"), None);
    }

    #[test]
    fn test_generate_diff() {
        let original = "line 1\nline 2\nline 3";
        let modified = "line 1\nmodified 2\nline 3";
        let diff = generate_diff(original, modified, 2);
        assert!(diff.contains("-line 2"));
        assert!(diff.contains("+modified 2"));
    }

    #[test]
    fn test_generate_diff_with_newlines_in_args() {
        // Test multi-line replacement
        let original = "fn foo() {\n    println!(\"hello\");\n}";
        let modified = "fn foo() {\n    println!(\"world\");\n}";
        let diff = generate_diff(original, modified, 2);
        assert!(diff.contains("-    println!(\"hello\");"));
        assert!(diff.contains("+    println!(\"world\");"));
    }

    #[test]
    fn test_generate_diff_trailing_newline() {
        // Test that trailing newlines are handled correctly
        let original = "line 1\nline 2\n";
        let modified = "line 1\nline 2\n";
        let diff = generate_diff(original, modified, 2);
        assert_eq!(diff, "No changes");
    }

    #[test]
    fn test_split_lines() {
        let (lines, has_newline) = split_lines("a\nb\n");
        assert_eq!(lines, vec!["a", "b"]);
        assert!(has_newline);

        let (lines, has_newline) = split_lines("a\nb");
        assert_eq!(lines, vec!["a", "b"]);
        assert!(!has_newline);

        let (lines, has_newline) = split_lines("");
        assert!(lines.is_empty());
        assert!(!has_newline);
    }

    #[test]
    fn test_find_actual_string_edge_cases() {
        // Empty content and search string
        assert_eq!(find_actual_string("", ""), Some(String::new()));
        assert_eq!(find_actual_string("", "hello"), None);
        assert_eq!(find_actual_string("hello", ""), Some(String::new()));

        // Multi-line content
        let content = "line1\nline2\nline3";
        assert_eq!(find_actual_string(content, "line2"), Some("line2".to_string()));
        assert_eq!(find_actual_string(content, "line1\nline2"), Some("line1\nline2".to_string()));

        // Special characters
        assert_eq!(find_actual_string("hello\tworld", "\t"), Some("\t".to_string()));
        assert_eq!(find_actual_string("hello\n\nworld", "\n\n"), Some("\n\n".to_string()));

        // Unicode
        assert_eq!(find_actual_string("你好世界", "世界"), Some("世界".to_string()));
    }

    #[test]
    fn test_generate_diff_empty_content() {
        // Empty to content
        let diff = generate_diff("", "new content", 2);
        assert!(diff.contains("+new content"));

        // Content to empty
        let diff = generate_diff("old content", "", 2);
        assert!(diff.contains("-old content"));

        // Both empty
        assert_eq!(generate_diff("", "", 2), "No changes");
    }

    #[test]
    fn test_generate_diff_at_start() {
        // Change at the beginning
        let original = "old_first\nline2\nline3";
        let modified = "new_first\nline2\nline3";
        let diff = generate_diff(original, modified, 1);
        assert!(diff.contains("-old_first"));
        assert!(diff.contains("+new_first"));
        assert!(diff.contains(" line2")); // context
    }

    #[test]
    fn test_generate_diff_at_end() {
        // Change at the end
        let original = "line1\nline2\nold_last";
        let modified = "line1\nline2\nnew_last";
        let diff = generate_diff(original, modified, 1);
        assert!(diff.contains("-old_last"));
        assert!(diff.contains("+new_last"));
        assert!(diff.contains(" line2")); // context
    }

    #[test]
    fn test_generate_diff_add_lines() {
        // Add lines in the middle
        let original = "line1\nline4";
        let modified = "line1\nline2\nline3\nline4";
        let diff = generate_diff(original, modified, 1);
        assert!(diff.contains("+line2"));
        assert!(diff.contains("+line3"));
        assert!(diff.contains(" line1"));
        assert!(diff.contains(" line4"));
    }

    #[test]
    fn test_generate_diff_remove_lines() {
        // Remove lines from the middle
        let original = "line1\nline2\nline3\nline4";
        let modified = "line1\nline4";
        let diff = generate_diff(original, modified, 1);
        assert!(diff.contains("-line2"));
        assert!(diff.contains("-line3"));
        assert!(diff.contains(" line1"));
        assert!(diff.contains(" line4"));
    }

    #[test]
    fn test_generate_diff_multiple_changes() {
        // Multiple separate changes (diff shows the whole range)
        let original = "a\nb\nc\nd\ne";
        let modified = "a\nB\nc\nD\ne";
        let diff = generate_diff(original, modified, 1);
        assert!(diff.contains("-b"));
        assert!(diff.contains("+B"));
        assert!(diff.contains("-d"));
        assert!(diff.contains("+D"));
    }

    #[test]
    fn test_generate_diff_whole_file_change() {
        // Entire file changed
        let original = "old line 1\nold line 2";
        let modified = "new line 1\nnew line 2";
        let diff = generate_diff(original, modified, 2);
        assert!(diff.contains("-old line 1"));
        assert!(diff.contains("-old line 2"));
        assert!(diff.contains("+new line 1"));
        assert!(diff.contains("+new line 2"));
    }

    #[test]
    fn test_generate_diff_no_context() {
        // Zero context lines
        let original = "line1\nline2\nline3\nline4\nline5";
        let modified = "line1\nCHANGED\nline3\nline4\nline5";
        let diff = generate_diff(original, modified, 0);
        assert!(diff.contains("-line2"));
        assert!(diff.contains("+CHANGED"));
        // Should not have context lines when context_lines is 0
        assert!(!diff.contains(" line1"));
        assert!(!diff.contains(" line3"));
    }

    #[test]
    fn test_generate_diff_context_limit() {
        // Context lines should be limited
        let original = "a\nb\nc\nd\ne\nf\ng";
        let modified = "a\nb\nC\nd\ne\nf\ng";
        let diff = generate_diff(original, modified, 1);
        assert!(diff.contains("-c"));
        assert!(diff.contains("+C"));
        assert!(diff.contains(" b"));
        assert!(diff.contains(" d"));
        // Should not include 'a' and 'e' when context is 1
        assert!(!diff.contains(" a"));
        assert!(!diff.contains(" e"));
    }

    #[test]
    fn test_split_lines_edge_cases() {
        // Just a newline
        let (lines, has_newline) = split_lines("\n");
        assert_eq!(lines, vec![""]);
        assert!(has_newline);

        // Multiple empty lines
        let (lines, has_newline) = split_lines("\n\n\n");
        assert_eq!(lines, vec!["", "", ""]);
        assert!(has_newline);

        // Single line without newline
        let (lines, has_newline) = split_lines("hello");
        assert_eq!(lines, vec!["hello"]);
        assert!(!has_newline);

        // Single line with newline
        let (lines, has_newline) = split_lines("hello\n");
        assert_eq!(lines, vec!["hello"]);
        assert!(has_newline);
    }

    #[test]
    fn test_generate_diff_newline_change_only() {
        // Only difference is trailing newline
        // Content lines are identical but one has trailing newline and other doesn't
        let original = "content";
        let modified = "content\n";
        let diff = generate_diff(original, modified, 2);
        // The function shows the content line as context (unchanged)
        // because currently it doesn't have special handling for newline-only changes
        assert!(diff.contains(" content"));
    }
}
