//! Custom Paragraph widget with character-level wrapping
//!
//! Receives zero-copy borrow of lines + pre-computed wrap info.

#![allow(clippy::unused_self, clippy::too_many_arguments)]

use std::sync::Arc;

use tuirealm::ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::components::wrap_info::WrapInfo;
use crate::utils::text::{char_idx_to_byte_idx, extract_line_segment};
use unicode_width::UnicodeWidthChar;

/// Selection range: ((`start_line`, `start_col`), (`end_line`, `end_col`))
pub type SelectionRange = ((usize, usize), (usize, usize));

/// A Paragraph-like widget with custom character-level wrapping logic.
///
/// Borrows lines and wrap info instead of owning text, avoiding per-frame clones.
pub struct WrapParagraph<'a> {
    lines: &'a [Arc<Line<'static>>],
    info: &'a WrapInfo,
    scroll_y: usize,
    selection: Option<SelectionRange>,
    highlight_style: Style,
}

impl<'a> WrapParagraph<'a> {
    /// Create a new `WrapParagraph` borrowing lines and wrap info.
    pub fn new(lines: &'a [Arc<Line<'static>>], info: &'a WrapInfo) -> Self {
        Self {
            lines,
            info,
            scroll_y: 0,
            selection: None,
            highlight_style: Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        }
    }

    /// Set the visual-row scroll offset.
    #[must_use]
    pub fn scroll_y(mut self, offset: usize) -> Self {
        self.scroll_y = offset;
        self
    }

    /// Set the selection range for highlighting (global line indices).
    #[must_use]
    pub fn selection(mut self, selection: Option<SelectionRange>) -> Self {
        self.selection = selection;
        self
    }

    /// Set the highlight style for selection.
    #[must_use]
    pub fn highlight_style(mut self, style: Style) -> Self {
        self.highlight_style = style;
        self
    }

    /// Render a line segment with selection highlighting.
    ///
    /// All char↔byte conversions are served from `WrapInfo` caches; no per-frame
    /// `.chars().count()` or `.chars().skip().take()` scans remain.
    fn render_line_with_selection(
        &self,
        line: &Line<'_>,
        start_byte: usize,
        end_byte: usize,
        start_char: usize,
        end_char: usize,
        global_line_idx: usize,
        x_start: u16,
        y: u16,
        max_width: u16,
        buf: &mut Buffer,
        selection: SelectionRange,
    ) {
        let ((sel_start_line, sel_start_col), (sel_end_line, sel_end_col)) = selection;

        // If this line is not in the selection range, render normally
        if global_line_idx < sel_start_line || global_line_idx > sel_end_line {
            let segment = extract_line_segment(line, start_byte, end_byte);
            render_line(&segment, x_start, y, max_width, buf);
            return;
        }

        let char_count = self.info.char_count(global_line_idx);

        // Calculate selection range within this line (in character indices)
        let line_sel_start = if global_line_idx == sel_start_line {
            sel_start_col
        } else {
            0
        };
        let line_sel_end = if global_line_idx == sel_end_line {
            sel_end_col
        } else {
            char_count
        };

        // Clamp selection to this wrap segment
        let seg_sel_start = line_sel_start.clamp(start_char, end_char);
        let seg_sel_end = line_sel_end.clamp(start_char, end_char);

        // If no selection in this segment, render normally
        if seg_sel_start >= seg_sel_end {
            let segment = extract_line_segment(line, start_byte, end_byte);
            render_line(&segment, x_start, y, max_width, buf);
            return;
        }

        // Build styled spans for this wrap segment
        let mut styled_spans = Vec::new();
        let mut current_char = 0;
        let span_char_counts = self
            .info
            .get_span_char_counts(global_line_idx)
            .unwrap_or(&[]);

        for (span, &span_char_count) in line.spans.iter().zip(span_char_counts.iter()) {
            let span_text = span.content.as_ref();
            let span_start_char = current_char;
            let span_end_char = current_char + span_char_count;

            // Skip spans completely outside the wrap segment
            if span_end_char <= start_char || span_start_char >= end_char {
                current_char = span_end_char;
                continue;
            }

            // Calculate overlap with wrap segment (in chars)
            let wrap_start_in_span = start_char.saturating_sub(span_start_char);
            let wrap_end_in_span = end_char
                .saturating_sub(span_start_char)
                .min(span_char_count);

            // Extract wrap portion via byte indices (zero allocation vs .chars().skip().take())
            let wrap_start_byte = char_idx_to_byte_idx(span_text, wrap_start_in_span);
            let wrap_end_byte = char_idx_to_byte_idx(span_text, wrap_end_in_span);
            let wrap_text = &span_text[wrap_start_byte..wrap_end_byte];

            // Calculate where this portion starts in global line chars
            let this_start_global = span_start_char + wrap_start_in_span;
            let base_style = span.style.patch(line.style);

            // Selection overlap within this extracted text
            let sel_start_rel = seg_sel_start.saturating_sub(this_start_global);
            let sel_end_rel = seg_sel_end
                .saturating_sub(this_start_global)
                .min(wrap_end_in_span.saturating_sub(wrap_start_in_span));

            if sel_start_rel >= sel_end_rel {
                styled_spans.push(Span::styled(wrap_text.to_string(), base_style));
            } else {
                // Split into before/selected/after using byte indices
                let before_end = char_idx_to_byte_idx(wrap_text, sel_start_rel);
                let selected_end = char_idx_to_byte_idx(wrap_text, sel_end_rel);

                let before = &wrap_text[..before_end];
                let selected = &wrap_text[before_end..selected_end];
                let after = &wrap_text[selected_end..];

                if !before.is_empty() {
                    styled_spans.push(Span::styled(before.to_string(), base_style));
                }
                if !selected.is_empty() {
                    styled_spans.push(Span::styled(selected.to_string(), self.highlight_style));
                }
                if !after.is_empty() {
                    styled_spans.push(Span::styled(after.to_string(), base_style));
                }
            }

            current_char = span_end_char;
        }

        let styled_line = Line::from(styled_spans).style(line.style);
        render_line(&styled_line, x_start, y, max_width, buf);
    }
}

