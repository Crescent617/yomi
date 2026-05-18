//! Wrap info cache with prefix-sum and cached boundaries for O(log n) visual↔logical mapping.

use std::sync::Arc;

use tuirealm::ratatui::text::Line;

use crate::utils::text::calc_wrap_boundaries;

/// Caches visual row heights, wrap boundaries, and prefix sum for a slice of logical lines.
///
/// `heights[i]`            = how many visual rows logical line `i` occupies when wrapped.
/// `boundaries[i]`         = byte indices where each visual row starts for line `i`.
/// `char_boundaries[i]`    = character indices where each visual row starts for line `i`.
/// `char_counts[i]`        = total character count for line `i`.
/// `span_char_counts[i]`   = per-span character counts for line `i`.
/// `prefix[i]`             = cumulative visual rows up to and including line `i`.
///
/// All lookups are O(1); finding the logical line at a given visual row is
/// O(log n) via binary search on the prefix sum.
#[derive(Debug)]
pub struct WrapInfo {
    heights: Vec<usize>,
    boundaries: Vec<Vec<usize>>,
    char_boundaries: Vec<Vec<usize>>,
    char_counts: Vec<usize>,
    span_char_counts: Vec<Vec<usize>>,
    prefix: Vec<usize>,
    width: usize,
}

impl Default for WrapInfo {
    fn default() -> Self {
        Self::new()
    }
}

impl WrapInfo {
    pub fn new() -> Self {
        Self {
            heights: Vec::new(),
            boundaries: Vec::new(),
            char_boundaries: Vec::new(),
            char_counts: Vec::new(),
            span_char_counts: Vec::new(),
            prefix: Vec::new(),
            width: 0,
        }
    }

    pub fn clear(&mut self) {
        self.heights.clear();
        self.boundaries.clear();
        self.char_boundaries.clear();
        self.char_counts.clear();
        self.span_char_counts.clear();
        self.prefix.clear();
        self.width = 0;
    }

