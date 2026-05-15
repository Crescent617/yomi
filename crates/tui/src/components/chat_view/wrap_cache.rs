//! Wrap height cache with prefix-sum for O(log n) visual↔logical mapping.

use std::sync::Arc;

use tuirealm::ratatui::text::Line;

use crate::utils::text::calc_wrap_boundaries;

/// Caches visual row heights for a slice of logical lines.
///
/// `heights[i]` = how many visual rows logical line `i` occupies when wrapped.
/// `prefix[i]`  = cumulative visual rows up to and including line `i`.
///
/// All lookups are O(1); finding the logical line at a given visual row is
/// O(log n) via binary search on the prefix sum.
#[derive(Debug)]
pub struct WrapCache {
    heights: Vec<usize>,
    prefix: Vec<usize>,
    width: usize,
}

impl Default for WrapCache {
    fn default() -> Self {
        Self::new()
    }
}

impl WrapCache {
    pub fn new() -> Self {
        Self {
            heights: Vec::new(),
            prefix: Vec::new(),
            width: 0,
        }
    }

    pub fn clear(&mut self) {
        self.heights.clear();
        self.prefix.clear();
        self.width = 0;
    }

    /// Rebuild cache for the given lines and width (single `O(total_chars)` pass).
    pub fn rebuild(&mut self, lines: &[Arc<Line<'static>>], width: usize) {
        self.heights.clear();
        self.prefix.clear();
        self.width = width;

        let mut sum = 0;
        for line in lines {
            let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let h = calc_wrap_boundaries(&text, width).len().max(1);
            self.heights.push(h);
            sum += h;
            self.prefix.push(sum);
        }
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
    #[allow(dead_code)]
    pub fn visual_to_logical(&self, visual_row: usize) -> (usize, usize) {
        let idx = self.prefix.partition_point(|&s| s <= visual_row);
        let prev = if idx == 0 { 0 } else { self.prefix[idx - 1] };
        (idx, visual_row - prev)
    }

    /// Given a global visual scroll offset, return:
    /// `(viewport_start_logical_line, first_row_offset_within_that_line)`.
    pub fn viewport_start(&self, visual_scroll: usize) -> (usize, usize) {
        let line = self.prefix.partition_point(|&s| s <= visual_scroll);
        let offset = if line == 0 {
            visual_scroll
        } else {
            visual_scroll - self.prefix[line - 1]
        };
        (line, offset)
    }

    /// Visual height of a single logical line.
    pub fn height(&self, logical_line: usize) -> usize {
        self.heights.get(logical_line).copied().unwrap_or(1)
    }

    #[allow(dead_code)]
    pub fn len(&self) -> usize {
        self.heights.len()
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.heights.is_empty()
    }
}
