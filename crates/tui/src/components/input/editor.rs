//! Core text editor for the input component

use tuirealm::{
    command::{Cmd, CmdResult},
    component::Component,
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{
        layout::Rect,
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
        Frame,
    },
    state::{State, StateValue},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    components::input_edit::TextInput,
    theme::{chars, colors},
};

/// Text selection state for input component
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InputSelection {
    pub start: usize, // byte position
    pub end: usize,   // byte position
}

impl InputSelection {
    /// Get normalized selection (start <= end)
    #[must_use]
    pub fn normalized(&self) -> Self {
        if self.start <= self.end {
            *self
        } else {
            Self {
                start: self.end,
                end: self.start,
            }
        }
    }

    /// Check if selection is empty
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Check if a byte position is within the selection
    pub fn contains(&self, pos: usize) -> bool {
        let norm = self.normalized();
        pos >= norm.start && pos < norm.end
    }
}

/// Result of handling a mouse event
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseEventResult {
    /// Event was not handled
    NotHandled,
    /// Event was handled, needs redraw
    Handled,
    /// Event was handled, needs redraw and auto-scroll may be needed
    /// (mouse is at boundary during drag)
    HandledWithScroll,
}

#[derive(Debug, Default)]
pub struct InputEditor {
    props: Props,
    content: String,
    cursor_pos: usize,
    last_ctrl_c_time: Option<std::time::Instant>,
    // Text selection state
    selection: Option<InputSelection>,
    is_selecting: bool,
    // Track last click for double-click detection
    last_click_time: Option<std::time::Instant>,
    last_click_pos: Option<usize>,
    // Current display area for mouse coordinate calculation
    current_area: Option<Rect>,
    // Manual scroll offset for auto-scroll during selection
    scroll_override: Option<usize>,
    // Random tip to show in placeholder
    pub(crate) placeholder_tip: String,
}

impl InputEditor {
    pub fn new() -> Self {
        Self {
            placeholder_tip: super::random_tip(),
            ..Self::default()
        }
    }
}

// Implement TextInput trait for InputEditor
impl TextInput for InputEditor {
    fn text(&self) -> &str {
        &self.content
    }

    fn text_mut(&mut self) -> &mut String {
        &mut self.content
    }

    fn cursor_pos(&self) -> usize {
        self.cursor_pos
    }

    fn set_cursor_pos(&mut self, pos: usize) {
        self.cursor_pos = pos.min(self.content.len());
    }
}

impl InputEditor {
    // InputEditor-specific methods that extend TextInput trait functionality

    /// Move cursor to previous line, keeping column position if possible
    pub fn move_up(&mut self) {
        // Find the start of current line
        let line_start = self.content[..self.cursor_pos]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        // Calculate column position in characters (not bytes)
        let col_chars = self.content[line_start..self.cursor_pos].chars().count();

        if line_start > 0 {
            // Find the start of previous line
            let prev_line_start = self.content[..line_start - 1]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            // Find the end of previous line
            let prev_line_end = line_start - 1;
            // Move to same column (by char count), or end of line if shorter
            let prev_line: String = self.content[prev_line_start..prev_line_end]
                .chars()
                .take(col_chars)
                .collect();
            self.cursor_pos = prev_line_start + prev_line.len();
        }
    }

    /// Move cursor to next line, keeping column position if possible
    pub fn move_down(&mut self) {
        // Find the end of current line
        let line_end = self.content[self.cursor_pos..]
            .find('\n')
            .map_or(self.content.len(), |i| self.cursor_pos + i);
        // Calculate column position in characters (not bytes)
        let line_start = self.content[..self.cursor_pos]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let col_chars = self.content[line_start..self.cursor_pos].chars().count();

        if line_end < self.content.len() {
            // Find the end of next line
            let next_line_end = self.content[line_end + 1..]
                .find('\n')
                .map_or(self.content.len(), |i| line_end + 1 + i);
            // Move to same column (by char count), or end of line if shorter
            let next_line_start = line_end + 1;
            let next_line: String = self.content[next_line_start..next_line_end]
                .chars()
                .take(col_chars)
                .collect();
            self.cursor_pos = next_line_start + next_line.len();
        }
    }

    /// Check if cursor is on the first line
    pub fn is_on_first_line(&self) -> bool {
        !self.content[..self.cursor_pos].contains('\n')
    }

    /// Check if cursor is on the last line
    pub fn is_on_last_line(&self) -> bool {
        !self.content[self.cursor_pos..].contains('\n')
    }