    /// Rebuild cache.
    ///
    /// `prefix_len` = number of leading lines that are guaranteed unchanged
    /// from the previous frame. When `width` is stable and `prefix_len` ≤ current
    /// cache length, only the suffix after `prefix_len` is recomputed.
    pub fn rebuild(&mut self, lines: &[Arc<Line<'static>>], width: usize, prefix_len: usize) {
        // Safety guard: prefix must not exceed current cache or new lines.
        let safe_prefix = prefix_len.min(self.heights.len()).min(lines.len());

        if width != self.width || lines.len() < self.heights.len() || safe_prefix == 0 {
            self.clear();
            self.width = width;
            for line in lines {
                self.push_line(line, width);
            }
            return;
        }

        // Only skip if the stable prefix already covers the entire buffer.
        if safe_prefix >= lines.len() {
            return;
        }

        // Reuse prefix, rebuild suffix.
        self.heights.truncate(safe_prefix);
        self.boundaries.truncate(safe_prefix);
        self.char_boundaries.truncate(safe_prefix);
        self.char_counts.truncate(safe_prefix);
        self.span_char_counts.truncate(safe_prefix);
        self.prefix.truncate(safe_prefix);
        for line in lines.iter().skip(safe_prefix) {
            self.push_line(line, width);
        }
    }

    fn push_line(&mut self, line: &Arc<Line<'static>>, width: usize) {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let byte_boundaries = calc_wrap_boundaries(&text, width);

        // Pre-compute char-level data to avoid repeated byte→char conversions
        let char_boundaries: Vec<usize> = byte_boundaries
            .iter()
            .map(|&b| text[..b.min(text.len())].chars().count())
            .collect();
        let char_count = text.chars().count();
        let span_counts: Vec<usize> = line
            .spans
            .iter()
            .map(|s| s.content.chars().count())
            .collect();

        let h = byte_boundaries.len().max(1);
        self.boundaries.push(byte_boundaries);
        self.char_boundaries.push(char_boundaries);
        self.char_counts.push(char_count);
        self.span_char_counts.push(span_counts);
        self.heights.push(h);
        let sum = self.prefix.last().copied().unwrap_or(0) + h;
        self.prefix.push(sum);
    }

    /// Total visual lines (O(1)).
    pub fn total_lines(&self) -> usize {
        self.prefix.last().copied().unwrap_or(0)
    }

    /// Starting visual row for the given logical line (O(1)).
    pub fn logical_to_visual(&self, logical_line: usize) -> usize {
        if logical_line == 0 {
            0
        } else {
            self.prefix.get(logical_line - 1).copied().unwrap_or(0)
        }
    }

    /// Convert a visual row to (`logical_line`, `row_within_line`) (`O(log n)`).
    pub fn visual_to_logical(&self, visual_row: usize) -> (usize, usize) {
        let idx = self.prefix.partition_point(|&s| s <= visual_row);
        let prev = if idx == 0 { 0 } else { self.prefix[idx - 1] };
        (idx, visual_row - prev)
    }

    /// Given a global visual scroll offset, return:
    /// `(viewport_start_logical_line, first_row_offset_within_that_line)`.
    pub fn viewport_start(&self, visual_scroll: usize) -> (usize, usize) {
        self.visual_to_logical(visual_scroll)
    }

    /// Visual height of a single logical line.
    pub fn height(&self, logical_line: usize) -> usize {
        self.heights.get(logical_line).copied().unwrap_or(1)
    }

    /// Cached byte wrap boundaries for a single logical line.
    pub fn get_boundaries(&self, logical_line: usize) -> Option<&[usize]> {
        self.boundaries.get(logical_line).map(|b| b.as_slice())
    }

    /// Cached character wrap boundaries for a single logical line.
    pub fn get_char_boundaries(&self, logical_line: usize) -> Option<&[usize]> {
        self.char_boundaries.get(logical_line).map(|b| b.as_slice())
    }

    /// Total character count for a single logical line.
    pub fn char_count(&self, logical_line: usize) -> usize {
        self.char_counts.get(logical_line).copied().unwrap_or(0)
    }

    /// Per-span character counts for a single logical line.
    pub fn get_span_char_counts(&self, logical_line: usize) -> Option<&[usize]> {
        self.span_char_counts
            .get(logical_line)
            .map(|v| v.as_slice())
    }

    pub fn len(&self) -> usize {
        self.heights.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_info_basic() {
        let mut info = WrapInfo::new();
        let lines = vec![Arc::new(Line::from("Hello World"))];
        info.rebuild(&lines, 5, 0);
        // "Hello World" at width 5 wraps to 3 visual rows
        assert_eq!(info.total_lines(), 3);
        assert_eq!(info.height(0), 3);
        assert_eq!(info.visual_to_logical(0), (0, 0));
        assert_eq!(info.visual_to_logical(2), (0, 2));
        assert_eq!(info.visual_to_logical(3), (1, 0)); // past last visual row of line 0
        assert_eq!(info.viewport_start(2), (0, 2));
    }

    #[test]
    fn test_wrap_info_cjk() {
        let mut info = WrapInfo::new();
        let lines = vec![Arc::new(Line::from("你好世界"))];
        info.rebuild(&lines, 4, 0);
        // CJK chars are width 2, so 4 chars = 2 visual rows at width 4
        assert_eq!(info.total_lines(), 2);
        assert_eq!(info.height(0), 2);
        let boundaries = info.get_boundaries(0).unwrap();
        assert_eq!(boundaries, &[0, 6]); // 4 CJK chars, 3 bytes each, 2 per row at width 4
        let char_boundaries = info.get_char_boundaries(0).unwrap();
        assert_eq!(char_boundaries, &[0, 2]);
        assert_eq!(info.char_count(0), 4);
        assert_eq!(info.get_span_char_counts(0).unwrap(), &[4]);
    }

    #[test]
    fn test_wrap_info_incremental_rebuild() {
        let mut info = WrapInfo::new();
        let lines = vec![Arc::new(Line::from("Hello"))];
        info.rebuild(&lines, 5, 0);
        assert_eq!(info.total_lines(), 1);

        let lines2 = vec![Arc::new(Line::from("Hello")), Arc::new(Line::from("World"))];
        info.rebuild(&lines2, 5, 1);
        assert_eq!(info.total_lines(), 2);
    }
}
