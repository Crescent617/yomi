//! Custom Paragraph widget with character-level wrapping
//!
//! This widget provides consistent wrap behavior between layout calculation
//! and rendering, solving the mismatch between manual wrap logic and
//! ratatui's `Paragraph::wrap()`.

#![allow(clippy::unused_self, clippy::too_many_arguments)]

use tuirealm::ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::Widget,
};
use unicode_width::UnicodeWidthChar;

/// Selection range: ((`start_line`, `start_col`), (`end_line`, `end_col`))
pub type SelectionRange = ((usize, usize), (usize, usize));

/// A Paragraph-like widget with custom character-level wrapping logic.
///
/// Unlike ratatui's Paragraph which uses its own wrap algorithm,
/// this widget uses a Unicode-aware character width algorithm that
/// matches the application's scroll calculations.
pub struct WrapParagraph<'a> {
    text: Text<'a>,
    scroll: (u16, u16),
    selection: Option<SelectionRange>,
    highlight_style: Style,
}

impl<'a> WrapParagraph<'a> {
    /// Create a new `WrapParagraph` with the given text.
    pub fn new(text: impl Into<Text<'a>>) -> Self {
        Self {
            text: text.into(),
            scroll: (0, 0),
            selection: None,
            highlight_style: Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        }
    }

    /// Calculate the total number of visual lines for the given text and width.
    /// This is a convenience method that doesn't require creating a `WrapParagraph` instance.
    pub fn wrapped_line_count_of(text: &Text<'_>, width: usize) -> usize {
        if width == 0 {
            return text.lines.len();
        }

        let temp = Self::new(Text::from(""));
        text.lines
            .iter()
            .map(|line| temp.wrap_line_height(line, width))
            .sum()
    }

    /// Set the scroll offset in (y, x) direction.
    #[must_use]
    pub fn scroll(mut self, offset: (u16, u16)) -> Self {
        self.scroll = offset;
        self
    }

    /// Set the selection range for highlighting.
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

    /// Calculate the total number of visual lines when wrapped at the given width.
    pub fn wrapped_line_count(&self, width: usize) -> usize {
        if width == 0 {
            return self.text.lines.len();
        }

        self.text
            .lines
            .iter()
            .map(|line| self.wrap_line_height(line, width))
            .sum()
    }

    /// Calculate how many visual rows a single line occupies when wrapped.
    fn wrap_line_height(&self, line: &Line<'_>, width: usize) -> usize {
        if width == 0 {
            return 1;
        }

        let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        if line_text.is_empty() {
            return 1;
        }

        let boundaries = Self::calculate_wrap_boundaries(&line_text, width);
        boundaries.len()
    }

    /// Calculate character indices where each visual row starts.
    ///
    /// Returns a vector of byte indices into the UTF-8 string where
    /// wrapping should occur. The first element is always 0.
    fn calculate_wrap_boundaries(text: &str, width: usize) -> Vec<usize> {
        if width == 0 || text.is_empty() {
            return vec![0];
        }

        let mut boundaries = vec![0];
        let mut current_width = 0;
        let mut byte_idx = 0;

        for ch in text.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);

            // If adding this character would exceed width, wrap here
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

