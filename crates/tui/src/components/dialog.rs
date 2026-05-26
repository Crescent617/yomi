//! Generic select dialog component for TUI
//!
//! Provides a modal-like dialog for selecting from a list of options.
//! Used for permission confirmation and other user choices.

use tuirealm::{
    command::{Cmd, CmdResult, Direction as CmdDirection},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{
        layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
        style::{Modifier, Style},
        widgets::{Block, BorderType, Borders, Clear, Paragraph},
        Frame,
    },
    state::{State, StateValue},
};

use crate::{attr, msg::Msg, theme::colors};
use unicode_width::UnicodeWidthStr;

/// Dialog result type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogResult {
    Selected(usize),
    Cancelled,
}

/// Maximum content width for dialogs
pub const MAX_CONTENT_WIDTH: u16 = 160;

/// Create a dialog block with centered title, top/bottom borders, and accent styling
pub fn dialog_block(title: &str) -> Block<'_> {
    Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_type(BorderType::Rounded)
        .border_style(colors::accent_system())
        .title_style(
            Style::default()
                .fg(colors::accent_system())
                .add_modifier(Modifier::BOLD),
        )
}

/// Calculate the inner content area for a dialog, constrained by [`MAX_CONTENT_WIDTH`]
pub fn dialog_inner_area(dialog_area: Rect) -> Rect {
    let min_margin = 1;
    let content_width = dialog_area
        .width
        .saturating_sub(min_margin * 2)
        .min(MAX_CONTENT_WIDTH);
    let h_pad = (dialog_area.width.saturating_sub(content_width)) / 2;
    dialog_area.inner(Margin {
        horizontal: h_pad,
        vertical: 1,
    })
}

/// A generic select dialog component
#[derive(Debug)]
pub struct SelectDialog {
    props: Props,
    /// Dialog title
    title: String,
    /// Options to select from (includes "Other..." for custom input)
    options: Vec<String>,
    /// Currently selected index
    selected: usize,
    /// Whether the dialog is active/visible
    active: bool,
    /// Optional message/body text (shown above options)
    message: Option<String>,
    /// Custom free-text input when user selects "Other..."
    custom_input: String,
    /// Whether focus is on the custom input line (true) or option list (false)
    input_focused: bool,
}