    pub fn insert_newline(&mut self) {
        self.content.insert(self.cursor_pos, '\n');
        self.cursor_pos += 1;
    }

    /// Handle ctrl-c: clear input, or quit if pressed twice within 1 second
    /// Returns true if should quit
    pub fn handle_ctrl_c(&mut self) -> bool {
        let now = std::time::Instant::now();
        if let Some(last_time) = self.last_ctrl_c_time {
            if now.duration_since(last_time).as_secs_f32() < 1.0 {
                // Double press within 1 second - quit
                return true;
            }
        }
        self.clear();
        self.last_ctrl_c_time = Some(now);
        false
    }

    pub fn content(&self) -> &str {
        &self.content
    }

    pub fn submit(&mut self) -> String {
        let content = self.content.clone();
        self.clear();
        content
    }

    // Selection methods

    /// Start text selection at the given byte position
    pub fn start_selection(&mut self, pos: usize) {
        let clamped = pos.min(self.content.len());
        self.selection = Some(InputSelection {
            start: clamped,
            end: clamped,
        });
        self.is_selecting = true;
    }

    /// Update selection end position while dragging
    pub fn update_selection(&mut self, pos: usize) {
        if let Some(ref mut sel) = self.selection {
            sel.end = pos.min(self.content.len());
        }
    }

    /// End text selection
    pub fn end_selection(&mut self) {
        self.is_selecting = false;
        self.scroll_override = None;
    }

    /// Clear the current selection
    pub fn clear_selection(&mut self) {
        self.selection = None;
        self.is_selecting = false;
    }

    /// Clear all state including selection and click tracking
    pub fn clear(&mut self) {
        self.content.clear();
        self.cursor_pos = 0;
        self.selection = None;
        self.is_selecting = false;
        self.last_click_time = None;
        self.last_click_pos = None;
        self.scroll_override = None;
        self.placeholder_tip = super::random_tip();
    }

    /// Move cursor and clear selection if present
    pub fn move_and_clear_selection(&mut self, f: impl FnOnce(&mut Self)) {
        if self.has_selection() {
            self.clear_selection();
        }
        f(self);
    }

    /// Get the current selection
    pub fn selection(&self) -> Option<&InputSelection> {
        self.selection.as_ref()
    }

    /// Check if there's an active selection (non-empty)
    pub fn has_selection(&self) -> bool {
        self.selection.as_ref().is_some_and(|s| !s.is_empty())
    }

    /// Get the selected text
    pub fn get_selected_text(&self) -> Option<String> {
        let sel = self.selection?;
        let norm = sel.normalized();
        if norm.is_empty() {
            return None;
        }
        Some(self.content[norm.start..norm.end].to_string())
    }

    /// Copy the current selection to clipboard
    pub fn copy_selection(&self) -> Option<String> {
        let text = self.get_selected_text()?;
        if text.is_empty() {
            return None;
        }

        // Copy to clipboard
        if let Err(e) = crate::utils::clipboard::copy_text(&text) {
            tracing::debug!("Failed to copy to clipboard: {}", e);
            return None;
        }

        Some(text)
    }

    /// Delete the selected text and clear selection
    pub fn delete_selection(&mut self) {
        if let Some(sel) = self.selection {
            let norm = sel.normalized();
            if !norm.is_empty() {
                self.content.drain(norm.start..norm.end);
                self.cursor_pos = norm.start;
            }
            self.clear_selection();
        }
    }

    /// Select a word at the given byte position (double-click)
    fn select_word_at(&mut self, pos: usize) {
        let clamped = pos.min(self.content.len());

        // Find word boundaries
        let is_word_char = |c: char| c.is_alphanumeric() || c == '_';

        // Find start of word
        let mut start = clamped;
        for (idx, c) in self.content[..clamped].char_indices().rev() {
            if !is_word_char(c) {
                break;
            }
            start = idx;
        }

        // Find end of word
        let mut end = clamped;
        for (idx, c) in self.content[clamped..].char_indices() {
            if !is_word_char(c) {
                break;
            }
            end = clamped + idx + c.len_utf8();
        }

        self.selection = Some(InputSelection { start, end });
        self.is_selecting = true;
    }

