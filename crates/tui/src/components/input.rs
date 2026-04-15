//! Input component for tuirealm

use tuirealm::{
    command::{Cmd, CmdResult},
    event::{Key, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind},
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

use crate::{components::input_edit::TextInput, msg::Msg, theme::colors};

#[derive(Debug, Default)]
pub struct InputMock {
    props: Props,
    content: String,
    cursor_pos: usize,
    last_ctrl_c_time: Option<std::time::Instant>,
}

impl InputMock {
    pub fn new() -> Self {
        Self::default()
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
        // Calculate available width for content (accounting for borders)
        let content_width = (area.width.saturating_sub(2) as usize).max(1); // -2 for borders

        // Get visual lines with wrapping
        let visual_lines = self.wrap_lines(content_width);

        // Find cursor position in visual lines
        let (cursor_visual_line, cursor_col, _) = self
            .find_cursor_visual_line(&visual_lines)
            .unwrap_or((0, 0, 0));

        // Calculate scroll offset to keep cursor visible
        let visible_height = area.height.saturating_sub(2).max(1) as usize; // -2 for top/bottom borders

        let scroll_offset = if visual_lines.len() > visible_height {
            // Scroll so cursor is visible (prefer showing cursor near bottom)
            cursor_visual_line
                .saturating_sub(visible_height.saturating_sub(1))
                .min(visual_lines.len().saturating_sub(visible_height))
        } else {
            0
        };

        // Render visible lines
        let all_lines: Vec<Line> = visual_lines
            .iter()
            .map(|vl| {
                Line::from(vec![
                    Span::styled(
                        vl.prefix,
                        Style::default()
                            .fg(colors::accent_user())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(vl.text.clone(), Style::default().fg(colors::text_primary())),
                ])
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
                    "Type a message...",
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
        let cursor_y = area.y + 1 + (cursor_visual_line - scroll_offset) as u16;

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
const MAX_FILES_TO_SCAN: usize = 1000;
/// Maximum number of files to display (performance)
const MAX_FILES_TO_DISPLAY: usize = 50;

/// Available slash command with descriptions
const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/new", "Create new session"),
    ("/clear", "Clear chat history"),
    ("/yolo", "Toggle YOLO mode (auto-approve all tools)"),
    ("/browse", "Toggle browse mode"),
];

/// Generic completion list for command and file completions
struct CompletionList<T> {
    visible: bool,
    selected: usize,
    items: Vec<T>,
}

impl<T> CompletionList<T> {
    fn new() -> Self {
        Self {
            visible: false,
            selected: 0,
            items: Vec::new(),
        }
    }

    fn is_visible(&self) -> bool {
        self.visible
    }

    fn show(&mut self, items: Vec<T>) {
        self.items = items;
        self.visible = !self.items.is_empty();
        self.selected = 0;
    }

    fn hide(&mut self) {
        self.visible = false;
        self.items.clear();
        self.selected = 0;
    }

    fn next(&mut self) {
        if !self.items.is_empty() {
            self.selected = (self.selected + 1) % self.items.len();
        }
    }

    fn prev(&mut self) {
        if !self.items.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.items.len() - 1);
        }
    }

    fn selected_index(&self) -> usize {
        self.selected
    }

    fn get_selected(&self) -> Option<&T> {
        self.items.get(self.selected)
    }

    fn len(&self) -> usize {
        self.items.len()
    }

    fn items(&self) -> &[T] {
        &self.items
    }
}

pub struct InputComponent {
    component: InputMock,
    mode: crate::app::AppMode,
    // History fields
    history: Vec<String>,
    history_index: Option<usize>, // None = new input, Some(i) = editing history[i]
    saved_input: String,          // Buffer for current input when browsing history
    // Command completion
    command_completion: CompletionList<(String, String)>,
    // File completion (@-mention)
    file_completion: CompletionList<String>,
    file_query: String,
    file_query_start_pos: usize, // Position of '@' in input
    working_dir: std::path::PathBuf,
    // File cache for performance
    all_files: Vec<String>,     // Cached file list
    file_cache_dirty: bool,     // Whether cache needs refresh
    total_files_scanned: usize, // Total files found (may exceed cache)
    files_truncated: bool,      // Whether scan hit MAX_FILES_TO_SCAN limit
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
            file_completion: CompletionList::new(),
            file_query: String::new(),
            file_query_start_pos: 0,
            working_dir: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            all_files: Vec::new(),
            file_cache_dirty: true,
            total_files_scanned: 0,
            files_truncated: false,
        }
    }

    /// Set the working directory for file completion
    pub fn set_working_dir(&mut self, path: impl Into<std::path::PathBuf>) {
        self.working_dir = path.into();
    }

    /// Update command completion state based on current input
    fn update_completion(&mut self) {
        let content = self.component.content();
        if content.starts_with('/') {
            let filtered: Vec<(String, String)> = SLASH_COMMANDS
                .iter()
                .filter(|(cmd, _)| cmd.starts_with(content))
                .map(|(cmd, desc)| ((*cmd).to_string(), (*desc).to_string()))
                .collect();
            self.command_completion.show(filtered);
        } else {
            self.command_completion.hide();
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
            self.component.clear();
            self.component.insert_str(cmd);
            self.component.insert_char(' ');
            self.command_completion.hide();
        }
    }

    /// Start file completion (@-mention)
    fn start_file_completion(&mut self) {
        self.file_query.clear();
        // Record cursor position as the start of query (after @)
        self.file_query_start_pos = self.component.cursor_pos();
        // Scan files once and cache
        self.ensure_file_cache();
        self.refresh_file_list();
    }

    /// Refresh file list from cache based on current query
    fn refresh_file_list(&mut self) {
        let filtered = if self.file_query.is_empty() {
            // Show first N files from cache when no query
            self.all_files
                .iter()
                .take(MAX_FILES_TO_DISPLAY)
                .cloned()
                .collect()
        } else {
            // Filter and limit results
            let mut filtered = Self::fuzzy_filter_files(&self.all_files, &self.file_query);
            filtered.truncate(MAX_FILES_TO_DISPLAY);
            filtered
        };
        self.file_completion.show(filtered);
    }

    /// Ensure file cache is populated (lazy loading)
    fn ensure_file_cache(&mut self) {
        if self.file_cache_dirty || self.all_files.is_empty() {
            let (files, count, truncated) = self.scan_files_limited();
            self.all_files = files;
            self.total_files_scanned = count;
            self.files_truncated = truncated;
            self.file_cache_dirty = false;
        }
    }

    /// Scan files recursively with limits, respecting .gitignore
    /// Returns (files, `total_count`, `was_truncated`)
    fn scan_files_limited(&self) -> (Vec<String>, usize, bool) {
        let gitignore = self.load_gitignore();
        let mut files = Vec::with_capacity(MAX_FILES_TO_SCAN);
        let mut count = 0usize;
        Self::scan_recursive_limited(&self.working_dir, "", &gitignore, &mut files, &mut count);
        let truncated = count >= MAX_FILES_TO_SCAN;
        // Sort: shorter paths first, then alphabetically
        files.sort_by(|a, b| {
            let a_parts = a.matches('/').count();
            let b_parts = b.matches('/').count();
            match a_parts.cmp(&b_parts) {
                std::cmp::Ordering::Equal => a.cmp(b),
                other => other,
            }
        });
        (files, count, truncated)
    }

    /// Load .gitignore patterns
    fn load_gitignore(&self) -> Vec<String> {
        let gitignore_path = self.working_dir.join(".gitignore");
        let mut patterns = vec![
            ".git".to_string(),
            ".gitignore".to_string(),
            "target/".to_string(),
            "node_modules/".to_string(),
        ];
        if let Ok(content) = std::fs::read_to_string(&gitignore_path) {
            for line in content.lines() {
                let line = line.trim();
                if !line.is_empty() && !line.starts_with('#') {
                    patterns.push(line.to_string());
                }
            }
        }
        patterns
    }

    /// Check if path matches gitignore pattern
    /// Check if path matches gitignore pattern
    fn matches_gitignore(path: &str, patterns: &[String]) -> bool {
        let path = path.trim_end_matches('/');
        for pattern in patterns {
            let pattern = pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            // Handle directory patterns (ending with /)
            let is_dir_pattern = pattern.ends_with('/');
            let clean_pattern = pattern.trim_end_matches('/');
            // Exact match
            if path == clean_pattern {
                return true;
            }
            // Check if any component matches
            if path.split('/').any(|part| {
                if is_dir_pattern {
                    part == clean_pattern
                } else {
                    part == clean_pattern || path.ends_with(&format!("/{clean_pattern}"))
                }
            }) {
                return true;
            }
            // Simple glob-like matching for *
            if clean_pattern.contains('*') {
                let parts: Vec<&str> = clean_pattern.split('*').collect();
                if parts.len() == 2 {
                    let prefix = parts[0];
                    let suffix = parts[1];
                    if (prefix.is_empty() || path.contains(prefix))
                        && (suffix.is_empty() || path.contains(suffix))
                    {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Recursively scan directory with limit
    fn scan_recursive_limited(
        base_dir: &std::path::Path,
        prefix: &str,
        gitignore: &[String],
        files: &mut Vec<String>,
        count: &mut usize,
    ) {
        if *count >= MAX_FILES_TO_SCAN {
            return;
        }
        let current_dir = if prefix.is_empty() {
            base_dir.to_path_buf()
        } else {
            base_dir.join(prefix)
        };
        if let Ok(entries) = std::fs::read_dir(&current_dir) {
            for entry in entries.flatten() {
                if *count >= MAX_FILES_TO_SCAN {
                    break;
                }
                let name = entry.file_name().to_string_lossy().to_string();
                let relative_path = if prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{prefix}/{name}")
                };
                // Check gitignore
                if Self::matches_gitignore(&relative_path, gitignore) {
                    continue;
                }
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_dir() {
                        files.push(format!("{relative_path}/"));
                        *count += 1;
                        // Recurse into subdirectory
                        Self::scan_recursive_limited(
                            base_dir,
                            &relative_path,
                            gitignore,
                            files,
                            count,
                        );
                    } else if metadata.is_file() {
                        files.push(relative_path);
                        *count += 1;
                    }
                }
            }
        }
    }

    /// Fuzzy filter files based on query (similar to fzf)
    /// Optimized: uses references to avoid cloning during filtering
    fn fuzzy_filter_files( files: &[String], query: &str) -> Vec<String> {
        if query.is_empty() {
            return files.iter().take(MAX_FILES_TO_DISPLAY).cloned().collect();
        }
        let query_lower = query.to_lowercase();
        let mut scored: Vec<(usize, i32, usize)> = files
            .iter()
            .enumerate()
            .filter_map(|(idx, file)| {
                // Case-insensitive match without allocating
                Self::fuzzy_match_case_insensitive(file, &query_lower).map(|score| (idx, score, file.len()))
            })
            .collect();
        // Sort by score (descending), then by length (ascending - shorter first)
        scored.sort_by(|a, b| match b.1.cmp(&a.1) {
            std::cmp::Ordering::Equal => a.2.cmp(&b.2),
            other => other,
        });
        scored
            .into_iter()
            .map(|(idx, _, _)| files[idx].clone())
            .collect()
    }

    /// Case-insensitive fuzzy matching (optimized: avoids lowercase allocation)
    fn fuzzy_match_case_insensitive(text: &str, pattern_lower: &str) -> Option<i32> {
        if pattern_lower.is_empty() {
            return Some(0);
        }
        let pattern_chars: Vec<char> = pattern_lower.chars().collect();
        let mut pattern_idx = 0;
        let mut score = 0i32;
        let mut consecutive_bonus = 0;
        let mut prev_match_idx: Option<usize> = None;

        for (text_idx, c) in text.chars().enumerate() {
            if pattern_idx < pattern_chars.len() {
                let pc = pattern_chars[pattern_idx];
                // Case-insensitive comparison
                let c_lower = c.to_lowercase().next()?;
                if c_lower == pc {
                    // Bonus for consecutive matches
                    if let Some(prev) = prev_match_idx {
                        if text_idx == prev + 1 {
                            consecutive_bonus += 1;
                            score += consecutive_bonus;
                        }
                    }
                    // Bonus for match at word boundary
                    if let Some(prev) = prev_match_idx {
                        if text_idx > 0 {
                            let prev_char = text.chars().nth(prev)?;
                            if prev_char == '/'
                                || prev_char == '-'
                                || prev_char == '_'
                                || prev_char == '.'
                            {
                                score += 3;
                            }
                        }
                    }
                    // Base score
                    score += 1;
                    prev_match_idx = Some(text_idx);
                    pattern_idx += 1;
                }
            }
        }

        if pattern_idx == pattern_chars.len() {
            score -= (text.len().saturating_sub(pattern_lower.len())) as i32;
            Some(score)
        } else {
            None
        }
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
        if let Some(selected) = self.file_completion.get_selected() {
            let current_pos = self.component.cursor_pos();
            // Delete the query part (from @ to current position)
            let query_len = current_pos.saturating_sub(self.file_query_start_pos);
            for _ in 0..query_len {
                self.component.backspace();
            }
            // Insert the selected file path
            self.component.insert_str(selected);
            self.file_completion.hide();
        }
    }

    /// Cancel file completion
    fn cancel_file_completion(&mut self) {
        self.file_completion.hide();
        self.file_query.clear();
    }

    /// Handle input when file completion is active
    fn handle_file_completion_input(
        &mut self,
        ev: &tuirealm::Event<crate::msg::UserEvent>,
    ) -> Msg {
        match *ev {
            // Enter or Tab: accept selected file
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            } | KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                self.accept_file_completion();
                Msg::InputChanged(self.component.content().to_string())
            }
            // Esc: cancel file completion
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.cancel_file_completion();
                Msg::Redraw
            }
            // Up arrow or Ctrl+P: navigate up
            tuirealm::Event::Keyboard(KeyEvent {
code: Key::Up, modifiers: KeyModifiers::NONE } | KeyEvent {
code: Key::Char('p'), modifiers: KeyModifiers::CONTROL }) => {
                self.file_completion_prev();
                Msg::Redraw
            }
            // Down arrow or Ctrl+N: navigate down
            tuirealm::Event::Keyboard(KeyEvent {
code: Key::Down, modifiers: KeyModifiers::NONE } | KeyEvent {
code: Key::Char('n'), modifiers: KeyModifiers::CONTROL }) => {
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
            // Regular character: add to query and filter
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                self.component.insert_char(c);
                self.file_query.push(c);
                self.refresh_file_list();
                Msg::InputChanged(self.component.content().to_string())
            }
            // Backspace: remove from query
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.backspace();
                self.file_query.pop();
                if self.file_query.is_empty() && self.component.content().ends_with('@') {
                    // Cancel if query is empty and only @ remains
                    self.cancel_file_completion();
                } else {
                    self.refresh_file_list();
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
        // Render completion dropdown above input if visible
        if self.command_completion.is_visible() && !self.command_completion.items().is_empty() {
            let completion_height = self.command_completion.len().min(4) as u16;
            let completion_area = Rect {
                x: area.x,
                y: area.y.saturating_sub(completion_height),
                width: area.width,
                height: completion_height,
            };

            // Clear the area first
            frame.render_widget(tuirealm::ratatui::widgets::Clear, completion_area);

            // Render completion items with command and description
            let items: Vec<tuirealm::ratatui::text::Line> = self
                .command_completion
                .items()
                .iter()
                .enumerate()
                .map(|(i, (cmd, desc))| {
                    let is_selected = i == self.command_completion.selected_index();
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
                })
                .collect();

            let completion_widget = tuirealm::ratatui::widgets::Paragraph::new(
                tuirealm::ratatui::text::Text::from(items),
            );
            frame.render_widget(completion_widget, completion_area);
        }

        // Render file completion dropdown if visible
        if self.file_completion.is_visible() && !self.file_completion.items().is_empty() {
            // Status line at bottom
            let status_text = if self.files_truncated {
                format!(
                    " {} / {}+ files",
                    self.file_completion.len(),
                    self.total_files_scanned
                )
            } else {
                format!(
                    " {} / {} files",
                    self.file_completion.len(),
                    self.total_files_scanned
                )
            };
            let display_count = self.file_completion.len().min(8);
            let file_completion_height = (display_count + 1) as u16; // +1 for status line
            let file_completion_area = Rect {
                x: area.x,
                y: area.y.saturating_sub(file_completion_height),
                width: area.width,
                height: file_completion_height,
            };

            // Clear the area first
            frame.render_widget(tuirealm::ratatui::widgets::Clear, file_completion_area);

            // Build file items
            let mut items: Vec<tuirealm::ratatui::text::Line> = self
                .file_completion
                .items()
                .iter()
                .take(8)
                .enumerate()
                .map(|(i, file)| {
                    let is_selected = i == self.file_completion.selected_index();
                    let is_dir = file.ends_with('/');
                    let file_style = if is_selected {
                        // Selected: accent_system fg with bold (same as command completion)
                        tuirealm::ratatui::style::Style::default()
                            .fg(colors::accent_system())
                            .add_modifier(tuirealm::ratatui::style::Modifier::BOLD)
                    } else {
                        // Not selected
                        if is_dir {
                            tuirealm::ratatui::style::Style::default().fg(colors::accent_system())
                        } else {
                            tuirealm::ratatui::style::Style::default().fg(colors::text_primary())
                        }
                    };
                    tuirealm::ratatui::text::Line::from(tuirealm::ratatui::text::Span::styled(
                        file.as_str(),
                        file_style,
                    ))
                })
                .collect();

            // Add status line at the bottom
            let status_style = tuirealm::ratatui::style::Style::default()
                .fg(colors::text_muted())
                .add_modifier(tuirealm::ratatui::style::Modifier::DIM);
            items.push(tuirealm::ratatui::text::Line::from(
                tuirealm::ratatui::text::Span::styled(status_text, status_style),
            ));

            let file_completion_widget = tuirealm::ratatui::widgets::Paragraph::new(
                tuirealm::ratatui::text::Text::from(items),
            );
            frame.render_widget(file_completion_widget, file_completion_area);
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
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('q'),
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
            // Pass through to normal input handler for other keys
            _ => self.handle_normal_input(ev),
        }
    }

    /// Parse slash command from input
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
            cmd => Some(Msg::CommandUnknown(cmd.to_string())),
        }
    }

    /// Handle input in normal mode - text editing
    fn handle_normal_input(&mut self, ev: &tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        // File completion mode - handle special keys first
        if self.file_completion.is_visible() {
            return Some(self.handle_file_completion_input(ev));
        }

        match *ev {
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
                self.component.insert_char(c);
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
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
                let content = self.component.submit();
                if content.is_empty() {
                    None
                } else if let Some(cmd_msg) = Self::parse_command(&content) {
                    // It's a command, return the command message
                    Some(cmd_msg)
                } else {
                    // Regular input
                    Some(Msg::InputSubmit(content))
                }
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.backspace();
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Delete,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.delete_char();
                self.update_completion();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Left,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.move_left();
                None
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Right,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.move_right();
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
                self.component.move_to_start_of_line();
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
                self.component.move_to_end_of_line();
                None
            }
            // Alt+B: move backward one word
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('b'),
                modifiers: KeyModifiers::ALT,
            }) => {
                self.component.move_word_left();
                None
            }
            // Alt+F: move forward one word
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('f'),
                modifiers: KeyModifiers::ALT,
            }) => {
                self.component.move_word_right();
                None
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('j'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.insert_newline();
                Some(Msg::InputChanged(self.component.content().to_string()))
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
                    self.component.move_up();
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
                    self.component.move_down();
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
                    Some(Msg::ShowStatusMessage(
                        "Press Ctrl+C again to exit".to_string(),
                        1000, // 1000ms = 1 second, matches double-press detection
                    ))
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
            // Toggle YOLO mode with Ctrl+Y
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('y'),
                modifiers: KeyModifiers::CONTROL,
            }) => Some(Msg::ToggleYoloMode),
            _ => None,
        }
    }
}