    /// Extract a segment of a line as a new Line, preserving styles.
    fn extract_line_segment<'b>(line: &Line<'b>, start_byte: usize, end_byte: usize) -> Line<'b> {
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
                let extracted = &span_text[overlap_start..overlap_end];
                spans.push(Span::styled(extracted.to_string(), span.style));
            }

            current_byte = span_end;
        }

        Line::from(spans).style(line.style)
    }

    /// Render a line segment with selection highlighting.
    ///
    /// Simplified "Extract-Then-Style" approach:
    /// 1. Extract the wrap segment text from spans
    /// 2. Apply selection styles to create new spans
    /// 3. Render using standard `render_line`
    fn render_line_with_selection(
        &self,
        line: &Line<'_>,
        start_byte: usize,
        end_byte: usize,
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
            let segment = Self::extract_line_segment(line, start_byte, end_byte);
            render_line(&segment, x_start, y, max_width, buf);
            return;
        }

        // Get full line text for byte-to-char conversion
        let full_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();

        // Convert byte bounds to char bounds
        let safe_start = start_byte.min(full_text.len());
        let safe_end = end_byte.min(full_text.len());
        let start_char = full_text[..safe_start].chars().count();
        let end_char = full_text[..safe_end].chars().count();

        // Calculate selection range within this line (in character indices)
        let line_sel_start = if global_line_idx == sel_start_line {
            sel_start_col
        } else {
            0
        };
        let line_sel_end = if global_line_idx == sel_end_line {
            sel_end_col
        } else {
            full_text.chars().count()
        };

        // Clamp selection to this wrap segment
        let seg_sel_start = line_sel_start.clamp(start_char, end_char);
        let seg_sel_end = line_sel_end.clamp(start_char, end_char);

        // If no selection in this segment, render normally
        if seg_sel_start >= seg_sel_end {
            let segment = Self::extract_line_segment(line, start_byte, end_byte);
            render_line(&segment, x_start, y, max_width, buf);
            return;
        }

        // Build styled spans for this wrap segment
        let mut styled_spans = Vec::new();
        let mut current_char = 0;

        for span in &line.spans {
            let span_text = span.content.as_ref();
            let span_char_count = span_text.chars().count();
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

            // Extract text for this wrap segment portion
            let wrap_text: String = span_text
                .chars()
                .skip(wrap_start_in_span)
                .take(wrap_end_in_span.saturating_sub(wrap_start_in_span))
                .collect();

            // Calculate where this portion starts in global line chars
            let this_start_global = span_start_char + wrap_start_in_span;
            let base_style = span.style.patch(line.style);

            // Calculate selection overlap within this extracted text
            let sel_start_rel = seg_sel_start.saturating_sub(this_start_global);
            let sel_end_rel = seg_sel_end
                .saturating_sub(this_start_global)
                .min(wrap_text.chars().count());

            if sel_start_rel >= sel_end_rel {
                // No selection in this span portion
                styled_spans.push(Span::styled(wrap_text, base_style));
            } else {
                // Split into before/selected/after
                let before: String = wrap_text.chars().take(sel_start_rel).collect();
                let selected: String = wrap_text
                    .chars()
                    .skip(sel_start_rel)
                    .take(sel_end_rel.saturating_sub(sel_start_rel))
                    .collect();
                let after: String = wrap_text.chars().skip(sel_end_rel).collect();

                if !before.is_empty() {
                    styled_spans.push(Span::styled(before, base_style));
                }
                if !selected.is_empty() {
                    styled_spans.push(Span::styled(selected, self.highlight_style));
                }
                if !after.is_empty() {
                    styled_spans.push(Span::styled(after, base_style));
                }
            }

            current_char = span_end_char;
        }

        let styled_line = Line::from(styled_spans).style(line.style);
        render_line(&styled_line, x_start, y, max_width, buf);
    }

    /// Convert visual row index to (`line_idx`, `row_within_line`).
    ///
    /// Used for mapping screen coordinates to text positions.
    pub fn visual_row_to_line(&self, visual_row: usize, width: usize) -> Option<(usize, usize)> {
        if width == 0 {
            return Some((visual_row, 0));
        }

        let mut current_row = 0;
        for (line_idx, line) in self.text.lines.iter().enumerate() {
            let wrapped_height = self.wrap_line_height(line, width);

            if visual_row < current_row + wrapped_height {
                return Some((line_idx, visual_row - current_row));
            }

            current_row += wrapped_height;
        }

        None
    }

    /// Get the visual row range for a given line index.
    pub fn line_to_visual_row(&self, line_idx: usize, width: usize) -> Option<usize> {
        if width == 0 {
            return Some(line_idx);
        }

        let mut current_row = 0;
        for (idx, line) in self.text.lines.iter().enumerate() {
            if idx == line_idx {
                return Some(current_row);
            }
            current_row += self.wrap_line_height(line, width);
        }

        None
    }
}

impl Widget for WrapParagraph<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.width == 0 || area.height == 0 {
            return;
        }

        let width = area.width as usize;
        let height = area.height as usize;
        let scroll_y = self.scroll.0 as usize;

        // Normalize selection
        let selection = self.selection.map(|((sl, sc), (el, ec))| {
            if sl < el || (sl == el && sc <= ec) {
                ((sl, sc), (el, ec))
            } else {
                ((el, ec), (sl, sc))
            }
        });

        let mut visual_row = 0;

        for (global_line_idx, line) in self.text.lines.iter().enumerate() {
            let line_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            let boundaries = Self::calculate_wrap_boundaries(&line_text, width);

            // Render each wrapped row of this line
            for (row_in_line, &start_byte) in boundaries.iter().enumerate() {
                let end_byte = boundaries
                    .get(row_in_line + 1)
                    .copied()
                    .unwrap_or(line_text.len());

                // Check if this visual row is visible
                if visual_row >= scroll_y && visual_row < scroll_y + height {
                    let y = area.y + (visual_row - scroll_y) as u16;

                    // Check if this line is within selection
                    let is_selected_line = selection.is_some_and(|((sl, _), (el, _))| {
                        global_line_idx >= sl && global_line_idx <= el
                    });

                    if is_selected_line {
                        // Render with selection highlighting
                        self.render_line_with_selection(
                            line,
                            start_byte,
                            end_byte,
                            global_line_idx,
                            area.x,
                            y,
                            area.width,
                            buf,
                            selection.unwrap(),
                        );
                    } else {
                        // Render normal line
                        let row_line = Self::extract_line_segment(line, start_byte, end_byte);
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
        // Pass remaining width, not original max_width, to prevent buffer overflow
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
                // Apply to previous cell
                buf[(x - 1, y)].set_style(style);
            }
            continue;
        }

        // Check if wide character would overflow (needs ch_width cells but only 1 available)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wrap_boundaries_ascii() {
        let boundaries = WrapParagraph::calculate_wrap_boundaries("Hello World", 5);
        assert_eq!(boundaries, vec![0, 5, 10]);
    }

    #[test]
    fn test_wrap_boundaries_cjk() {
        // CJK characters are width 2, 3 bytes each in UTF-8
        // "你好世界" at width 4 fits 2 chars per row
        let boundaries = WrapParagraph::calculate_wrap_boundaries("你好世界", 4);
        assert_eq!(boundaries, vec![0, 6]); // Row 1: bytes 0-5, Row 2: bytes 6-11
    }

    #[test]
    fn test_wrap_line_count() {
        let para = WrapParagraph::new(Text::from("Hello World\nSecond line"));
        // "Hello World" (11 chars) wraps to 3 lines at width 5
        // "Second line" (11 chars) wraps to 3 lines at width 5
        assert_eq!(para.wrapped_line_count(5), 6);
    }
}