    /// Check if this is a double click (within 300ms and same position)
    fn is_double_click(&mut self, pos: usize) -> bool {
        const DOUBLE_CLICK_THRESHOLD: std::time::Duration = std::time::Duration::from_millis(300);

        let now = std::time::Instant::now();
        let is_double = self
            .last_click_time
            .is_some_and(|t| now.duration_since(t) < DOUBLE_CLICK_THRESHOLD)
            && self.last_click_pos == Some(pos);

        self.last_click_time = Some(now);
        self.last_click_pos = Some(pos);

        is_double
    }

    /// Handle mouse event for text selection
    /// Returns `MouseEventResult` indicating how the event was handled
    pub fn handle_mouse_event(
        &mut self,
        kind: tuirealm::event::MouseEventKind,
        mouse_x: u16,
        mouse_y: u16,
    ) -> MouseEventResult {
        use tuirealm::event::MouseEventKind;

        let area = match self.current_area {
            Some(a) => a,
            None => return MouseEventResult::NotHandled,
        };

        if !Self::is_mouse_within_area(mouse_x, mouse_y, area) && !self.is_selecting {
            self.clear_selection();
            return MouseEventResult::NotHandled;
        }

        let content_width = area.width as usize;
        let visible_height = area.height.saturating_sub(2).max(1) as usize;
        let visual_lines = self.wrap_lines(content_width);

        let (scroll_offset, needs_auto_scroll) =
            self.calculate_scroll_with_auto_scroll(mouse_y, area, &visual_lines, visible_height);

        let byte_pos =
            Self::mouse_pos_to_byte_pos(mouse_x, mouse_y, area, &visual_lines, scroll_offset);

        match kind {
            MouseEventKind::Down(_) => {
                if self.is_double_click(byte_pos) {
                    self.select_word_at(byte_pos);
                } else {
                    self.start_selection(byte_pos);
                }
                MouseEventResult::Handled
            }
            MouseEventKind::Drag(_) => {
                if self.is_selecting {
                    self.update_selection(byte_pos);
                    if needs_auto_scroll {
                        return MouseEventResult::HandledWithScroll;
                    }
                }
                MouseEventResult::Handled
            }
            MouseEventKind::Up(_) => {
                if self.is_selecting {
                    self.end_selection();
                    let _ = self.copy_selection();
                }
                self.scroll_override = None;
                MouseEventResult::Handled
            }
            _ => MouseEventResult::NotHandled,
        }
    }

    /// Check if mouse coordinates are within the input area
    fn is_mouse_within_area(mouse_x: u16, mouse_y: u16, area: Rect) -> bool {
        mouse_x >= area.x
            && mouse_x < area.x + area.width
            && mouse_y >= area.y
            && mouse_y < area.y + area.height
    }

    /// Calculate scroll offset, applying auto-scroll if near boundaries during drag
    fn calculate_scroll_with_auto_scroll(
        &mut self,
        mouse_y: u16,
        area: Rect,
        visual_lines: &[VisualLine],
        visible_height: usize,
    ) -> (usize, bool) {
        let max_scroll = visual_lines.len().saturating_sub(visible_height);

        let base_scroll = if visual_lines.len() > visible_height {
            let (cursor_line, _, _) = self
                .find_cursor_visual_line(visual_lines)
                .unwrap_or((0, 0, 0));
            cursor_line
                .saturating_sub(visible_height.saturating_sub(1))
                .min(max_scroll)
        } else {
            0
        };

        let scroll_offset = self.scroll_override.unwrap_or(base_scroll);

        let top_boundary = area.y + 1;
        let bottom_boundary = area.y + area.height - 1;
        let threshold = 1u16;

        let near_top = mouse_y < top_boundary + threshold;
        let near_bottom = mouse_y >= bottom_boundary.saturating_sub(threshold);
        let needs_auto_scroll = self.is_selecting && (near_top || near_bottom);

        let effective_scroll = if needs_auto_scroll {
            let new_scroll = if near_top {
                scroll_offset.saturating_sub(1)
            } else {
                (scroll_offset + 1).min(max_scroll)
            };
            self.scroll_override = Some(new_scroll);
            new_scroll
        } else {
            if !self.is_selecting {
                self.scroll_override = None;
            }
            scroll_offset
        };

        (effective_scroll, needs_auto_scroll)
    }

