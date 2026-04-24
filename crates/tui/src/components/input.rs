//! Input component for tuirealm

use std::time::{SystemTime, UNIX_EPOCH};

use tuirealm::{
    command::{Cmd, CmdResult},
    event::{Key, KeyEvent, KeyModifiers, MouseEventKind},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::Rect,
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
    },
    Component, Frame, MockComponent, State, StateValue,
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::{
    components::{
        input_edit::TextInput, status_bar::StatusMessage, CompletionList, FileCompletion,
    },
    msg::Msg,
    theme::colors,
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

#[derive(Debug, Default)]
pub struct InputMock {
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
    placeholder_tip: String,
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

impl InputMock {
    pub fn new() -> Self {
        Self {
            placeholder_tip: random_tip(),
            ..Self::default()
        }
    }
}

// Implement TextInput trait for InputMock
impl TextInput for InputMock {
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

impl InputMock {
    // InputMock-specific methods that extend TextInput trait functionality

    /// Move cursor to previous line, keeping column position if possible
    pub fn move_up(&mut self) {
        // Find the start of current line
        let line_start = self.content[..self.cursor_pos]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        // Calculate column position
        let col = self.cursor_pos - line_start;

        if line_start > 0 {
            // Find the start of previous line
            let prev_line_start = self.content[..line_start - 1]
                .rfind('\n')
                .map_or(0, |i| i + 1);
            // Find the end of previous line
            let prev_line_end = line_start - 1;
            // Move to same column, or end of line if shorter
            let prev_line_len = prev_line_end - prev_line_start;
            self.cursor_pos = prev_line_start + col.min(prev_line_len);
        }
    }

    /// Move cursor to next line, keeping column position if possible
    pub fn move_down(&mut self) {
        // Find the end of current line
        let line_end = self.content[self.cursor_pos..]
            .find('\n')
            .map_or(self.content.len(), |i| self.cursor_pos + i);
        // Calculate column position
        let line_start = self.content[..self.cursor_pos]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let col = self.cursor_pos - line_start;

        if line_end < self.content.len() {
            // Find the end of next line
            let next_line_end = self.content[line_end + 1..]
                .find('\n')
                .map_or(self.content.len(), |i| line_end + 1 + i);
            // Move to same column, or end of line if shorter
            let next_line_start = line_end + 1;
            let next_line_len = next_line_end - next_line_start;
            self.cursor_pos = next_line_start + col.min(next_line_len);
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
        self.placeholder_tip = random_tip();
    }

    /// Move cursor and clear selection if present
    fn move_and_clear_selection(&mut self, f: impl FnOnce(&mut Self)) {
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
    fn display_col_to_byte_pos(text: &str, target_col: usize) -> usize {
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
struct VisualLine {
    text: String,
    prefix: &'static str,
    content_start: usize, // Start index in original content
    content_end: usize,   // End index in original content
}

impl InputMock {
    /// Wrap text into visual lines based on available width
    fn wrap_lines(&self, content_width: usize) -> Vec<VisualLine> {
        let mut visual_lines = Vec::new();
        let mut content_idx = 0;

        for (line_num, line) in self.content.split('\n').enumerate() {
            let prefix = if line_num == 0 { "❯ " } else { "│ " };
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
                    let chunk_prefix = if is_first_chunk { prefix } else { "│ " };

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

    /// Find which visual line contains the cursor position
    fn find_cursor_visual_line(
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

impl MockComponent for InputMock {
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
                        let sel_start_in_line = norm.start.saturating_sub(line_start);
                        let sel_end_in_line = (norm.end - line_start).min(vl.text.len());

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
                    "❯ ",
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

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.props.set(attr, value);
    }

    fn state(&self) -> State {
        State::One(StateValue::String(self.content.clone()))
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(tuirealm::command::Direction::Left) => {
                self.move_left();
                CmdResult::None
            }
            Cmd::Move(tuirealm::command::Direction::Right) => {
                self.move_right();
                CmdResult::None
            }
            Cmd::Submit => {
                let content = self.submit();
                CmdResult::Submit(State::One(StateValue::String(content)))
            }
            _ => CmdResult::None,
        }
    }
}

/// Input component that handles keyboard events
/// Mode is passed from Model via attr
/// Maximum number of files to scan (prevents hanging on huge repos)
/// Available slash command with descriptions
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/new", "Create new session"),
    ("/clear", "Clear chat history"),
    ("/yolo", "Toggle YOLO mode (auto-approve all tools)"),
    ("/browse", "Toggle browse mode"),
    ("/compact", "Force message compaction"),
];

/// Random tips to show in the input placeholder
const INPUT_TIPS: &[&str] = &[
    "Shift+Enter newline · Enter send",
    "Ctrl+O browse mode · /new session",
    "Ctrl+C clear · double-click select",
    "Ctrl+V paste image · @ mention file",
    "Ctrl+P/N history · Tab complete",
    "Ctrl+W delete word · Ctrl+U kill line",
    "Alt+B/F word jump · mouse drag select",
];

/// Get a random tip based on current time
fn random_tip() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let idx = (now as usize) % INPUT_TIPS.len();
    INPUT_TIPS[idx].to_string()
}

/// Generic completion list for command and file completions
pub struct InputComponent {
    component: InputMock,
    mode: crate::app::AppMode,
    // History fields
    history: Vec<String>,
    history_index: Option<usize>, // None = new input, Some(i) = editing history[i]
    saved_input: String,          // Buffer for current input when browsing history
    // Command completion
    command_completion: CompletionList<(String, String)>,
    command_query: String,    // Current query string (text after /)
    command_start_pos: usize, // Position of '/' in the input
    // File completion (@-mention)
    file_completion: FileCompletion,
    // Paste support (images and text)
    placeholder_counter: usize,
    image_paths: std::collections::HashMap<String, std::path::PathBuf>,
    pasted_contents: std::collections::HashMap<String, String>,
}

impl Default for InputComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl InputComponent {
    pub fn new() -> Self {
        Self {
            component: InputMock::new(),
            mode: crate::app::AppMode::Normal,
            history: Vec::new(),
            history_index: None,
            saved_input: String::new(),
            command_completion: CompletionList::new(),
            command_query: String::new(),
            command_start_pos: 0,
            file_completion: FileCompletion::new(),
            placeholder_counter: 0,
            image_paths: std::collections::HashMap::new(),
            pasted_contents: std::collections::HashMap::new(),
        }
    }

    /// Generic helper to render a completion list dropdown
    fn render_completion_dropdown<T>(
        list: &mut CompletionList<T>,
        frame: &mut Frame,
        area: Rect,
        max_visible: usize,
        footer_lines: u16,
        render_item: impl Fn(&T, usize, usize) -> tuirealm::ratatui::text::Line,
    ) {
        // Note: visibility is controlled by the caller (e.g., FileCompletion::is_visible)
        // We only check if the list has items to render
        if list.is_empty() {
            return;
        }

        // Ensure selected item is visible (sticky window behavior)
        list.ensure_visible(max_visible);
        let scroll_offset = list.scroll_offset();

        let visible_count = list.len().min(max_visible);
        let height = visible_count as u16 + footer_lines;
        let dropdown_area = Rect {
            x: area.x,
            y: area.y.saturating_sub(height),
            width: area.width,
            height,
        };

        // Clear the area first
        frame.render_widget(tuirealm::ratatui::widgets::Clear, dropdown_area);

        // Render items with scrolling
        let items: Vec<tuirealm::ratatui::text::Line> = list
            .items()
            .iter()
            .enumerate()
            .skip(scroll_offset)
            .take(max_visible)
            .map(|(i, item)| render_item(item, i, list.selected_index()))
            .collect();

        let widget =
            tuirealm::ratatui::widgets::Paragraph::new(tuirealm::ratatui::text::Text::from(items));
        frame.render_widget(widget, dropdown_area);
    }

    /// Set the working directory for file completion
    pub fn set_working_dir(&mut self, path: impl Into<std::path::PathBuf>) {
        self.file_completion.set_working_dir(path);
    }

    /// Try to read image from clipboard and save to temp file
    fn try_paste_image(&mut self) -> Option<String> {
        self.try_paste_image_arboard()
    }

    /// Try to get image from clipboard
    fn try_paste_image_arboard(&mut self) -> Option<String> {
        use arboard::Clipboard;

        let mut clipboard = match Clipboard::new() {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!("Failed to create arboard clipboard: {}", e);
                return None;
            }
        };

        // Try to get image from clipboard
        let image = match clipboard.get_image() {
            Ok(img) => img,
            Err(e) => {
                tracing::debug!("No image in arboard clipboard: {}", e);
                return None;
            }
        };

        tracing::debug!(
            "Got image from arboard: {}x{}, {} bytes",
            image.width,
            image.height,
            image.bytes.len()
        );

        self.save_image_to_temp(image.width, image.height, &image.bytes)
    }

    /// Save image bytes to temp file and return placeholder
    fn save_image_to_temp(&mut self, width: usize, height: usize, bytes: &[u8]) -> Option<String> {
        // Create temp file
        let temp_dir = std::env::temp_dir().join("yomi_images");
        if let Err(e) = std::fs::create_dir_all(&temp_dir) {
            tracing::warn!("Failed to create temp dir: {}", e);
            return None;
        }

        self.placeholder_counter += 1;
        let filename = format!(
            "paste_{}_{}.png",
            std::process::id(),
            self.placeholder_counter
        );
        let filepath = temp_dir.join(&filename);

        // Check if bytes length is valid for RGBA
        let expected_len = width * height * 4;
        if bytes.len() != expected_len {
            tracing::warn!(
                "Image bytes length mismatch: got {}, expected {} ({}x{}x4)",
                bytes.len(),
                expected_len,
                width,
                height
            );
            return None;
        }

        // Save image as PNG using image crate
        let img = match image::RgbaImage::from_raw(width as u32, height as u32, bytes.to_vec()) {
            Some(img) => img,
            None => {
                tracing::warn!("Failed to create RgbaImage from raw bytes");
                return None;
            }
        };

        if let Err(e) = img.save(&filepath) {
            tracing::warn!("Failed to save image: {}", e);
            return None;
        }

        tracing::info!("Saved pasted image to: {:?}", filepath);

        // Create placeholder and store mapping
        let placeholder = format!("[Pasted #{} image]", self.placeholder_counter);
        self.image_paths.insert(placeholder.clone(), filepath);

        Some(placeholder)
    }

    /// Handle text paste by creating a placeholder
    fn handle_text_paste(&mut self, text: String) -> Msg {
        // If there's a selection, delete it first
        if self.component.has_selection() {
            self.component.delete_selection();
        }
        self.placeholder_counter += 1;
        let placeholder = format!("[Pasted #{} text]", self.placeholder_counter);
        self.pasted_contents.insert(placeholder.clone(), text);
        self.component.insert_str(&placeholder);
        self.update_completion();
        Msg::InputChanged(self.component.content().to_string())
    }

    /// Get current input as content blocks (with image and paste placeholders converted)
    pub fn get_content_blocks(&self) -> Vec<kernel::types::ContentBlock> {
        let text = self.component.content();
        tracing::debug!(
            "get_content_blocks: text='{}', image_paths={:?}, pasted_contents={:?}",
            text,
            self.image_paths,
            self.pasted_contents
        );
        let blocks = self.convert_to_content_blocks(text);
        tracing::info!("Converted to {} content blocks", blocks.len());
        for (i, block) in blocks.iter().enumerate() {
            match block {
                kernel::types::ContentBlock::Text { text } => {
                    tracing::debug!("Block {}: Text ({} chars)", i, text.len());
                }
                kernel::types::ContentBlock::ImageUrl { image_url } => {
                    let preview = if image_url.url.len() > 60 {
                        format!("{}...({} chars)", &image_url.url[..50], image_url.url.len())
                    } else {
                        image_url.url.clone()
                    };
                    tracing::info!("Block {}: ImageUrl {}", i, preview);
                }
                _ => {
                    tracing::debug!("Block {}: Other", i);
                }
            }
        }
        blocks
    }

    /// Convert input content with placeholders to content blocks
    /// Images are converted to base64 data URLs for LLM API compatibility
    /// Paste placeholders [Pasted #N image/text] are replaced with actual content
    fn convert_to_content_blocks(&self, text: &str) -> Vec<kernel::types::ContentBlock> {
        use kernel::types::{ContentBlock, ImageUrl};

        let mut blocks = Vec::new();
        let mut remaining = text;

        // Find all placeholders (both image and paste) and split text
        while let Some(start) = remaining.find('[') {
            // Add text before placeholder
            if start > 0 {
                blocks.push(ContentBlock::Text {
                    text: remaining[..start].to_string(),
                });
            }

            // Find placeholder end
            if let Some(end) = remaining[start..].find(']') {
                let end_idx = start + end;
                let potential_placeholder = &remaining[start..=end_idx];

                // Check if it's a known placeholder
                if let Some(path) = self.image_paths.get(potential_placeholder) {
                    // Image placeholder
                    match Self::image_to_base64_url(path) {
                        Some(base64_url) => blocks.push(ContentBlock::ImageUrl {
                            image_url: ImageUrl {
                                url: base64_url,
                                detail: Some("auto".to_string()),
                            },
                        }),
                        None => blocks.push(ContentBlock::Text {
                            text: format!("[Error: Failed to process {potential_placeholder}]"),
                        }),
                    }
                    remaining = &remaining[end_idx + 1..];
                } else if let Some(pasted_text) = self.pasted_contents.get(potential_placeholder) {
                    // Text placeholder
                    blocks.push(ContentBlock::Text {
                        text: pasted_text.clone(),
                    });
                    remaining = &remaining[end_idx + 1..];
                }
                // Not a recognized placeholder, treat '[' as regular text
                else {
                    blocks.push(ContentBlock::Text {
                        text: "[".to_string(),
                    });
                    remaining = &remaining[start + 1..];
                }
            } else {
                // No closing ']', treat as regular text
                break;
            }
        }

        // Add remaining text
        if !remaining.is_empty() {
            blocks.push(ContentBlock::Text {
                text: remaining.to_string(),
            });
        }

        if blocks.is_empty() {
            blocks.push(ContentBlock::Text {
                text: text.to_string(),
            });
        }

        blocks
    }

    /// Convert image file to base64 data URL
    /// OpenAI/Anthropic expect format: `data:image/{format};base64,{base64_data`}
    fn image_to_base64_url(path: &std::path::Path) -> Option<String> {
        // Read image file
        let image_data = match std::fs::read(path) {
            Ok(data) => data,
            Err(e) => {
                tracing::warn!("Failed to read image file {:?}: {}", path, e);
                return None;
            }
        };

        // Detect MIME type from file magic bytes
        let mime_type = match Self::detect_image_mime_type(&image_data) {
            Ok(mime) => mime,
            Err(e) => {
                tracing::error!("Failed to detect image format: {}", e);
                return None;
            }
        };

        // Encode to base64
        let base64_data =
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &image_data);

        // Remove any newlines that might be in the base64 output
        let base64_clean: String = base64_data.chars().filter(|c| !c.is_whitespace()).collect();

        // Create data URL with correct MIME type
        let data_url = format!("data:{mime_type};base64,{base64_clean}");

        tracing::debug!(
            "Converted image {:?} to {} base64 ({} bytes -> {} chars)",
            path,
            mime_type,
            image_data.len(),
            base64_clean.len()
        );

        Some(data_url)
    }

    /// Detect image MIME type from file magic bytes
    /// Returns error for unsupported formats
    fn detect_image_mime_type(data: &[u8]) -> Result<&'static str, String> {
        if data.starts_with(b"\x89PNG\r\n\x1a\n") {
            Ok("image/png")
        } else if data.starts_with(b"\xff\xd8\xff") {
            Ok("image/jpeg")
        } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
            Ok("image/gif")
        } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
            Ok("image/webp")
        } else {
            let magic: String = data.iter().take(16).fold(String::new(), |mut acc, b| {
                use std::fmt::Write;
                let _ = write!(acc, "{b:02x}");
                acc
            });
            Err(format!("Unsupported image format (magic bytes: {magic})"))
        }
    }

    /// Start command completion at the given cursor position
    fn start_command_completion(&mut self, cursor_pos: usize) {
        self.command_query.clear();
        self.command_start_pos = cursor_pos;
        self.refresh_command_list();
    }

    /// Refresh command list based on current query
    fn refresh_command_list(&mut self) {
        let query = &self.command_query;
        let filtered: Vec<(String, String)> = SLASH_COMMANDS
            .iter()
            .filter(|(cmd, _)| {
                if query.is_empty() {
                    true
                } else {
                    cmd.to_lowercase().contains(&query.to_lowercase())
                }
            })
            .map(|(cmd, desc)| ((*cmd).to_string(), (*desc).to_string()))
            .collect();
        self.command_completion.show(filtered);
    }

    /// Update command completion state based on current input
    fn update_completion(&mut self) {
        let content = self.component.content();
        if content.starts_with('/') && !self.command_completion.is_visible() {
            // Start fresh completion when '/' is typed
            self.start_command_completion(1);
        } else if !content.starts_with('/') {
            self.command_completion.hide();
            self.command_query.clear();
        }
    }

    /// Select next completion item
    fn completion_next(&mut self) {
        self.command_completion.next();
    }

    /// Select previous completion item
    fn completion_prev(&mut self) {
        self.command_completion.prev();
    }

    /// Accept the selected completion
    fn accept_completion(&mut self) {
        if let Some((cmd, _)) = self.command_completion.get_selected() {
            // Delete the entire query including the leading '/'
            // (command_start_pos is position after '/', so we go back one more)
            let end = self.component.cursor_pos();
            let start = self.command_start_pos.saturating_sub(1);
            for _ in 0..(end - start) {
                self.component.backspace();
            }
            // Insert the selected command followed by a space
            self.component.insert_str(cmd);
            self.component.insert_char(' ');
            self.command_completion.hide();
            self.command_query.clear();
        }
    }

    /// Start file completion (@-mention)
    fn start_file_completion(&mut self) {
        let cursor_pos = self.component.cursor_pos();
        self.file_completion.start(cursor_pos);
    }

    /// Select next file completion item
    fn file_completion_next(&mut self) {
        self.file_completion.next();
    }

    /// Select previous file completion item
    fn file_completion_prev(&mut self) {
        self.file_completion.prev();
    }

    /// Accept the selected file completion
    fn accept_file_completion(&mut self) {
        if let Some((selected, start, end)) = self.file_completion.accept() {
            let _current_pos = self.component.cursor_pos();
            // Delete the query part (from @ to current position)
            // The range is returned by accept()
            for _ in 0..(end - start) {
                self.component.backspace();
            }
            // Insert the selected file path followed by a space
            self.component.insert_str(&selected);
            self.component.insert_char(' ');
            // accept() already hides the completion
        }
    }

    /// Cancel file completion
    fn cancel_file_completion(&mut self) {
        self.file_completion.cancel();
    }

    /// Cancel command completion
    fn cancel_command_completion(&mut self) {
        self.command_completion.hide();
        self.command_query.clear();
    }

    /// Handle input when command completion is active
    fn handle_command_completion_input(
        &mut self,
        ev: &tuirealm::Event<crate::msg::UserEvent>,
    ) -> Msg {
        use tuirealm::event::{Key, KeyEvent, KeyModifiers};

        match ev {
            // Enter or Tab: accept completion
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Enter | Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.accept_completion();
                Msg::InputChanged(self.component.content().to_string())
            }
            // Shift+Tab, Up arrow or Ctrl+P: navigate up
            tuirealm::Event::Keyboard(
                KeyEvent {
                    code: Key::BackTab,
                    modifiers: KeyModifiers::SHIFT,
                }
                | KeyEvent {
                    code: Key::Up,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('p'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.completion_prev();
                Msg::Redraw
            }
            // Escape or Ctrl+C: cancel completion
            tuirealm::Event::Keyboard(
                KeyEvent {
                    code: Key::Esc,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.cancel_command_completion();
                // Also clear the input when Ctrl+C is pressed during completion
                if matches!(
                    ev,
                    tuirealm::Event::Keyboard(KeyEvent {
                        code: Key::Char('c'),
                        modifiers: KeyModifiers::CONTROL,
                    })
                ) {
                    self.component.clear();
                }
                Msg::Redraw
            }
            // Down arrow or Ctrl+N: navigate down
            tuirealm::Event::Keyboard(
                KeyEvent {
                    code: Key::Down,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('n'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.completion_next();
                Msg::Redraw
            }
            // Space: cancel completion and insert space
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char(' '),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.cancel_command_completion();
                self.component.insert_char(' ');
                Msg::InputChanged(self.component.content().to_string())
            }
            // Regular character: add to query and refresh
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                self.component.insert_char(*c);
                self.command_query.push(*c);
                self.refresh_command_list();
                Msg::InputChanged(self.component.content().to_string())
            }
            // Backspace: remove from query and refresh
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.backspace();
                let cursor_pos = self.component.cursor_pos();
                // Cancel completion if cursor moved before / symbol
                if cursor_pos < self.command_start_pos {
                    self.cancel_command_completion();
                } else {
                    self.command_query.pop();
                    self.refresh_command_list();
                }
                Msg::InputChanged(self.component.content().to_string())
            }
            _ => Msg::Redraw,
        }
    }

    /// Handle input when file completion is active
    fn handle_file_completion_input(&mut self, ev: &tuirealm::Event<crate::msg::UserEvent>) -> Msg {
        use tuirealm::event::{Key, KeyEvent, KeyModifiers};

        match ev {
            // Enter or Tab: accept completion
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Enter | Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.accept_file_completion();
                Msg::InputChanged(self.component.content().to_string())
            }
            // Shift+Tab, Up arrow or Ctrl+P: navigate up
            tuirealm::Event::Keyboard(
                KeyEvent {
                    code: Key::BackTab,
                    modifiers: KeyModifiers::SHIFT,
                }
                | KeyEvent {
                    code: Key::Up,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('p'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.file_completion_prev();
                Msg::Redraw
            }
            // Escape or Ctrl+C: cancel completion
            tuirealm::Event::Keyboard(
                KeyEvent {
                    code: Key::Esc,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.cancel_file_completion();
                // Also clear the input when Ctrl+C is pressed during completion
                if matches!(
                    ev,
                    tuirealm::Event::Keyboard(KeyEvent {
                        code: Key::Char('c'),
                        modifiers: KeyModifiers::CONTROL,
                    })
                ) {
                    self.component.clear();
                }
                Msg::Redraw
            }
            // Down arrow or Ctrl+N: navigate down
            tuirealm::Event::Keyboard(
                KeyEvent {
                    code: Key::Down,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('n'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.file_completion_next();
                Msg::Redraw
            }
            // Space: cancel completion and insert space
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char(' '),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.cancel_file_completion();
                self.component.insert_char(' ');
                Msg::InputChanged(self.component.content().to_string())
            }
            // Regular character: let FileCompletion handle it
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                self.component.insert_char(*c);
                let cursor_pos = self.component.cursor_pos();
                let _ = self.file_completion.handle_input(*c, cursor_pos);
                Msg::InputChanged(self.component.content().to_string())
            }
            // Backspace: let FileCompletion handle it
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.backspace();
                let cursor_pos = self.component.cursor_pos();
                // Cancel completion if cursor moved before @ symbol or handle_input returns false
                if cursor_pos < self.file_completion.query_start_pos()
                    || !self.file_completion.handle_input('\x08', cursor_pos)
                {
                    self.cancel_file_completion();
                }
                Msg::InputChanged(self.component.content().to_string())
            }
            _ => Msg::Redraw,
        }
    }
    /// Set the current mode
    pub const fn set_mode(&mut self, mode: crate::app::AppMode) {
        self.mode = mode;
    }

    /// Calculate the number of visual lines needed for the current content
    /// given a specific content width (accounting for wrapping)
    pub fn calculate_visual_lines(&self, content_width: usize) -> usize {
        let visual_lines = self.component.wrap_lines(content_width.max(1));
        visual_lines.len()
    }

    /// Set the history entries
    pub fn set_history(&mut self, history: Vec<String>) {
        self.history = history;
        self.history_index = None;
        self.saved_input = String::new();
    }

    /// Navigate to previous history entry (Ctrl+P)
    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }

        match self.history_index {
            None => {
                // Save current input and go to last history entry
                self.saved_input = self.component.content().to_string();
                let last_idx = self.history.len() - 1;
                self.component = InputMock::new();
                self.component.insert_str(&self.history[last_idx]);
                self.history_index = Some(last_idx);
            }
            Some(idx) if idx > 0 => {
                // Go to older entry
                let new_idx = idx - 1;
                self.component = InputMock::new();
                self.component.insert_str(&self.history[new_idx]);
                self.history_index = Some(new_idx);
            }
            Some(_) => {
                // Already at oldest
            }
        }
    }

    /// Navigate to next history entry (Ctrl+N)
    fn history_next(&mut self) {
        match self.history_index {
            None => {
                // Already at newest (editing new input)
            }
            Some(idx) if idx + 1 < self.history.len() => {
                // Go to newer entry
                let new_idx = idx + 1;
                self.component = InputMock::new();
                self.component.insert_str(&self.history[new_idx]);
                self.history_index = Some(new_idx);
            }
            Some(_) => {
                // Return to saved input
                self.component = InputMock::new();
                self.component.insert_str(&self.saved_input);
                self.history_index = None;
            }
        }
    }
}

impl MockComponent for InputComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Render command completion using generic helper
        Self::render_completion_dropdown(
            &mut self.command_completion,
            frame,
            area,
            6, // MAX_VISIBLE_ITEMS
            0, // No footer
            |(cmd, desc), i, selected_idx| {
                let is_selected = i == selected_idx;
                let cmd_style = if is_selected {
                    tuirealm::ratatui::style::Style::default()
                        .fg(colors::accent_system())
                        .add_modifier(tuirealm::ratatui::style::Modifier::BOLD)
                } else {
                    tuirealm::ratatui::style::Style::default().fg(colors::text_primary())
                };
                let desc_style = if is_selected {
                    tuirealm::ratatui::style::Style::default()
                        .fg(colors::text_muted())
                        .add_modifier(tuirealm::ratatui::style::Modifier::BOLD)
                } else {
                    tuirealm::ratatui::style::Style::default().fg(colors::text_muted())
                };
                tuirealm::ratatui::text::Line::from(vec![
                    tuirealm::ratatui::text::Span::styled(cmd.as_str(), cmd_style),
                    tuirealm::ratatui::text::Span::styled("  ", desc_style),
                    tuirealm::ratatui::text::Span::styled(desc.as_str(), desc_style),
                ])
            },
        );

        // Render file completion dropdown (reserves footer space for status line)
        Self::render_completion_dropdown(
            self.file_completion.completion_list_mut(),
            frame,
            area,
            8, // MAX_VISIBLE_FILES
            1, // Reserve space for status line above input
            |file, i, selected_idx| {
                let is_selected = i == selected_idx;
                let style = if is_selected {
                    tuirealm::ratatui::style::Style::default()
                        .fg(colors::accent_system())
                        .add_modifier(tuirealm::ratatui::style::Modifier::BOLD)
                } else {
                    tuirealm::ratatui::style::Style::default().fg(colors::text_primary())
                };
                tuirealm::ratatui::text::Line::from(tuirealm::ratatui::text::Span::styled(
                    file.as_str(),
                    style,
                ))
            },
        );

        // Render file completion status line (after dropdown, at the reserved footer position)
        if self.file_completion.is_visible() && !self.file_completion.is_empty() {
            let status_text = if self.file_completion.was_truncated() {
                format!(
                    " {} / {}+ files",
                    self.file_completion.len(),
                    self.file_completion.total_scanned()
                )
            } else {
                format!(
                    " {} / {} files",
                    self.file_completion.len(),
                    self.file_completion.total_scanned()
                )
            };
            let status_height = 1u16;
            let status_area = Rect {
                x: area.x,
                y: area.y.saturating_sub(status_height),
                width: area.width,
                height: status_height,
            };

            let status_style = tuirealm::ratatui::style::Style::default()
                .fg(colors::text_muted())
                .add_modifier(tuirealm::ratatui::style::Modifier::DIM);
            let status_line = tuirealm::ratatui::text::Line::from(
                tuirealm::ratatui::text::Span::styled(status_text, status_style),
            );
            let status_widget = tuirealm::ratatui::widgets::Paragraph::new(
                tuirealm::ratatui::text::Text::from(vec![status_line]),
            );
            frame.render_widget(status_widget, status_area);
        }

        self.component.view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.component.query(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom("mode") => {
                if let AttrValue::Number(mode_val) = value {
                    self.mode = match mode_val {
                        1 => crate::app::AppMode::Browse,
                        _ => crate::app::AppMode::Normal,
                    };
                }
            }
            Attribute::Custom("history") => {
                if let AttrValue::String(data) = value {
                    if let Ok(history) = serde_json::from_str::<Vec<String>>(&data) {
                        self.set_history(history);
                    }
                }
            }
            Attribute::Custom("working_dir") => {
                if let AttrValue::String(path) = value {
                    self.set_working_dir(path);
                }
            }
            _ => self.component.attr(attr, value),
        }
    }

    fn state(&self) -> State {
        self.component.state()
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        self.component.perform(cmd)
    }
}