impl SelectDialog {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            props: Props::default(),
            title: title.into(),
            options: Vec::new(),
            selected: 0,
            active: false,
            message: None,
            custom_input: String::new(),
            input_focused: false,
        }
    }

    /// Show the dialog with given options ("Other..." is appended by the caller)
    pub fn show(&mut self, options: Vec<String>, message: Option<String>) {
        self.options = options;
        self.selected = 0;
        self.message = message;
        self.active = true;
        self.custom_input.clear();
        self.input_focused = false;
    }

    /// Hide the dialog
    pub fn hide(&mut self) {
        self.active = false;
        self.custom_input.clear();
        self.input_focused = false;
    }

    /// Check if dialog is active
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Move selection up
    const fn select_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        } else {
            self.selected = self.options.len().saturating_sub(1);
        }
    }

    /// Move selection down
    const fn select_down(&mut self) {
        if self.selected + 1 < self.options.len() {
            self.selected += 1;
        } else {
            self.selected = 0;
        }
    }

    /// Get the currently selected option
    const fn current_selection(&self) -> Option<usize> {
        if self.active && !self.options.is_empty() {
            Some(self.selected)
        } else {
            None
        }
    }

    /// Check if the currently selected option is the custom "Other..." entry
    const fn is_custom_selected(&self) -> bool {
        self.active && !self.options.is_empty() && self.selected == self.options.len() - 1
    }

    /// Insert a character into the custom input buffer
    pub fn insert_char(&mut self, c: char) {
        self.custom_input.push(c);
    }

    /// Delete the last character from the custom input buffer
    pub fn backspace(&mut self) {
        self.custom_input.pop();
    }

    /// Toggle focus between option list and custom input line
    pub fn toggle_input_focus(&mut self) {
        self.input_focused = !self.input_focused;
    }

    /// Get the current custom input text
    pub fn custom_input(&self) -> &str {
        &self.custom_input
    }

    /// Check if focus is on the custom input line
    pub const fn is_input_focused(&self) -> bool {
        self.input_focused
    }

    fn render_dialog(&self, frame: &mut Frame, area: Rect) {
        // 宽度拉满
        let dialog_width = area.width;
        let message_height = self
            .message
            .as_ref()
            .map_or(0, |m| m.lines().count() as u16);
        // +1 for the custom input line
        let dialog_height =
            (6 + message_height + self.options.len() as u16).min(area.height.saturating_sub(4));

        let dialog_area = Rect {
            x: area.x,
            y: area.y + (area.height - dialog_height) / 2,
            width: dialog_width,
            height: dialog_height,
        };

        // Clear the background behind dialog
        frame.render_widget(Clear, dialog_area);

        let block = dialog_block(self.title.as_str());
        let inner = dialog_inner_area(dialog_area);

        let constraints = if message_height > 0 {
            vec![
                Constraint::Length(message_height + 1), // Message + padding
                Constraint::Min(1),                     // Options list
                Constraint::Length(1),                  // Custom input line
            ]
        } else {
            vec![
                Constraint::Min(1),    // Options list
                Constraint::Length(1), // Custom input line
            ]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        // Render message if present, left aligned
        if let Some(message) = &self.message {
            let message_para = Paragraph::new(message.as_str())
                .alignment(Alignment::Left)
                .style(Style::default().fg(colors::text_secondary()));
            frame.render_widget(message_para, chunks[0]);
        }

        // Render options as left-aligned paragraphs
        let list_area = if message_height > 0 {
            chunks[1]
        } else {
            chunks[0]
        };

        let input_area = if message_height > 0 {
            chunks[2]
        } else {
            chunks[1]
        };

        let max_visible = list_area.height as usize;
        let item_count = self.options.len().min(max_visible);

        if item_count > 0 {
            let option_constraints: Vec<Constraint> =
                (0..item_count).map(|_| Constraint::Length(1)).collect();
            let option_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(option_constraints)
                .split(list_area);

            for (idx, option) in self.options.iter().enumerate().take(item_count) {
                let prefix = if idx == self.selected { "▸ " } else { "  " };
                let content = format!("{prefix}{option}");

                let style = if idx == self.selected {
                    Style::default()
                        .fg(colors::accent_system())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(colors::text_primary())
                };

                let para = Paragraph::new(content)
                    .alignment(Alignment::Left)
                    .style(style);

                frame.render_widget(para, option_chunks[idx]);
            }
        }

        // Render custom input line
        let input_prefix = "> ";
        let input_text = if self.custom_input.is_empty() {
            if self.input_focused {
                format!("{input_prefix}_")
            } else {
                format!("{input_prefix}Other...")
            }
        } else {
            format!("{input_prefix}{}", self.custom_input)
        };

        let input_style = if self.input_focused {
            Style::default()
                .fg(colors::text_primary())
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(colors::text_muted())
        };

        let input_para = Paragraph::new(input_text)
            .alignment(Alignment::Left)
            .style(input_style);
        frame.render_widget(input_para, input_area);

        // Set cursor when input line is focused
        if self.input_focused {
            let cursor_x =
                input_area.x + input_prefix.width() as u16 + self.custom_input.width() as u16;
            let cursor_y = input_area.y;
            frame.set_cursor_position(tuirealm::ratatui::layout::Position::new(cursor_x, cursor_y));
        }

        // Render the border block last (on top)
        frame.render_widget(block, dialog_area);
    }
}

impl Component for SelectDialog {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if self.active {
            self.render_dialog(frame, area);
        }
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.props.get(attr).map(|v| v.into())
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(attr::DIALOG_SHOW) => {
                if let AttrValue::String(data) = value {
                    // Format: "title\x00option1\x00option2\x00...\x00message"
                    let parts: Vec<&str> = data.split('\x00').collect();
                    if parts.len() >= 2 {
                        let title = parts[0].to_string();
                        let message = if parts.len() > 2 {
                            Some(parts[parts.len() - 1].to_string())
                        } else {
                            None
                        };
                        let options: Vec<String> = parts
                            [1..parts.len() - usize::from(message.is_some())]
                            .iter()
                            .map(|s| (*s).to_string())
                            .collect();
                        self.title = title;
                        self.show(options, message);
                    }
                }
            }
            Attribute::Custom(attr::DIALOG_HIDE) => {
                self.hide();
            }
            _ => {
                self.props.set(attr, value);
            }
        }
    }

    fn state(&self) -> State {
        if let Some(idx) = self.current_selection() {
            // Use String to represent the selected index
            State::Single(StateValue::String(idx.to_string()))
        } else {
            State::None
        }
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        if !self.active {
            return CmdResult::NoChange;
        }

        match cmd {
            Cmd::Move(CmdDirection::Up) => {
                self.select_up();
                CmdResult::Changed(State::Single(StateValue::String(self.selected.to_string())))
            }
            Cmd::Move(CmdDirection::Down) => {
                self.select_down();
                CmdResult::Changed(State::Single(StateValue::String(self.selected.to_string())))
            }
            Cmd::Submit => {
                if let Some(idx) = self.current_selection() {
                    self.hide();
                    CmdResult::Submit(State::Single(StateValue::String(idx.to_string())))
                } else {
                    CmdResult::NoChange
                }
            }
            Cmd::Cancel => {
                self.hide();
                CmdResult::Submit(State::None)
            }
            _ => CmdResult::NoChange,
        }
    }
}

