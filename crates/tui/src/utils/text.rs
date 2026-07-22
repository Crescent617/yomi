//! Text preprocessing utilities for TUI rendering

use tuirealm::ratatui::text::{Line, Span};

/// Preprocess text for display by:
/// - Converting tabs to 2 spaces for consistent width
pub fn preprocess(text: impl AsRef<str>) -> String {
    text.as_ref().replace('\t', "  ")
}

/// Humanize a tool name for display: convert `snake_case` / `kebab-case` /
/// space-separated names to CamelCase (e.g. `my_custom_tool` → `MyCustomTool`).
pub fn humanize_tool_name(s: &str) -> String {
    if s.is_empty() {
        return String::new();
    }

    // If already starts with uppercase, assume it's already CamelCase
    if s.starts_with(|c: char| c.is_uppercase()) {
        return s.to_string();
    }

    let mut result = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if matches!(c, '_' | '-' | ' ') {
            capitalize_next = true;
        } else if capitalize_next {
            result.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            result.push(c);
        }
    }
    result
}

/// Extract a segment of a line as a new Line with owned data, preserving styles.
/// Extracts text from `start_byte` (inclusive) to `end_byte` (exclusive).
pub fn extract_line_segment(line: &Line<'_>, start_byte: usize, end_byte: usize) -> Line<'static> {
    let mut spans = Vec::new();
    let mut current_byte = 0;

    for span in &line.spans {
        let span_text = span.content.as_ref();
        let span_len = span_text.len();
        let span_start = current_byte;
        let span_end = current_byte + span_len;

        // Check if this span overlaps with the target range
        if span_end <= start_byte || span_start >= end_byte {
            current_byte = span_end;
            continue;
        }

        // Calculate overlap
        let overlap_start = start_byte.saturating_sub(span_start);
        let overlap_end = end_byte.saturating_sub(span_start).min(span_len);

        if overlap_start < overlap_end {
            debug_assert!(span_text.is_char_boundary(overlap_start));
            debug_assert!(span_text.is_char_boundary(overlap_end));
            let extracted = &span_text[overlap_start..overlap_end];
            spans.push(Span::styled(extracted.to_string(), span.style));
        }

        current_byte = span_end;
    }

    Line::from(spans).style(line.style)
}

/// Get byte index from character index (Unicode-safe)
/// Returns the byte position corresponding to the `char_idx`-th character
pub fn char_idx_to_byte_idx(text: &str, char_idx: usize) -> usize {
    text.char_indices()
        .nth(char_idx)
        .map_or(text.len(), |(byte_idx, _)| byte_idx)
}

/// Extract substring by character indices (Unicode-safe)
/// Returns the substring from `start_char` to `end_char` (in characters, not bytes)
pub fn substring_by_chars(text: &str, start_char: usize, end_char: usize) -> String {
    text.chars()
        .skip(start_char)
        .take(end_char.saturating_sub(start_char))
        .collect()
}

/// Truncate text to max character count (Unicode-safe)
/// Returns the truncated string with "..." suffix if truncated
pub fn truncate_by_chars(text: &str, max_chars: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= max_chars {
        text.to_string()
    } else if max_chars <= 3 {
        // If max is 3 or less, just return "..."
        "...".to_string()
    } else {
        let truncated: String = text.chars().take(max_chars - 3).collect();
        format!("{truncated}...")
    }
}

/// Calculate wrap boundaries using display width (Unicode-aware).
/// Returns vector of byte indices where each visual row starts.
pub fn calc_wrap_boundaries(text: &str, width: usize) -> Vec<usize> {
    if width == 0 || text.is_empty() {
        return vec![0];
    }

    let mut boundaries = vec![0];
    let mut current_width = 0;
    let mut byte_idx = 0;

    for ch in text.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);

        if current_width + ch_width > width && current_width > 0 {
            boundaries.push(byte_idx);
            current_width = ch_width;
        } else {
            current_width += ch_width;
        }
        byte_idx += ch.len_utf8();
    }

    boundaries
}

/// Truncate text by display width (accounts for CJK characters being 2 columns).
/// Returns the truncated string with suffix appended if truncated.
///
/// # Arguments
/// * `text` - The input string
/// * `max_width` - Maximum display width in columns
/// * `suffix` - Suffix to append when truncated (e.g., "...")
///
/// # Behavior
/// - If `text` display width <= `max_width`: returns `text` as-is (no suffix)
/// - If `max_width <= suffix width`: returns truncated suffix
/// - Otherwise: truncates to fit `text + suffix` within `max_width`
pub fn truncate_by_width(text: &str, max_width: usize, suffix: &str) -> String {
    use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

    let text_width = text.width_cjk();
    let suffix_width = suffix.width_cjk();

    if text_width <= max_width {
        return text.to_string();
    }

    if max_width <= suffix_width {
        // Not enough space for suffix, truncate suffix itself
        let mut result = String::new();
        let mut current_width = 0;
        for ch in suffix.chars() {
            let ch_width = ch.width_cjk().unwrap_or(0);
            if current_width + ch_width > max_width {
                break;
            }
            result.push(ch);
            current_width += ch_width;
        }
        return result;
    }

    // Build truncated text to fit within max_width - suffix_width
    let target_width = max_width - suffix_width;
    let mut result = String::new();
    let mut current_width = 0;

    for ch in text.chars() {
        let ch_width = ch.width_cjk().unwrap_or(0);
        if current_width + ch_width > target_width {
            break;
        }
        result.push(ch);
        current_width += ch_width;
    }

    result.push_str(suffix);
    result
}

#[cfg(test)]
#[path = "text_test.rs"]
mod tests;
