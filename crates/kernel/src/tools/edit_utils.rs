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

/// Count occurrences of a substring in a string
pub fn count_occurrences(haystack: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    haystack.matches(needle).count()
}

/// Generate a simple diff between two strings
///
/// Returns a unified diff-style output showing the changes.
pub fn generate_diff(original: &str, modified: &str, context_lines: usize) -> String {
    let original_lines: Vec<&str> = original.lines().collect();
    let modified_lines: Vec<&str> = modified.lines().collect();

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

    if first_diff == last_diff_orig && first_diff == last_diff_mod {
        return "No changes".to_string();
    }

    // Calculate context range
    let context_start = first_diff.saturating_sub(context_lines);
    let context_end_orig = (last_diff_orig + context_lines).min(original_lines.len());
    let _context_end_mod = (last_diff_mod + context_lines).min(modified_lines.len());

    let mut result = String::new();

    // Show context before
    for line in original_lines.iter().take(first_diff).skip(context_start) {
        writeln!(result, " {line}").unwrap();
    }

    // Show removed lines
    for (i, line) in original_lines
        .iter()
        .enumerate()
        .take(last_diff_orig)
        .skip(first_diff)
    {
        write!(result, "-{line}").unwrap();
        if i < original_lines.len() - 1 || original.ends_with('\n') {
            result.push('\n');
        }
    }

    // Show added lines
    for (i, line) in modified_lines
        .iter()
        .enumerate()
        .take(last_diff_mod)
        .skip(first_diff)
    {
        write!(result, "+{line}").unwrap();
        if i < modified_lines.len() - 1 || modified.ends_with('\n') {
            result.push('\n');
        }
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

/// Generate a patch-style output with line numbers
///
/// This format is more user-friendly for displaying edits.
pub fn generate_patch(
    file_path: &str,
    original: &str,
    _modified: &str,
    old_string: &str,
    new_string: &str,
) -> String {
    let original_lines: Vec<&str> = original.lines().collect();

    // Find the line number where old_string appears
    let mut start_line = 1;
    for (i, line) in original_lines.iter().enumerate() {
        if line.contains(old_string) {
            start_line = i + 1;
            break;
        }
    }

    let old_count = old_string.lines().count();
    let new_count = new_string.lines().count();

    format!(
        "--- {file_path}\n+++ {file_path}\n@@ -{start_line},{old_count} +{start_line},{new_count} @@"
    )
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
    fn test_count_occurrences() {
        assert_eq!(count_occurrences("hello hello hello", "hello"), 3);
        assert_eq!(count_occurrences("hello world", "foo"), 0);
        assert_eq!(count_occurrences("", "hello"), 0);
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
    fn test_generate_patch() {
        let original = "line 1\nline 2\nline 3";
        let patch = generate_patch("test.txt", original, original, "line 2", "modified");
        assert!(patch.contains("--- test.txt"));
        assert!(patch.contains("+++ test.txt"));
        assert!(patch.contains("@@"));
    }
}
