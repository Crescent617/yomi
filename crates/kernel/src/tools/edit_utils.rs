//! Utility functions for the Edit tool
//!
//! These functions handle quote normalization, diff generation, and edit validation.

// Curly quote characters
pub const LEFT_SINGLE_CURLY_QUOTE: char = '\u{2018}';  // '
pub const RIGHT_SINGLE_CURLY_QUOTE: char = '\u{2019}'; // '
pub const LEFT_DOUBLE_CURLY_QUOTE: char = '\u{201c}';  // "
pub const RIGHT_DOUBLE_CURLY_QUOTE: char = '\u{201d}'; // "

/// Normalize quotes in a string by converting curly quotes to straight quotes
///
/// This is needed because models typically output straight quotes, but files
/// may contain curly quotes (e.g., from copy-paste from rich text editors).
pub fn normalize_quotes(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            LEFT_SINGLE_CURLY_QUOTE | RIGHT_SINGLE_CURLY_QUOTE => '\'',
            LEFT_DOUBLE_CURLY_QUOTE | RIGHT_DOUBLE_CURLY_QUOTE => '"',
            _ => c,
        })
        .collect()
}

/// Check if a string contains any curly quotes
pub fn has_curly_quotes(s: &str) -> bool {
    s.chars().any(|c| {
        c == LEFT_SINGLE_CURLY_QUOTE
            || c == RIGHT_SINGLE_CURLY_QUOTE
            || c == LEFT_DOUBLE_CURLY_QUOTE
            || c == RIGHT_DOUBLE_CURLY_QUOTE
    })
}

/// Find the actual string in file content, accounting for quote normalization
///
/// Returns the actual string found in the file (with original quote style),
/// or None if the string is not found.
///
/// This function tries:
/// 1. Exact match
/// 2. Match after normalizing curly quotes in the file content
/// 3. Match after normalizing curly quotes in the search string
/// 4. Match after normalizing curly quotes in both
pub fn find_actual_string(file_content: &str, search_string: &str) -> Option<String> {
    // First try exact match
    if file_content.contains(search_string) {
        return Some(search_string.to_string());
    }

    // Try with normalized quotes in file content
    let normalized_file = normalize_quotes(file_content);
    let normalized_search = normalize_quotes(search_string);

    if let Some(pos) = normalized_file.find(&normalized_search) {
        // Map the position back to the original file content
        // We need to find the substring in the original file that corresponds
        // to the normalized match position
        let search_len_chars = normalized_search.chars().count();

        // Find the byte position in the original file that corresponds to
        // the character position in the normalized file
        let mut orig_byte_pos = 0;
        let mut norm_char_pos = 0;

        // Iterate through the original file to find the start position
        for (byte_idx, c) in file_content.char_indices() {
            if norm_char_pos == pos {
                orig_byte_pos = byte_idx;
                break;
            }
            // Normalize this character to count it
            let normalized_char = normalize_quotes(&c.to_string());
            norm_char_pos += normalized_char.chars().count();
        }

        // Now find the end position by iterating from the start
        let mut end_byte_pos = orig_byte_pos;
        let mut chars_matched = 0;
        for (byte_idx, c) in file_content[orig_byte_pos..].char_indices() {
            if chars_matched >= search_len_chars {
                end_byte_pos = orig_byte_pos + byte_idx;
                break;
            }
            let normalized_char = normalize_quotes(&c.to_string());
            chars_matched += normalized_char.chars().count();
            end_byte_pos = orig_byte_pos + byte_idx + c.len_utf8();
        }

        return Some(file_content[orig_byte_pos..end_byte_pos].to_string());
    }

    None
}

/// Apply curly quotes to a string based on context
///
/// Uses a simple heuristic: a quote character preceded by whitespace,
/// start of string, or opening punctuation is treated as an opening quote;
/// otherwise it's a closing quote.
pub fn apply_curly_quotes(s: &str, use_single: bool, use_double: bool) -> String {
    if !use_single && !use_double {
        return s.to_string();
    }

    let chars: Vec<char> = s.chars().collect();
    let mut result = String::with_capacity(s.len());

    for (i, c) in chars.iter().enumerate() {
        match c {
            '\'' if use_single => {
                if is_opening_context(&chars, i) {
                    result.push(LEFT_SINGLE_CURLY_QUOTE);
                } else {
                    result.push(RIGHT_SINGLE_CURLY_QUOTE);
                }
            }
            '"' if use_double => {
                if is_opening_context(&chars, i) {
                    result.push(LEFT_DOUBLE_CURLY_QUOTE);
                } else {
                    result.push(RIGHT_DOUBLE_CURLY_QUOTE);
                }
            }
            _ => result.push(*c),
        }
    }

    result
}

