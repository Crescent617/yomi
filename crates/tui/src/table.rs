//! Markdown table rendering for TUI
//!
//! Supports streaming (incremental) rendering and complete table rendering.

use crate::theme::colors;
use tuirealm::ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};

/// Cell alignment in a table
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum CellAlign {
    #[default]
    Left,
    Center,
    Right,
}

impl CellAlign {
    /// Format content with alignment for given width
    #[allow(clippy::trivially_copy_pass_by_ref)]
    fn format(&self, content: &str, width: usize) -> String {
        let content_width = unicode_width::UnicodeWidthStr::width(content);
        if content_width >= width {
            return content.to_string();
        }
        let padding = width - content_width;
        match self {
            CellAlign::Left => format!("{}{}", content, " ".repeat(padding)),
            CellAlign::Right => format!("{}{}", " ".repeat(padding), content),
            CellAlign::Center => {
                let right = padding / 2;
                let left = padding - right;
                format!("{}{}{}", " ".repeat(left), content, " ".repeat(right))
            }
        }
    }
}

/// Wrap text into multiple lines based on display width.
/// Respects existing newlines and word boundaries when possible.
fn wrap_text_to_width(text: &str, max_width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut lines = Vec::new();

    for line in text.lines() {
        let mut current_line = String::new();
        let mut current_width = 0;
        let mut pending_space = false;

        // Try to wrap at word boundaries
        let mut word_buffer = String::new();
        let mut word_width = 0;

        for ch in line.chars() {
            let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);

            if ch.is_whitespace() {
                // End of a word
                if !word_buffer.is_empty() {
                    // Check if word + pending space fits in current line
                    let space_width = usize::from(pending_space);
                    if current_width + space_width + word_width > max_width {
                        // Word doesn't fit, flush current line and start new one
                        if !current_line.is_empty() {
                            lines.push(current_line);
                            current_line = String::new();
                            current_width = 0;
                        }
                        // If word itself is longer than max_width, break it
                        if word_width > max_width {
                            lines.extend(break_long_word(&word_buffer, max_width));
                            pending_space = false;
                        } else {
                            current_line.clone_from(&word_buffer);
                            current_width = word_width;
                            pending_space = true;
                        }
                    } else {
                        // Word fits, add it with pending space
                        if pending_space {
                            current_line.push(' ');
                            current_width += 1;
                        }
                        current_line.push_str(&word_buffer);
                        current_width += word_width;
                        pending_space = true;
                    }
                    word_buffer.clear();
                    word_width = 0;
                }
                // Mark that we have a pending space (but don't add it yet)
                if !current_line.is_empty() {
                    pending_space = true;
                }
            } else {
                // Part of a word
                word_buffer.push(ch);
                word_width += ch_width;
            }
        }

        // Handle remaining word buffer
        if !word_buffer.is_empty() {
            let space_width = usize::from(pending_space);
            if current_width + space_width + word_width > max_width {
                // Flush current line first
                if !current_line.is_empty() {
                    lines.push(current_line);
                }
                // If word itself is longer than max_width, break it
                if word_width > max_width {
                    lines.extend(break_long_word(&word_buffer, max_width));
                } else {
                    lines.push(word_buffer);
                }
            } else {
                if pending_space {
                    current_line.push(' ');
                }
                current_line.push_str(&word_buffer);
                lines.push(current_line);
            }
        } else if !current_line.is_empty() {
            lines.push(current_line);
        } else {
            lines.push(String::new());
        }
    }

    if lines.is_empty() {
        lines.push(String::new());
    }

    lines
}

/// Break a long word into chunks that fit within `max_width`
fn break_long_word(word: &str, max_width: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current_chunk = String::new();
    let mut current_width = 0;

    for ch in word.chars() {
        let ch_width = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);

        if current_width + ch_width > max_width && !current_chunk.is_empty() {
            chunks.push(current_chunk);
            current_chunk = String::new();
            current_width = 0;
        }

        current_chunk.push(ch);
        current_width += ch_width;
    }

    if !current_chunk.is_empty() {
        chunks.push(current_chunk);
    }

    chunks
}

/// A row in the table
#[derive(Debug, Clone)]
pub struct TableRow {
    pub cells: Vec<String>,
    pub is_header: bool,
}

/// Complete table for history rendering
#[derive(Debug, Clone)]
pub struct Table {
    pub header: Option<TableRow>,
    pub rows: Vec<TableRow>,
    pub aligns: Vec<CellAlign>,
}