    /// Convert mouse coordinates to byte position in content
    fn mouse_pos_to_byte_pos(
        mouse_x: u16,
        mouse_y: u16,
        area: Rect,
        visual_lines: &[VisualLine],
        scroll_offset: usize,
    ) -> usize {
        let row_in_view = mouse_y.saturating_sub(area.y).saturating_sub(1) as usize;
        let line_idx = (scroll_offset + row_in_view).min(visual_lines.len().saturating_sub(1));

        let visual_line = match visual_lines.get(line_idx) {
            Some(vl) => vl,
            None => return 0,
        };

        let prefix_width = visual_line.prefix.width();
        let content_x = if (mouse_x as usize) < area.x as usize + prefix_width {
            0
        } else {
            (mouse_x as usize) - area.x as usize - prefix_width
        };

        let line_byte_pos = Self::display_col_to_byte_pos(&visual_line.text, content_x);
        visual_line.content_start + line_byte_pos
    }

    /// Convert display column to byte position in the given text
    pub fn display_col_to_byte_pos(text: &str, target_col: usize) -> usize {
        let mut display_col = 0;
        let mut byte_pos = 0;

        for c in text.chars() {
            let ch_width = c.width().unwrap_or(0);

            if display_col + ch_width > target_col {
                return byte_pos;
            }

            display_col += ch_width;
            byte_pos += c.len_utf8();
        }

        byte_pos
    }
}

/// A visual line with prefix info for cursor calculation
#[derive(Debug)]
pub(crate) struct VisualLine {
    pub text: String,
    pub prefix: &'static str,
    pub content_start: usize, // Start index in original content
    pub content_end: usize,   // End index in original content
}

impl InputEditor {
    /// Wrap text into visual lines based on available width
    pub(crate) fn wrap_lines(&self, content_width: usize) -> Vec<VisualLine> {
        let mut visual_lines = Vec::new();
        let mut content_idx = 0;

        for (line_num, line) in self.content.split('\n').enumerate() {
            let prefix = if line_num == 0 {
                chars::INPUT_PROMPT
            } else {
                chars::INPUT_PROMPT_MULTI
            };
            let prefix_width = prefix.width();
            let available_width = content_width.saturating_sub(prefix_width);

            if line.is_empty() {
                // Empty line - still need a visual line for the prefix
                visual_lines.push(VisualLine {
                    text: String::new(),
                    prefix,
                    content_start: content_idx,
                    content_end: content_idx,
                });
            } else {
                // Wrap the line into chunks that fit
                let mut line_idx = 0;
                let mut is_first_chunk = true;

                while line_idx < line.len() {
                    // Find how many chars fit in available_width
                    let chunk = Self::truncate_to_width(&line[line_idx..], available_width);
                    let chunk_len = chunk.len();
                    let chunk_prefix = if is_first_chunk {
                        prefix
                    } else {
                        chars::INPUT_PROMPT_MULTI
                    };

                    visual_lines.push(VisualLine {
                        text: chunk.to_string(),
                        prefix: chunk_prefix,
                        content_start: content_idx + line_idx,
                        content_end: content_idx + line_idx + chunk_len,
                    });

                    line_idx += chunk_len;
                    is_first_chunk = false;
                }
            }

            // +1 for the '\n' character
            content_idx += line.len() + 1;
        }

        visual_lines
    }

    /// Truncate a string to fit within `max_width` display columns
    fn truncate_to_width(s: &str, max_width: usize) -> &str {
        if s.width() <= max_width {
            return s;
        }

        let mut width = 0;
        let mut end = 0;

        for (idx, c) in s.char_indices() {
            let char_width = c.width().unwrap_or(0);
            if width + char_width > max_width {
                break;
            }
            width += char_width;
            end = idx + c.len_utf8();
        }

        &s[..end]
    }

    /// Align a byte position to the nearest valid char boundary.
    /// If the position is inside a multi-byte character, it will be
    /// adjusted to the start of that character.
    pub fn align_to_char_boundary(s: &str, pos: usize) -> usize {
        let len = s.len();
        if pos >= len {
            return len;
        }
        if s.is_char_boundary(pos) {
            return pos;
        }
        // Search backwards for the nearest char boundary
        let mut idx = pos;
        while idx > 0 && !s.is_char_boundary(idx) {
            idx -= 1;
        }
        idx
    }

    /// Find which visual line contains the cursor position
    pub(crate) fn find_cursor_visual_line(
        &self,
        visual_lines: &[VisualLine],
    ) -> Option<(usize, usize, usize)> {
        // Returns (line_index, column_in_visual_line, visual_line_start_in_content)
        for (i, line) in visual_lines.iter().enumerate() {
            if self.cursor_pos >= line.content_start && self.cursor_pos <= line.content_end {
                let col_in_line = if self.cursor_pos > line.content_start {
                    self.content[line.content_start..self.cursor_pos].width()
                } else {
                    0
                };
                return Some((i, col_in_line, line.content_start));
            }
        }
        // Cursor at the end
        if let Some(last) = visual_lines.last() {
            let col = last.text.width();
            return Some((visual_lines.len() - 1, col, last.content_start));
        }
        None
    }
}