/// Check if a quote at the given position is an opening quote
fn is_opening_context(chars: &[char], index: usize) -> bool {
    if index == 0 {
        return true;
    }

    let prev = chars[index - 1];
    matches!(
        prev,
        ' ' | '\t' | '\n' | '\r' | '(' | '[' | '{' | '\u{2014}' | '\u{2013}'
    )
}

/// Preserve quote style when making edits
///
/// When `old_string` matched via quote normalization (curly quotes in file,
/// straight quotes from model), apply the same curly quote style to `new_string`
/// so the edit preserves the file's typography.
pub fn preserve_quote_style(old_string: &str, actual_old_string: &str, new_string: &str) -> String {
    // If they're the same, no normalization happened
    if old_string == actual_old_string {
        return new_string.to_string();
    }

    // Detect which curly quote types were in the file
    let has_double = actual_old_string.contains(LEFT_DOUBLE_CURLY_QUOTE)
        || actual_old_string.contains(RIGHT_DOUBLE_CURLY_QUOTE);
    let has_single = actual_old_string.contains(LEFT_SINGLE_CURLY_QUOTE)
        || actual_old_string.contains(RIGHT_SINGLE_CURLY_QUOTE);

    apply_curly_quotes(new_string, has_single, has_double)
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
    let first_diff = first_diff.unwrap_or_else(|| {
        original_lines.len().min(modified_lines.len())
    });

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
    for i in context_start..first_diff {
        result.push_str(&format!(" {}\n", original_lines[i]));
    }

    // Show removed lines
    for i in first_diff..last_diff_orig {
        result.push_str(&format!("-{}", original_lines[i]));
        if i < original_lines.len() - 1 || original.ends_with('\n') {
            result.push('\n');
        }
    }

    // Show added lines
    for i in first_diff..last_diff_mod {
        result.push_str(&format!("+{}", modified_lines[i]));
        if i < modified_lines.len() - 1 || modified.ends_with('\n') {
            result.push('\n');
        }
    }

    // Show context after
    for i in last_diff_orig..context_end_orig {
        result.push_str(&format!(" {}\n", original_lines[i]));
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
    fn test_normalize_quotes() {
        assert_eq!(normalize_quotes("'hello'"), "'hello'");
        assert_eq!(normalize_quotes("\"hello\""), "\"hello\"");
        assert_eq!(normalize_quotes("\u{2018}hello\u{2019}"), "'hello'");
        assert_eq!(normalize_quotes("\u{201c}hello\u{201d}"), "\"hello\"");
    }

    #[test]
    fn test_has_curly_quotes() {
        assert!(!has_curly_quotes("'hello'"));
        assert!(has_curly_quotes("\u{2018}hello\u{2019}"));
        assert!(has_curly_quotes("\u{201c}hello\u{201d}"));
    }

    #[test]
    fn test_find_actual_string_exact_match() {
        let content = "hello world";
        assert_eq!(
            find_actual_string(content, "hello"),
            Some("hello".to_string())
        );
    }

    #[test]
    fn test_find_actual_string_with_curly_quotes() {
        let content = "say \u{2018}hello\u{2019} to the world";
        // Model provides straight quotes
        assert_eq!(
            find_actual_string(content, "'hello'"),
            Some("\u{2018}hello\u{2019}".to_string())
        );
    }

    #[test]
    fn test_find_actual_string_not_found() {
        let content = "hello world";
        assert_eq!(find_actual_string(content, "foo"), None);
    }

    #[test]
    fn test_apply_curly_quotes() {
        let input = r#"He said "hello" and 'goodbye'"#;
        let result = apply_curly_quotes(input, true, true);
        assert!(result.contains(LEFT_DOUBLE_CURLY_QUOTE));
        assert!(result.contains(RIGHT_DOUBLE_CURLY_QUOTE));
        assert!(result.contains(LEFT_SINGLE_CURLY_QUOTE));
        assert!(result.contains(RIGHT_SINGLE_CURLY_QUOTE));
    }

    #[test]
    fn test_preserve_quote_style() {
        let old = "'hello'";
        let actual = "\u{2018}hello\u{2019}";
        let new = "'hi there'";

        let result = preserve_quote_style(old, actual, new);
        assert!(result.contains(LEFT_SINGLE_CURLY_QUOTE));
        assert!(result.contains(RIGHT_SINGLE_CURLY_QUOTE));
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