impl Table {
    /// Parse table from markdown content
    pub fn from_events(events: &[pulldown_cmark::Event]) -> Option<Self> {
        let mut rows = Vec::new();
        let mut aligns = Vec::new();
        let mut current_row: Vec<String> = Vec::new();
        let mut current_cell = String::new();
        let mut in_table_head = false;
        for event in events {
            use pulldown_cmark::Event as MdEvent;
            use pulldown_cmark::Tag;
            use pulldown_cmark::TagEnd;

            match event {
                MdEvent::End(TagEnd::Table) => {
                    break;
                }
                MdEvent::Start(Tag::TableHead) => {
                    in_table_head = true;
                }
                MdEvent::End(TagEnd::TableHead) => {
                    in_table_head = false;
                }
                MdEvent::Start(Tag::TableRow) => {
                    current_row.clear();
                    current_cell.clear();
                }
                MdEvent::End(TagEnd::TableRow) => {
                    if !current_cell.is_empty() {
                        current_row.push(current_cell.trim().to_string());
                        current_cell.clear();
                    }

                    // Check if this is a separator row (contains only dashes and colons)
                    if current_row.iter().all(|cell| {
                        let trimmed = cell.trim();
                        !trimmed.is_empty()
                            && trimmed
                                .chars()
                                .all(|c| c == '-' || c == ':' || c.is_whitespace())
                    }) {
                        aligns = current_row
                            .iter()
                            .map(|cell| parse_align(cell.trim()))
                            .collect();
                    } else if !current_row.is_empty() {
                        rows.push(TableRow {
                            cells: current_row.clone(),
                            is_header: in_table_head,
                        });
                    }
                }
                MdEvent::Start(Tag::TableCell) if !current_cell.is_empty() => {
                    current_row.push(current_cell.trim().to_string());
                    current_cell.clear();
                }
                MdEvent::End(TagEnd::TableCell) => {
                    current_row.push(current_cell.trim().to_string());
                    current_cell.clear();
                }
                MdEvent::Text(text) => {
                    current_cell.push_str(text);
                }
                MdEvent::Code(code) => {
                    current_cell.push_str(code);
                }
                _ => {}
            }
        }

        if rows.is_empty() {
            return None;
        }

        // First row is header if we don't have explicit header
        let header = if rows.first().is_some_and(|r| r.is_header) {
            rows.remove(0).into()
        } else {
            None
        };

        Some(Table {
            header,
            rows,
            aligns,
        })
    }

    /// Calculate optimal column widths
    #[allow(clippy::cast_precision_loss)]
    fn calculate_widths(&self, max_width: usize) -> Vec<usize> {
        let num_cols = self
            .header
            .as_ref()
            .map(|h| h.cells.len())
            .or_else(|| self.rows.first().map(|r| r.cells.len()))
            .unwrap_or(0);

        if num_cols == 0 {
            return Vec::new();
        }

        // Calculate minimum width needed for each column
        let mut widths: Vec<usize> = (0..num_cols)
            .map(|col| {
                let mut max = 3; // Minimum width
                if let Some(header) = &self.header {
                    if let Some(cell) = header.cells.get(col) {
                        max = max.max(unicode_width::UnicodeWidthStr::width(cell.as_str()));
                    }
                }
                for row in &self.rows {
                    if let Some(cell) = row.cells.get(col) {
                        max = max.max(unicode_width::UnicodeWidthStr::width(cell.as_str()));
                    }
                }
                max
            })
            .collect();

        // Account for borders: each column adds 3 chars (" │ "), plus 1 for final "│"
        let border_width = num_cols * 3 + 1;
        let content_width: usize = widths.iter().sum();
        let total_width = content_width + border_width;

        // If too wide, scale down proportionally (but keep at least 3 per column)
        if total_width > max_width && max_width > border_width {
            let available = max_width - border_width;
            let scale = available as f64 / content_width as f64;
            for w in &mut widths {
                *w = ((*w as f64 * scale) as usize).max(3);
            }
        }

        widths
    }

    /// Render table as lines
    pub fn render(&self, max_width: usize) -> Vec<Line<'static>> {
        let widths = self.calculate_widths(max_width);
        if widths.is_empty() {
            return Vec::new();
        }

        let mut lines = Vec::new();

        // Top border
        lines.push(Self::render_horizontal_border(&widths, '┌', '┬', '┐'));