impl Widget for WrapParagraph<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 || self.lines.is_empty() {
            return;
        }

        debug_assert!(
            self.lines.len() == self.info.len(),
            "lines ({}) and wrap_info ({}) out of sync",
            self.lines.len(),
            self.info.len()
        );

        let _width = area.width as usize;
        let height = area.height as usize;
        let scroll_y = self.scroll_y;

        // Normalize selection
        let selection = self.selection.map(|((sl, sc), (el, ec))| {
            if sl < el || (sl == el && sc <= ec) {
                ((sl, sc), (el, ec))
            } else {
                ((el, ec), (sl, sc))
            }
        });

        let mut visual_row = 0;

        for (global_line_idx, line) in self.lines.iter().enumerate() {
            let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let boundaries = self.info.get_boundaries(global_line_idx).unwrap_or(&[0]);
            let char_boundaries = self
                .info
                .get_char_boundaries(global_line_idx)
                .unwrap_or(&[0]);
            let char_count = self.info.char_count(global_line_idx);

            // Render each wrapped row of this line
            for (row_in_line, (&start_byte, &start_char)) in
                boundaries.iter().zip(char_boundaries.iter()).enumerate()
            {
                let end_byte = boundaries
                    .get(row_in_line + 1)
                    .copied()
                    .unwrap_or(line_text.len());
                let end_char = char_boundaries
                    .get(row_in_line + 1)
                    .copied()
                    .unwrap_or(char_count);

                // Check if this visual row is visible
                if visual_row >= scroll_y && visual_row < scroll_y + height {
                    let y = area.y + (visual_row - scroll_y) as u16;

                    // Check if this line is within selection
                    let is_selected_line = selection.is_some_and(|((sl, _), (el, _))| {
                        global_line_idx >= sl && global_line_idx <= el
                    });

                    if is_selected_line {
                        self.render_line_with_selection(
                            line,
                            start_byte,
                            end_byte,
                            start_char,
                            end_char,
                            global_line_idx,
                            area.x,
                            y,
                            area.width,
                            buf,
                            selection.unwrap(),
                        );
                    } else {
                        let row_line = extract_line_segment(line, start_byte, end_byte);
                        render_line(&row_line, area.x, y, area.width, buf);
                    }
                }

                visual_row += 1;
            }
        }

        // Clear remaining area
        let rendered_rows = visual_row.saturating_sub(scroll_y);
        for y_offset in rendered_rows..height {
            let y_pos = area.y + y_offset as u16;
            if y_pos < area.y + area.height {
                for x in 0..area.width {
                    let x_pos = area.x + x;
                    buf[(x_pos, y_pos)].reset();
                }
            }
        }
    }
}

/// Render a single line to the buffer at the given position.
fn render_line(line: &Line<'_>, x_start: u16, y: u16, max_width: u16, buf: &mut Buffer) {
    let mut x = x_start;
    let max_x = x_start + max_width;

    for span in &line.spans {
        let style = span.style.patch(line.style);
        let remaining_width = max_x.saturating_sub(x);
        x = render_text(span.content.as_ref(), x, y, remaining_width, buf, style);
        if x >= max_x {
            return;
        }
    }
}

/// Render text to buffer and return the new x position.
fn render_text(
    text: &str,
    x_start: u16,
    y: u16,
    max_width: u16,
    buf: &mut Buffer,
    style: Style,
) -> u16 {
    let mut x = x_start;
    let max_x = x_start.saturating_add(max_width);

    for ch in text.chars() {
        if x >= max_x {
            break;
        }

        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(1) as u16;

        // Handle zero-width characters
        if ch_width == 0 {
            if x > x_start {
                buf[(x - 1, y)].set_style(style);
            }
            continue;
        }

        // Check if wide character would overflow
        if x.saturating_add(ch_width) > max_x {
            break;
        }

        buf[(x, y)].set_char(ch).set_style(style);

        // Fill wide character continuation cells
        for offset in 1..ch_width {
            buf[(x + offset, y)].set_char(' ').set_style(style);
        }

        x += ch_width;
    }

    x
}