/// Component wrapper for `SelectDialog`
#[derive(Debug)]
pub struct SelectDialogComponent {
    component: SelectDialog,
}

impl SelectDialogComponent {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            component: SelectDialog::new(title),
        }
    }

    pub fn show(&mut self, options: Vec<String>, message: Option<String>) {
        self.component.show(options, message);
    }

    pub fn hide(&mut self) {
        self.component.hide();
    }

    pub const fn is_active(&self) -> bool {
        self.component.is_active()
    }
}

impl Component for SelectDialogComponent {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        self.component.view(frame, area);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
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

impl AppComponent<Msg, crate::msg::UserEvent> for SelectDialogComponent {
    fn on(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        use tuirealm::event::{Key, KeyEvent, KeyModifiers};
        use Event::Keyboard;

        tracing::trace!(
            "Dialog received event: {:?}, active={}",
            ev,
            self.component.is_active()
        );

        if !self.component.is_active() {
            return None;
        }

        match *ev {
            // Tab: toggle focus between option list and custom input line
            Keyboard(KeyEvent {
                code: Key::Tab,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.toggle_input_focus();
                Some(Msg::Redraw)
            }
            // Up arrow or Ctrl+P or 'k': navigate up (only in list mode)
            Keyboard(
                KeyEvent {
                    code: Key::Up | Key::Char('k'),
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('p'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) if !self.component.is_input_focused() => {
                self.component.select_up();
                Some(Msg::Redraw)
            }
            // Down arrow or Ctrl+N or 'j': navigate down (only in list mode)
            Keyboard(
                KeyEvent {
                    code: Key::Down | Key::Char('j'),
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('n'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) if !self.component.is_input_focused() => {
                self.component.select_down();
                Some(Msg::Redraw)
            }
            // Character input: only when input line is focused
            Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) if self.component.is_input_focused() => {
                self.component.insert_char(c);
                Some(Msg::Redraw)
            }
            // Backspace: only when input line is focused
            Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) if self.component.is_input_focused() => {
                self.component.backspace();
                Some(Msg::Redraw)
            }
            // Enter: behavior depends on focus and selection
            Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) => {
                if self.component.is_input_focused() && !self.component.custom_input().is_empty() {
                    // Submit custom text answer
                    let text = self.component.custom_input().to_string();
                    self.component.hide();
                    tracing::info!("Dialog: custom input submitted: {}", text);
                    Some(Msg::DialogCustomInput(text))
                } else if !self.component.is_input_focused() && self.component.is_custom_selected()
                {
                    // Selected "Other..." in list mode: switch to input focus
                    self.component.toggle_input_focus();
                    Some(Msg::Redraw)
                } else if let Some(idx) = self.component.current_selection() {
                    // Selected a regular option
                    tracing::info!("Dialog: Enter pressed, selection={idx}");
                    self.component.hide();
                    Some(Msg::DialogSelected(idx))
                } else {
                    None
                }
            }
            // Esc: cancel dialog, or exit input focus back to list
            Keyboard(KeyEvent {
                code: Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => {
                if self.component.is_input_focused() {
                    self.component.toggle_input_focus();
                    Some(Msg::Redraw)
                } else {
                    tracing::info!("Dialog: Esc pressed");
                    self.component.hide();
                    Some(Msg::DialogCancelled)
                }
            }
            _ => None,
        }
    }
}