        // Header
        if let Some(header) = &self.header {
            lines.extend(self.render_row(header, &widths, true));
            lines.push(Self::render_horizontal_border(&widths, '├', '┼', '┤'));
        } else if !self.aligns.is_empty() {
            // No header but have aligns from separator row
            lines.push(Self::render_horizontal_border(&widths, '├', '┼', '┤'));
        }

        // Data rows
        for row in &self.rows {
            lines.extend(self.render_row(row, &widths, false));
        }

        // Bottom border
        lines.push(Self::render_horizontal_border(&widths, '└', '┴', '┘'));

        lines
    }

    fn render_row(&self, row: &TableRow, widths: &[usize], is_header: bool) -> Vec<Line<'static>> {
        // Wrap cell content to fit column widths
        let cell_lines: Vec<Vec<String>> = row
            .cells
            .iter()
            .enumerate()
            .map(|(idx, cell)| {
                let width = widths.get(idx).copied().unwrap_or(10);
                wrap_text_to_width(cell, width)
            })
            .collect();

        let max_lines = cell_lines.iter().map(|v| v.len()).max().unwrap_or(1);

        let mut lines = Vec::new();
        for line_idx in 0..max_lines {
            let mut spans = vec![Span::styled("│ ", Style::default().fg(colors::border()))];

            // Render all columns, not just existing cells
            for col_idx in 0..widths.len() {
                let content = cell_lines
                    .get(col_idx)
                    .and_then(|cell| cell.get(line_idx))
                    .map_or("", |s| s.as_str());
                let width = widths[col_idx];
                let align = self.aligns.get(col_idx).copied().unwrap_or_default();

                let formatted = align.format(content, width);
                let style = if is_header {
                    // Header: bold only, no special color
                    Style::default()
                        .fg(colors::text_primary())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(colors::text_primary())
                };

                spans.push(Span::styled(formatted, style));
                // Last column uses " │" (no trailing space), others use " │ "
                let border_str = if col_idx == widths.len() - 1 {
                    " │"
                } else {
                    " │ "
                };
                spans.push(Span::styled(
                    border_str,
                    Style::default().fg(colors::border()),
                ));
            }

            lines.push(Line::from(spans));
        }

        lines
    }

    fn render_horizontal_border(
        widths: &[usize],
        left: char,
        mid: char,
        right: char,
    ) -> Line<'static> {
        let mut content =
            String::with_capacity(widths.iter().sum::<usize>() + widths.len() * 3 + 2);
        content.push(left);

        for (i, width) in widths.iter().enumerate() {
            content.push_str(&"─".repeat(width + 2));
            if i < widths.len() - 1 {
                content.push(mid);
            }
        }
        content.push(right);

        Line::from(Span::styled(content, Style::default().fg(colors::border())))
    }
}

/// Streaming table renderer for incomplete tables
#[derive(Debug, Default)]
pub struct StreamingTableRenderer {
    rows: Vec<TableRow>,
    current_row: Vec<String>,
    current_cell: String,
    aligns: Vec<CellAlign>,
    expecting_separator: bool,
    column_count: Option<usize>,
    in_header: bool,
}

