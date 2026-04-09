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
use unicode_width::UnicodeWidthStr;

use crate::{msg::Msg, theme::colors};

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

    pub fn insert_char(&mut self, c: char) {
        self.content.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        self.content.insert_str(self.cursor_pos, s);
        self.cursor_pos += s.len();
    }

    pub fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            let mut idx = self.cursor_pos - 1;
            while idx > 0 && !self.content.is_char_boundary(idx) {
                idx -= 1;
            }
            self.content.drain(idx..self.cursor_pos);
            self.cursor_pos = idx;
        }
    }

    pub fn delete_char(&mut self) {
        if self.cursor_pos < self.content.len() {
            let mut idx = self.cursor_pos + 1;
            while idx < self.content.len() && !self.content.is_char_boundary(idx) {
                idx += 1;
            }
            self.content.drain(self.cursor_pos..idx);
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor_pos > 0 {
            let mut idx = self.cursor_pos - 1;
            while idx > 0 && !self.content.is_char_boundary(idx) {
                idx -= 1;
            }
            self.cursor_pos = idx;
        }
    }

    pub fn move_right(&mut self) {
        if self.cursor_pos < self.content.len() {
            let mut idx = self.cursor_pos + 1;
            while idx < self.content.len() && !self.content.is_char_boundary(idx) {
                idx += 1;
            }
            self.cursor_pos = idx.min(self.content.len());
        }
    }

    pub const fn move_to_start(&mut self) {
        self.cursor_pos = 0;
    }

    pub const fn move_to_end(&mut self) {
        self.cursor_pos = self.content.len();
    }

    pub fn clear(&mut self) {
        self.content.clear();
        self.cursor_pos = 0;
    }

    pub fn insert_newline(&mut self) {
        self.content.insert(self.cursor_pos, '\n');
        self.cursor_pos += 1;
    }

    /// Delete from cursor to start of line (like ctrl-u in bash)
    pub fn kill_to_start_of_line(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        // Find the start of current line
        let line_start = self.content[..self.cursor_pos]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        self.content.drain(line_start..self.cursor_pos);
        self.cursor_pos = line_start;
    }

    /// Delete word backward (like ctrl-w in bash)
    pub fn delete_word(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        // Skip trailing whitespace
        let mut pos = self.cursor_pos;
        while pos > 0 {
            let mut prev = pos - 1;
            while prev > 0 && !self.content.is_char_boundary(prev) {
                prev -= 1;
            }
            if self.content[prev..pos]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                pos = prev;
            } else {
                break;
            }
        }
        // Now find the start of the word
        while pos > 0 {
            let mut prev = pos - 1;
            while prev > 0 && !self.content.is_char_boundary(prev) {
                prev -= 1;
            }
            if self.content[prev..pos]
                .chars()
                .next()
                .unwrap_or(' ')
                .is_whitespace()
            {
                break;
            }
            pos = prev;
        }
        self.content.drain(pos..self.cursor_pos);
        self.cursor_pos = pos;
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

impl MockComponent for InputMock {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Calculate cursor position
        let cursor_line = self.content[..self.cursor_pos.min(self.content.len())]
            .chars()
            .filter(|&c| c == '\n')
            .count();

        // Calculate scroll offset to keep cursor visible
        let visible_height = area.height.saturating_sub(2).max(1) as usize; // -2 for top/bottom borders, min 1
                                                                            // Use matches('\n') to correctly count lines including trailing newlines
        let total_lines = self.content.matches('\n').count() + 1;
        let scroll_offset = if total_lines > visible_height {
            // Scroll so cursor is visible (prefer showing cursor near bottom)
            cursor_line
                .saturating_sub(visible_height.saturating_sub(1))
                .min(total_lines.saturating_sub(visible_height))
        } else {
            0
        };

        // Render only visible lines
        // Use split('\n') instead of lines() to handle trailing newlines correctly
        let all_lines: Vec<Line> = self
            .content
            .split('\n')
            .enumerate()
            .map(|(i, line)| {
                let prefix = if i == 0 { "❯ " } else { "│ " };
                Line::from(vec![
                    Span::styled(
                        prefix,
                        Style::default()
                            .fg(colors::accent_user())
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        line.to_string(),
                        Style::default().fg(colors::text_primary()),
                    ),
                ])
            })
            .collect();

        // Slice visible lines based on scroll offset
        let start = scroll_offset.min(all_lines.len());
        let end = (scroll_offset + visible_height).min(all_lines.len());
        let visible_lines: Vec<Line> = all_lines[start..end].to_vec();

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
            tuirealm::ratatui::text::Text::from(visible_lines)
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

        // Set cursor position (adjusted for scroll)
        let line_start = self.content[..self.cursor_pos.min(self.content.len())]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let line_content = &self.content[line_start..self.cursor_pos.min(self.content.len())];
        let col = line_content.width();

        let cursor_x = area.x + 2 + col as u16; // 2 for "❯ " prefix
        let cursor_y = area.y + 1 + (cursor_line - scroll_offset) as u16; // +1 for top border, adjusted for scroll

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
/// Note: Mode is managed by Model, not by this component
pub struct InputComponent {
    component: InputMock,
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
        }
    }
}

impl MockComponent for InputComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.component.view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.component.query(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        self.component.attr(attr, value);
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
        self.handle_input(ev)
    }
}

impl InputComponent {
    /// Handle all input events - mode-aware handling is done by Model
    fn handle_input(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        match ev {
            // Browse mode navigation keys - sent to Model (Model decides based on mode)
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('j'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollDown),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('k'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollUp),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('q'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ToggleBrowseMode),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('d'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::PageDown),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::PageUp),
            // Normal mode: character input
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.insert_char(c);
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::SHIFT,
            }) => {
                self.component.insert_char(c);
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) => {
                let content = self.component.submit();
                if content.is_empty() {
                    None
                } else {
                    Some(Msg::InputSubmit(content))
                }
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.backspace();
                Some(Msg::InputChanged(self.component.content().to_string()))
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Delete,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.delete_char();
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
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Home,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.move_to_start();
                None
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::End,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.move_to_end();
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
                self.component.delete_word();
                Some(Msg::InputChanged(self.component.content().to_string()))
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
                    // First Ctrl+C: clear input and show hint in status bar
                    Some(Msg::ShowStatusMessage(
                        "Press Ctrl+C again to exit".to_string(),
                    ))
                }
            }
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Up,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollUp),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollDown),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::PageUp,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollUp),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::PageDown,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ScrollDown),
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => Some(Msg::ToggleThinking),
            // Toggle browse mode with Ctrl+O
            tuirealm::Event::Keyboard(KeyEvent {
                code: Key::Char('o'),
                modifiers: KeyModifiers::CONTROL,
            }) => Some(Msg::ToggleBrowseMode),
            // Mouse scroll events
            tuirealm::Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                ..
            }) => Some(Msg::ScrollUp),
            tuirealm::Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                ..
            }) => Some(Msg::ScrollDown),
            _ => None,
        }
    }
}