impl Component for InputEditor {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Store area for mouse coordinate calculation
        self.current_area = Some(area);

        // Calculate available width for content
        // Note: area.width already excludes borders (they're rendered by Paragraph block)
        let content_width = area.width as usize;

        // Get visual lines with wrapping
        let visual_lines = self.wrap_lines(content_width);

        // Find cursor position in visual lines
        let (cursor_visual_line, cursor_col, _) = self
            .find_cursor_visual_line(&visual_lines)
            .unwrap_or((0, 0, 0));

        // Calculate scroll offset to keep cursor visible
        let visible_height = area.height.saturating_sub(2).max(1) as usize; // -2 for top/bottom borders

        let scroll_offset = if let Some(override_scroll) = self.scroll_override {
            // Use manual scroll override (e.g., from auto-scroll during selection)
            override_scroll.min(visual_lines.len().saturating_sub(visible_height))
        } else if visual_lines.len() > visible_height {
            // Scroll so cursor is visible (prefer showing cursor near bottom)
            cursor_visual_line
                .saturating_sub(visible_height.saturating_sub(1))
                .min(visual_lines.len().saturating_sub(visible_height))
        } else {
            0
        };

        // Render visible lines with selection highlighting
        let highlight_style = Style::default()
            .fg(colors::text_primary())
            .bg(colors::selected_bg());
        let normal_style = Style::default().fg(colors::text_primary());
        let prefix_style = Style::default()
            .fg(colors::accent_user())
            .add_modifier(Modifier::BOLD);

        let all_lines: Vec<Line> = visual_lines
            .iter()
            .map(|vl| {
                // Build spans for this line, handling selection
                let mut spans = vec![Span::styled(vl.prefix, prefix_style)];

                if let Some(sel) = self.selection {
                    let norm = sel.normalized();
                    let line_start = vl.content_start;
                    let line_end = vl.content_end;

                    // Check if selection overlaps with this visual line
                    if norm.start < line_end && norm.end > line_start {
                        // There is overlap, split into segments
                        // Clamp positions to valid range within vl.text (which may be empty)
                        let sel_start_in_line =
                            norm.start.saturating_sub(line_start).min(vl.text.len());
                        let sel_end_in_line =
                            norm.end.saturating_sub(line_start).min(vl.text.len());

                        // Ensure byte indices are at char boundaries to avoid panic
                        let sel_start_in_line =
                            Self::align_to_char_boundary(&vl.text, sel_start_in_line);
                        let sel_end_in_line =
                            Self::align_to_char_boundary(&vl.text, sel_end_in_line);

                        if sel_start_in_line > 0 {
                            // Unselected prefix
                            spans.push(Span::styled(
                                vl.text[..sel_start_in_line].to_string(),
                                normal_style,
                            ));
                        }
                        if sel_end_in_line > sel_start_in_line {
                            // Selected portion
                            spans.push(Span::styled(
                                vl.text[sel_start_in_line..sel_end_in_line].to_string(),
                                highlight_style,
                            ));
                        }
                        if sel_end_in_line < vl.text.len() {
                            // Unselected suffix
                            spans.push(Span::styled(
                                vl.text[sel_end_in_line..].to_string(),
                                normal_style,
                            ));
                        }
                    } else {
                        // No overlap, render normally
                        spans.push(Span::styled(vl.text.clone(), normal_style));
                    }
                } else {
                    // No selection, render normally
                    spans.push(Span::styled(vl.text.clone(), normal_style));
                }

                Line::from(spans)
            })
            .collect();

        // Slice visible lines based on scroll offset
        let start = scroll_offset.min(all_lines.len());
        let end = (scroll_offset + visible_height).min(all_lines.len());
        let visible_line_slices: Vec<Line> = all_lines[start..end].to_vec();

        // Show placeholder only when content is truly empty
        let text = if self.content.is_empty() {
            tuirealm::ratatui::text::Text::from(vec![Line::from(vec![
                Span::styled(
                    chars::INPUT_PROMPT,
                    Style::default()
                        .fg(colors::accent_user())
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    &self.placeholder_tip,
                    Style::default().fg(colors::text_muted()),
                ),
            ])])
        } else {
            tuirealm::ratatui::text::Text::from(visible_line_slices)
        };