impl StreamingTableRenderer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start a new table
    pub fn start_table(&mut self) {
        *self = Self::default();
    }

    /// Start table head section
    pub fn start_head(&mut self) {
        self.in_header = true;
    }

    /// End table head section
    pub fn end_head(&mut self) {
        // Flush the header row if there's any content
        self.end_row();
        self.in_header = false;
        self.expecting_separator = true;
    }

    /// Start a new row
    pub fn start_row(&mut self) {
        self.current_row.clear();
        self.current_cell.clear();
    }

    /// Start a new cell
    pub fn start_cell(&mut self) {
        if !self.current_cell.is_empty() {
            self.current_row.push(self.current_cell.trim().to_string());
            self.current_cell.clear();
        }
    }

    /// Append text to current cell
    pub fn append_text(&mut self, text: &str) {
        self.current_cell.push_str(text);
    }

    /// End current cell
    pub fn end_cell(&mut self) {
        // Always add cell content (even empty) to maintain column alignment
        self.current_row.push(self.current_cell.trim().to_string());
        self.current_cell.clear();

        if self.column_count.is_none() && !self.current_row.is_empty() {
            self.column_count = Some(self.current_row.len());
        }
    }

    /// End current row
    pub fn end_row(&mut self) {
        // Note: end_cell should be called explicitly for each cell
        // We don't call it here to avoid adding an extra empty cell

        if !self.current_row.is_empty() {
            // Check if this is a separator row
            if self.expecting_separator
                && self.current_row.iter().all(|cell| {
                    let trimmed = cell.trim();
                    !trimmed.is_empty()
                        && trimmed
                            .chars()
                            .all(|c| c == '-' || c == ':' || c.is_whitespace())
                })
            {
                self.aligns = self
                    .current_row
                    .iter()
                    .map(|c| parse_align(c.trim()))
                    .collect();
                self.expecting_separator = false;
            } else {
                self.rows.push(TableRow {
                    cells: self.current_row.clone(),
                    is_header: self.in_header,
                });

                // If we just added the header row, expect separator
                if self.in_header {
                    self.expecting_separator = true;
                }
            }
        }

        self.current_row.clear();
    }

    /// Check if table has any content
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty() && self.current_row.is_empty() && self.current_cell.is_empty()
    }

    /// Render current table state
    pub fn render(&self, max_width: usize) -> Vec<Line<'static>> {
        if self.is_empty() {
            return Vec::new();
        }

        // Calculate column count from all available data
        let mut col_count = self.column_count.unwrap_or(1);

        // Check current row + current cell for additional columns
        let current_row_cols = self.current_row.len() + usize::from(!self.current_cell.is_empty());
        col_count = col_count.max(current_row_cols);

        // Also check completed rows
        for row in &self.rows {
            col_count = col_count.max(row.cells.len());
        }

        col_count = col_count.max(1);
        let widths = self.calculate_widths(col_count, max_width);

        let mut lines = Vec::new();

        // Top border
        lines.push(Self::render_horizontal_border(&widths, '┌', '┬', '┐'));

        // Completed rows
        for (i, row) in self.rows.iter().enumerate() {
            let is_header = row.is_header;
            lines.extend(self.render_row(row, &widths));

            // Add separator after header
            if is_header && !self.expecting_separator {
                lines.push(Self::render_horizontal_border(&widths, '├', '┼', '┤'));
            } else if i < self.rows.len() - 1 {
                // Separator between data rows for better readability in streaming mode
                lines.push(Self::render_horizontal_border(&widths, '├', '┼', '┤'));
            }
        }

        // Current incomplete row
        if !self.current_row.is_empty() || !self.current_cell.is_empty() {
            let mut current = self.current_row.clone();
            if !self.current_cell.is_empty() {
                current.push(self.current_cell.clone());
            }

            let temp_row = TableRow {
                cells: current,
                is_header: self.in_header,
            };
            lines.extend(self.render_row(&temp_row, &widths));
        }

        // Bottom border (use dashed style for incomplete tables)
        if self.rows.is_empty() || (!self.current_row.is_empty() || !self.current_cell.is_empty()) {
            // Table is still being built, use dashed bottom
            lines.push(Self::render_horizontal_border(&widths, '└', '┴', '┘'));
        } else {
            lines.push(Self::render_horizontal_border(&widths, '└', '┴', '┘'));
        }

        lines
    }

    fn render_row(&self, row: &TableRow, widths: &[usize]) -> Vec<Line<'static>> {
        // Wrap cell content to fit column widths
        let cell_lines: Vec<Vec<String>> = row
            .cells
            .iter()
            .enumerate()
            .map(|(idx, cell)| {
                let width = widths.get(idx).copied().unwrap_or(10);
                wrap_text_to_width(cell, width)
            })
            .collect();

        let max_lines = cell_lines.iter().map(|v| v.len()).max().unwrap_or(1);

        let mut lines = Vec::new();
        for line_idx in 0..max_lines {
            let mut spans = vec![Span::styled("│ ", Style::default().fg(colors::border()))];

            // Render all columns, using empty string for missing cells
            for col_idx in 0..widths.len() {
                let content = cell_lines
                    .get(col_idx)
                    .and_then(|c| c.get(line_idx))
                    .map_or("", |s| s.as_str());
                let width = widths[col_idx];
                let align = self.aligns.get(col_idx).copied().unwrap_or_default();

                let formatted = align.format(content, width);
                let style = if row.is_header {
                    // Header: bold only, no special color
                    Style::default()
                        .fg(colors::text_primary())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(colors::text_primary())
                };

                spans.push(Span::styled(formatted, style));
                // Last column uses " │" (no trailing space), others use " │ "
                let border_str = if col_idx == widths.len() - 1 {
                    " │"
                } else {
                    " │ "
                };
                spans.push(Span::styled(
                    border_str,
                    Style::default().fg(colors::border()),
                ));
            }

            lines.push(Line::from(spans));
        }

        lines
    }

    fn render_horizontal_border(
        widths: &[usize],
        left: char,
        mid: char,
        right: char,
    ) -> Line<'static> {
        let mut content =
            String::with_capacity(widths.iter().sum::<usize>() + widths.len() * 3 + 2);
        content.push(left);

        for (i, width) in widths.iter().enumerate() {
            content.push_str(&"─".repeat(width + 2));
            if i < widths.len() - 1 {
                content.push(mid);
            }
        }
        content.push(right);

        Line::from(Span::styled(content, Style::default().fg(colors::border())))
    }

    #[allow(clippy::cast_precision_loss)]
    fn calculate_widths(&self, col_count: usize, max_width: usize) -> Vec<usize> {
        let mut widths: Vec<usize> = (0..col_count).map(|_| 3).collect();

        // Calculate from completed rows
        for row in &self.rows {
            for (i, cell) in row.cells.iter().enumerate() {
                if i < widths.len() {
                    let cell_width = unicode_width::UnicodeWidthStr::width(cell.as_str());
                    widths[i] = widths[i].max(cell_width);
                }
            }
        }

        // Include current row
        for (i, cell) in self.current_row.iter().enumerate() {
            if i < widths.len() {
                let cell_width = unicode_width::UnicodeWidthStr::width(cell.as_str());
                widths[i] = widths[i].max(cell_width);
            }
        }

        // Include current cell
        if !self.current_cell.is_empty() && !self.current_row.is_empty() {
            let idx = self.current_row.len();
            if idx < widths.len() {
                let cell_width = unicode_width::UnicodeWidthStr::width(self.current_cell.as_str());
                widths[idx] = widths[idx].max(cell_width);
            }
        }

        // Account for borders: each column adds 3 chars (" │ "), plus 1 for final "│"
        let border_width = col_count * 3 + 1;
        let content_width: usize = widths.iter().sum();
        let total_width = content_width + border_width;

        // Scale down if needed
        if total_width > max_width && max_width > border_width {
            let available = max_width - border_width;
            let scale = available as f64 / content_width as f64;
            for w in &mut widths {
                *w = ((*w as f64 * scale) as usize).max(3);
            }
        }

        widths
    }
}

