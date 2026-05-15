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

    /// Rebuild cache.
    ///
    /// `prefix_len` = number of leading lines that are guaranteed unchanged
    /// from the previous frame (e.g. banner + history messages).  When
    /// `width` is stable and `prefix_len` ≤ current cache length, only the
    /// suffix after `prefix_len` is recomputed.
    ///
    /// Typical callers:
    /// - Scroll without content change → `prefix_len` = number of stable lines (skip).
    /// - Streaming (history stable, suffix grows) → `prefix_len` = `history_end`.
    /// - Resize / new message → `prefix_len` = 0 (full rebuild).
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

        if lines.len() == self.heights.len() {
            // Nothing changed.
            return;
        }

        // Reuse prefix, rebuild suffix.
        self.heights.truncate(safe_prefix);
        self.prefix.truncate(safe_prefix);
        for line in lines.iter().skip(safe_prefix) {
            self.push_line(line, width);
        }
    }

    fn push_line(&mut self, line: &Arc<Line<'static>>, width: usize) {
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let h = calc_wrap_boundaries(&text, width).len().max(1);
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