        let paragraph = Paragraph::new(text).block(
            tuirealm::ratatui::widgets::Block::default()
                .borders(
                    tuirealm::ratatui::widgets::Borders::TOP
                        | tuirealm::ratatui::widgets::Borders::BOTTOM,
                )
                .border_style(Style::default().fg(colors::border())),
        );

        frame.render_widget(paragraph, area);

        // Set cursor position
        let cursor_x = area.x
            + visual_lines
                .get(cursor_visual_line)
                .map_or(2, |l| l.prefix.width() as u16)
            + cursor_col as u16;
        let cursor_y = area.y + 1 + cursor_visual_line.saturating_sub(scroll_offset) as u16;

        // Always show cursor when component is active (even if content is empty)
        if cursor_y < area.y + area.height {
            frame.set_cursor_position(tuirealm::ratatui::layout::Position::new(cursor_x, cursor_y));
        }
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.props.get(attr).map(|v| v.into())
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.props.set(attr, value);
    }

    fn state(&self) -> State {
        State::Single(StateValue::String(self.content.clone()))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(tuirealm::command::Direction::Left) => {
                self.move_left();
                CmdResult::NoChange
            }
            Cmd::Move(tuirealm::command::Direction::Right) => {
                self.move_right();
                CmdResult::NoChange
            }
            Cmd::Submit => {
                let content = self.submit();
                CmdResult::Submit(State::Single(StateValue::String(content)))
            }
            _ => CmdResult::NoChange,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_input_selection_normalized() {
        let sel = InputSelection { start: 10, end: 5 };
        let norm = sel.normalized();
        assert_eq!(norm.start, 5);
        assert_eq!(norm.end, 10);

        let sel2 = InputSelection { start: 5, end: 10 };
        let norm2 = sel2.normalized();
        assert_eq!(norm2.start, 5);
        assert_eq!(norm2.end, 10);
    }

    #[test]
    fn test_input_selection_contains() {
        let sel = InputSelection { start: 5, end: 10 };
        assert!(sel.contains(5));
        assert!(sel.contains(9));
        assert!(!sel.contains(10));
        assert!(!sel.contains(4));
    }

    #[test]
    fn test_display_col_to_byte_pos_ascii() {
        let text = "hello world";
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 0), 0);
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 5), 5);
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 100), 11);
    }

    #[test]
    fn test_display_col_to_byte_pos_unicode() {
        // CJK characters are typically 2 display columns wide
        let text = "你好世界"; // Each char is 2-3 bytes (UTF-8) and 2 display columns

        // At column 0, should be at start
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 0), 0);

        // At column 1 (middle of first char), should still be at first char
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 1), 0);

        // At column 2 (end of first char), should move to second char
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 2), "你".len());

        // At column 4 (end of second char)
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 4), "你好".len());
    }

    #[test]
    fn test_display_col_to_byte_pos_mixed() {
        // Mixed ASCII and Unicode
        let text = "hi你好";
        // h(0)i(1)你(2-4)好(5-7)
        // Display: h(0)i(1)你(2-3)好(4-5)

        assert_eq!(InputEditor::display_col_to_byte_pos(text, 0), 0); // Before 'h'
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 1), 1); // After 'h', at 'i'
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 2), 2); // After 'i', at '你'
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 3), 2); // Middle of '你'
        assert_eq!(InputEditor::display_col_to_byte_pos(text, 4), 5); // After '你', at '好'
    }

    #[test]
    fn test_select_word_at() {
        let mut input = InputEditor::new();
        input.insert_str("hello world test");

        // Click on 'w' in "world"
        input.select_word_at(6);
        let sel = input.selection().unwrap();
        assert_eq!(sel.start, 6);
        assert_eq!(sel.end, 11); // "world" is 5 chars

        // Click on 'o' in "hello"
        input.select_word_at(4);
        let sel2 = input.selection().unwrap();
        assert_eq!(sel2.start, 0);
        assert_eq!(sel2.end, 5); // "hello" is 5 chars
    }

    #[test]
    fn test_delete_selection() {
        let mut input = InputEditor::new();
        input.insert_str("hello world");
        input.start_selection(0);
        input.update_selection(5); // Select "hello"
        input.delete_selection();

        assert_eq!(input.content(), " world");
        assert_eq!(input.cursor_pos(), 0);
    }
}