impl Component<Msg, crate::msg::UserEvent> for InputComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        self.handle_input(&ev)
    }
}

impl InputComponent {
    /// Handle all input events - mode-aware handling
    fn handle_input(&mut self, ev: &tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        // Browse mode: navigation shortcuts take priority
        if self.mode == crate::app::AppMode::Browse {
            return self.handle_browse_input(ev);
        }

        // Normal mode: text input with some shortcuts
        self.handle_normal_input(ev)
    }

    /// Handle input in browse mode - navigation keys
    fn handle_browse_input(&mut self, ev: &tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        match *ev {
            // Browse mode navigation
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('j'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollDown),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('k'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollUp),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::PageUp),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('d'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::PageDown),
            // ESC or 'q' to exit browse mode
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('q') | Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ToggleBrowseMode),
            // Go to top/bottom (vim-style)
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('g'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::GoToTop),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('G'),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => Some(Msg::GoToBottom),
            // Toggle expand all with Ctrl+E in browse mode
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('e'),
                modifiers: KeyModifiers::CONTROL,
            }) => Some(Msg::ToggleExpandAll),
            // Pass through to normal input handler for other keys
            _ => self.handle_normal_input(ev),
        }
    }

    /// Parse slash command from input
    /// Returns Some(Msg) for known commands, None for unknown (treated as regular message)
    fn parse_command(content: &str) -> Option<Msg> {
        if !content.starts_with('/') {
            return None;
        }

        let parts: Vec<&str> = content.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }

        match parts[0] {
            "/new" => Some(Msg::CommandNew),
            "/clear" => Some(Msg::CommandClear),
            "/yolo" => Some(Msg::CommandYolo),
            "/browse" => Some(Msg::CommandBrowse),
            "/compact" => Some(Msg::CommandCompact),
            _ => None, // Unknown command: treat as regular message
        }
    }

    /// Handle input in normal mode - text editing
    fn handle_normal_input(&mut self, ev: &tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        use tuirealm::event::MouseEvent;

        // File completion mode - handle special keys first (use is_active, not is_visible)
        if self.file_completion.is_active() {
            return Some(self.handle_file_completion_input(ev));
        }

        // Command completion mode - handle special keys
        if self.command_completion.is_visible() {
            return Some(self.handle_command_completion_input(ev));
        }

        // Handle paste event first (needs to borrow text)
        if let tuirealm::Event::Paste(text) = ev {
            return Some(self.handle_text_paste(text.clone()));
        }

        // Handle mouse events for text selection
        if let tuirealm::Event::Mouse(MouseEvent {
            kind, column, row, ..
        }) = ev
        {
            let result = self.component.handle_mouse_event(*kind, *column, *row);

            match result {
                MouseEventResult::NotHandled => {}
                MouseEventResult::Handled => {
                    // If selection was copied, show status message
                    if matches!(kind, tuirealm::event::MouseEventKind::Up(_)) {
                        if let Some(text) = self.component.get_selected_text() {
                            let preview = crate::utils::text::truncate_unicode(&text, 30);
                            let count = text.chars().count();
                            let msg = if count > 30 {
                                format!("📋 {preview}... ({count} chars)")
                            } else {
                                format!("📋 {preview}")
                            };
                            return Some(Msg::ShowStatusMessage(StatusMessage::success(msg, 2000)));
                        }
                    }
                    return Some(Msg::Redraw);
                }
                MouseEventResult::HandledWithScroll => {
                    // Auto-scroll during drag - return Redraw to continue scrolling
                    return Some(Msg::Redraw);
                }
            }
        }

        match *ev {
            // Ctrl+V: paste from clipboard (fallback for systems without bracketed paste)
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('v'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                // If there's a selection, delete it first
                if self.component.has_selection() {
                    self.component.delete_selection();
                }
                // Try to paste image first
                if let Some(placeholder) = self.try_paste_image() {
                    self.component.insert_str(&placeholder);
                    self.update_completion();
                    return Some(Msg::InputChanged(self.component.content().to_string()));
                }
                // Fall back to reading text from clipboard
                #[cfg(not(target_os = "macos"))]
                {
                    use arboard::Clipboard;
                    match Clipboard::new() {
                        Ok(mut clipboard) => match clipboard.get_text() {
                            Ok(text) => return Some(self.handle_text_paste(text)),
                            Err(e) => tracing::debug!("No text in clipboard: {}", e),
                        },
                        Err(e) => tracing::debug!("Failed to create clipboard: {}", e),
                    }
                }
                None
            }
            // @: start file completion (must be before generic Char handler)
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('@'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.insert_char('@');
                self.start_file_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                // If there's a selection, delete it first, then insert the character
                if self.component.has_selection() {
                    self.component.delete_selection();
                }
                self.component.insert_char(c);
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            // Shift+Enter or Ctrl+J: insert newline
            tuirealm::Event::Keyboard(
                KeyEvent {
                    code: Key::Enter,
                    modifiers: KeyModifiers::SHIFT,
                }
                | KeyEvent {
                    code: Key::Char('j'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.component.insert_newline();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            // Enter: submit input
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) => {
                // If completion is visible, accept it (same as Tab)
                if self.command_completion.is_visible() {
                    self.accept_completion();
                    self.update_completion();
                    return Some(Msg::InputChanged(self.component.content().to_string()));
                }
                // Get content blocks (supports multi-modal: text, images, etc.)
                let content_blocks = self.get_content_blocks();
                // Check if content is effectively empty (no text and no images)
                let has_content = content_blocks.iter().any(|block| match block {
                    kernel::types::ContentBlock::Text { text } => !text.trim().is_empty(),
                    _ => true,
                });
                if has_content {
                    // Check if it's a command (only supports text-only content)
                    let text_content = self.component.content();
                    if let Some(cmd_msg) = Self::parse_command(text_content) {
                        // It's a command, return the command message
                        // Clear input after submitting command
                        let _ = self.component.submit();
                        Some(cmd_msg)
                    } else {
                        // Regular input with multi-modal support
                        // Clear input and mappings after submitting
                        let _ = self.component.submit();
                        self.placeholder_counter = 0;
                        self.image_paths.clear();
                        self.pasted_contents.clear();
                        Some(Msg::InputSubmit(content_blocks))
                    }
                } else {
                    None
                }
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                // If there's a selection, delete it; otherwise do normal backspace
                if self.component.has_selection() {
                    self.component.delete_selection();
                } else {
                    self.component.backspace();
                }
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Delete,
                modifiers: KeyModifiers::NONE,
            }) => {
                // If there's a selection, delete it; otherwise do normal delete
                if self.component.has_selection() {
                    self.component.delete_selection();
                } else {
                    self.component.delete_char();
                }
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Left,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.move_and_clear_selection(|c| c.move_left());
                None
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Right,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.move_and_clear_selection(|c| c.move_right());
                None
            }
            // Home or Ctrl+A: move to start of line
            tuirealm::Event::Keyboard(
                KeyEvent {
                    code: Key::Home,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('a'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.component
                    .move_and_clear_selection(|c| c.move_to_start_of_line());
                None
            }
            // End or Ctrl+E: move to end of line
            tuirealm::Event::Keyboard(
                KeyEvent {
                    code: Key::End,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('e'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.component
                    .move_and_clear_selection(|c| c.move_to_end_of_line());
                None
            }
            // Alt+B: move backward one word
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('b'),
                modifiers: KeyModifiers::ALT,
            }) => {
                self.component
                    .move_and_clear_selection(|c| c.move_word_left());
                None
            }
            // Alt+F: move forward one word
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('f'),
                modifiers: KeyModifiers::ALT,
            }) => {
                self.component
                    .move_and_clear_selection(|c| c.move_word_right());
                None
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.kill_to_start_of_line();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('w'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.delete_word_backward();
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            // Tab: accept completion or insert spaces
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                if self.command_completion.is_visible() {
                    self.accept_completion();
                    self.update_completion();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                } else {
                    // Insert tab/indent when no completion
                    self.component.insert_str("    ");
                    Some(Msg::InputChanged(self.component.content().to_string()))
                }
            }
            // Up arrow: navigate completion or history
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Up,
                modifiers: KeyModifiers::NONE,
            }) => {
                if self.command_completion.is_visible() {
                    self.completion_prev();
                    Some(Msg::Redraw)
                } else if self.component.is_on_first_line() {
                    self.history_prev();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                } else {
                    self.component.move_and_clear_selection(|c| c.move_up());
                    None
                }
            }
            // Down arrow: navigate completion or history
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::NONE,
            }) => {
                if self.command_completion.is_visible() {
                    self.completion_next();
                    Some(Msg::Redraw)
                } else if self.component.is_on_last_line() {
                    self.history_next();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                } else {
                    self.component.move_and_clear_selection(|c| c.move_down());
                    None
                }
            }
            // Ctrl+P: navigate completion or history
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('p'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                if self.command_completion.is_visible() {
                    self.completion_prev();
                    Some(Msg::Redraw)
                } else {
                    self.history_prev();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                }
            }
            // Ctrl+N: navigate completion or history
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('n'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                if self.command_completion.is_visible() {
                    self.completion_next();
                    Some(Msg::Redraw)
                } else {
                    self.history_next();
                    Some(Msg::InputChanged(self.component.content().to_string()))
                }
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::CancelRequest),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('c'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                if self.component.handle_ctrl_c() {
                    Some(Msg::Quit)
                } else {
                    // First Ctrl+C: show hint in status bar for 1 second
                    Some(Msg::ShowStatusMessage(StatusMessage::new(
                        "Press Ctrl+C again to exit",
                        crate::components::status_bar::MessageLevel::Unknown,
                        1000, // 1000ms = 1 second, matches double-press detection
                    )))
                }
            }
            // PageUp/PageDown always scroll chat view
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::PageUp,
                modifiers: KeyModifiers::NONE,
            })
            | tuirealm::Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                ..
            }) => Some(Msg::ScrollUp),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::PageDown,
                modifiers: KeyModifiers::NONE,
            })
            | tuirealm::Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                ..
            }) => Some(Msg::ScrollDown),
            // Toggle browse mode with Ctrl+O
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('o'),
                modifiers: KeyModifiers::CONTROL,
            }) => Some(Msg::ToggleBrowseMode),
            _ => None,
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
        assert_eq!(InputMock::display_col_to_byte_pos(text, 0), 0);
        assert_eq!(InputMock::display_col_to_byte_pos(text, 5), 5);
        assert_eq!(InputMock::display_col_to_byte_pos(text, 100), 11);
    }

    #[test]
    fn test_display_col_to_byte_pos_unicode() {
        // CJK characters are typically 2 display columns wide
        let text = "你好世界"; // Each char is 2-3 bytes (UTF-8) and 2 display columns

        // At column 0, should be at start
        assert_eq!(InputMock::display_col_to_byte_pos(text, 0), 0);

        // At column 1 (middle of first char), should still be at first char
        assert_eq!(InputMock::display_col_to_byte_pos(text, 1), 0);

        // At column 2 (end of first char), should move to second char
        assert_eq!(InputMock::display_col_to_byte_pos(text, 2), "你".len());

        // At column 4 (end of second char)
        assert_eq!(InputMock::display_col_to_byte_pos(text, 4), "你好".len());
    }

    #[test]
    fn test_display_col_to_byte_pos_mixed() {
        // Mixed ASCII and Unicode
        let text = "hi你好";
        // h(0)i(1)你(2-4)好(5-7)
        // Display: h(0)i(1)你(2-3)好(4-5)

        assert_eq!(InputMock::display_col_to_byte_pos(text, 0), 0); // Before 'h'
        assert_eq!(InputMock::display_col_to_byte_pos(text, 1), 1); // After 'h', at 'i'
        assert_eq!(InputMock::display_col_to_byte_pos(text, 2), 2); // After 'i', at '你'
        assert_eq!(InputMock::display_col_to_byte_pos(text, 3), 2); // Middle of '你'
        assert_eq!(InputMock::display_col_to_byte_pos(text, 4), 5); // After '你', at '好'
    }

    #[test]
    fn test_select_word_at() {
        let mut input = InputMock::new();
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
        let mut input = InputMock::new();
        input.insert_str("hello world");
        input.start_selection(0);
        input.update_selection(5); // Select "hello"
        input.delete_selection();

        assert_eq!(input.content(), " world");
        assert_eq!(input.cursor_pos(), 0);
    }
}