/// Parse alignment from separator cell like ":---", ":--:", "---:"
fn parse_align(cell: &str) -> CellAlign {
    let trimmed = cell.trim();
    let left = trimmed.starts_with(':');
    let right = trimmed.ends_with(':');

    match (left, right) {
        (true, true) => CellAlign::Center,
        (false, true) => CellAlign::Right,
        _ => CellAlign::Left,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cell_align_format() {
        assert_eq!(CellAlign::Left.format("hi", 5), "hi   ");
        assert_eq!(CellAlign::Right.format("hi", 5), "   hi");
        assert_eq!(CellAlign::Center.format("hi", 5), "  hi ");
        assert_eq!(CellAlign::Center.format("hi", 4), " hi ");
    }

    #[test]
    fn test_parse_align() {
        assert_eq!(parse_align(":---"), CellAlign::Left);
        assert_eq!(parse_align("---"), CellAlign::Left);
        assert_eq!(parse_align(":--:"), CellAlign::Center);
        assert_eq!(parse_align("---:"), CellAlign::Right);
    }

    #[test]
    fn debug_multi_column() {
        let mut renderer = StreamingTableRenderer::new();

        renderer.start_table();
        renderer.start_head();
        renderer.start_row();

        renderer.start_cell();
        renderer.append_text("Name");
        renderer.end_cell();

        renderer.start_cell();
        renderer.append_text("Status");
        renderer.end_cell();

        renderer.start_cell();
        renderer.append_text("Size");
        renderer.end_cell();

        renderer.end_row();
        renderer.end_head();

        // Separator
        renderer.start_row();
        renderer.start_cell();
        renderer.append_text("------");
        renderer.end_cell();
        renderer.start_cell();
        renderer.append_text("--------");
        renderer.end_cell();
        renderer.start_cell();
        renderer.append_text("------");
        renderer.end_cell();
        renderer.end_row();

        // Data row
        renderer.start_row();
        renderer.start_cell();
        renderer.append_text("file.txt");
        renderer.end_cell();
        renderer.start_cell();
        renderer.append_text("done");
        renderer.end_cell();
        renderer.start_cell();
        renderer.append_text("1.5KB");
        renderer.end_cell();
        renderer.end_row();

        // Debug
        println!("column_count: {:?}", renderer.column_count);
        println!("rows.len(): {}", renderer.rows.len());
        for (i, row) in renderer.rows.iter().enumerate() {
            println!("row[{}]: {:?} cells: {:?}", i, row.is_header, row.cells);
        }
        println!("current_row: {:?}", renderer.current_row);
        println!("current_cell: {:?}", renderer.current_cell);
        println!("aligns: {:?}", renderer.aligns);

        let lines = renderer.render(80);
        println!("\nOutput:");
        for line in &lines {
            println!("'{line}'");
        }

        // Check column count in output
        for line in &lines {
            let s = line.to_string();
            if s.contains('│') {
                let count = s.matches('│').count();
                println!("Line has {count} │: '{s}'");
            }
        }
    }

    #[test]
    fn test_wrap_text_to_width() {
        // Simple word wrap
        let lines = wrap_text_to_width("hello world", 5);
        assert_eq!(lines, vec!["hello", "world"]);

        // No wrap needed
        let lines = wrap_text_to_width("hi", 10);
        assert_eq!(lines, vec!["hi"]);

        // Long word break
        let lines = wrap_text_to_width("abcdefghij", 5);
        assert_eq!(lines, vec!["abcde", "fghij"]);

        // Empty string
        let lines = wrap_text_to_width("", 5);
        assert_eq!(lines, vec![""]);

        // Multiple spaces
        let lines = wrap_text_to_width("a   b", 3);
        assert_eq!(lines, vec!["a b"]);
    }

    #[test]
    fn test_wrap_with_unicode() {
        // Chinese characters (each is width 2)
        let lines = wrap_text_to_width("你好世界", 4);
        assert_eq!(lines, vec!["你好", "世界"]);

        // Mixed content
        let lines = wrap_text_to_width("hello 你好", 6);
        assert_eq!(lines, vec!["hello", "你好"]);
    }

    #[test]
    fn test_streaming_table_with_wrapped_content() {
        let mut renderer = StreamingTableRenderer::new();

        renderer.start_table();
        renderer.start_head();
        renderer.start_row();
        renderer.start_cell();
        renderer.append_text("Name");
        renderer.end_cell();
        renderer.start_cell();
        renderer.append_text("Description");
        renderer.end_cell();
        renderer.end_row();
        renderer.end_head();

        // Separator
        renderer.start_row();
        renderer.start_cell();
        renderer.append_text("---");
        renderer.end_cell();
        renderer.start_cell();
        renderer.append_text("---");
        renderer.end_cell();
        renderer.end_row();

        // Data row with long content
        renderer.start_row();
        renderer.start_cell();
        renderer.append_text("Item1");
        renderer.end_cell();
        renderer.start_cell();
        renderer.append_text("This is a very long description");
        renderer.end_cell();
        renderer.end_row();

        // Render with narrow width to force wrapping
        let lines = renderer.render(30);

        assert!(!lines.is_empty(), "Table should produce output");

        // The description should be wrapped across multiple lines
        let output: String = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(
            output.contains('│'),
            "Table should contain vertical borders"
        );
        // Should have multiple lines due to wrapping
        assert!(
            lines.len() > 4,
            "Wrapped table should have more lines: got {} lines",
            lines.len()
        );
    }

    #[test]
    fn test_complete_table_with_wrapping() {
        let table = Table {
            header: Some(TableRow {
                cells: vec!["Name".to_string(), "Description".to_string()],
                is_header: true,
            }),
            rows: vec![TableRow {
                cells: vec![
                    "Item".to_string(),
                    "This is a very long description that needs wrapping".to_string(),
                ],
                is_header: false,
            }],
            aligns: vec![CellAlign::Left, CellAlign::Left],
        };

        // Render with narrow width to force wrapping
        let lines = table.render(40);

        assert!(!lines.is_empty(), "Table should render");
        // Should have header + separator + wrapped data rows + borders
        assert!(lines.len() >= 4, "Wrapped table should have multiple lines");

        // Verify borders are present
        let output = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains('│'), "Should have vertical borders");
        assert!(output.contains('─'), "Should have horizontal borders");
    }
}
