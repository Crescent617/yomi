//! Command palette component for TUI
//!
//! Provides a VS Code-style command palette for quick access to actions.
//! Triggered by / when input box is empty.

use tuirealm::{
    command::{Cmd, CmdResult},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
        style::{Modifier, Style},
        widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    },
    Component, Frame, MockComponent, State, StateValue,
};

use unicode_width::UnicodeWidthStr;
use crate::{components::input_edit::{TextBuffer, TextInput}, msg::Msg, theme::colors};

/// A command that can be executed from the palette
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub label: String,
}

impl Command {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }
}

/// Command palette component
#[derive(Debug)]
pub struct CommandPalette {
    props: Props,
    commands: Vec<Command>,
    filtered: Vec<usize>,
    selected: usize,
    input: TextBuffer,
    visible: bool,
}

impl CommandPalette {

    pub fn with_default_commands() -> Self {
        let mut palette = Self::new();
        palette.commands = vec![
            Command::new("new_chat", "New Chat"),
            Command::new("clear", "Clear History"),
            Command::new("toggle_browse", "Toggle Browse Mode"),
            Command::new("toggle_yolo", "Toggle YOLO Mode"),
            Command::new("scroll_top", "Scroll to Top"),
            Command::new("scroll_bottom", "Scroll to Bottom"),
        ];
        palette
    }
    pub fn new() -> Self {
        Self {
            props: Props::default(),
            commands: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            input: TextBuffer::new(),
            visible: false,
        }
    }

    pub fn show(&mut self, commands: Vec<Command>) {
        self.commands = commands;
        self.input.clear();
        self.selected = 0;
        self.visible = true;
        self.update_filtered();
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.input.clear();
    }

    pub const fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn set_search(&mut self, search: String) {
        self.input = TextBuffer::with_content(search);
        self.selected = 0;
        self.update_filtered();
    }

    pub fn search(&self) -> &str {
        self.input.content()
    }

    /// Get mutable access to the input buffer for advanced editing
    pub fn input_mut(&mut self) -> &mut crate::components::input_edit::TextBuffer {
        &mut self.input
    }

    fn select_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        } else if !self.filtered.is_empty() {
            self.selected = self.filtered.len() - 1;
        }
    }

    fn select_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        } else {
            self.selected = 0;
        }
    }

    fn current_selection(&self) -> Option<String> {
        if self.visible && !self.filtered.is_empty() {
            self.filtered.get(self.selected).map(|idx| self.commands[*idx].id.clone())
        } else {
            None
        }
    }

    fn update_filtered(&mut self) {
        let search_lower = self.input.content().to_lowercase();
        self.filtered = self
            .commands
            .iter()
            .enumerate()
            .filter(|(_, cmd)| {
                if search_lower.is_empty() {
                    true
                } else {
                    cmd.label.to_lowercase().contains(&search_lower)
                        || cmd.id.to_lowercase().contains(&search_lower)
                }
            })
            .map(|(idx, _)| idx)
            .collect();

        if self.selected >= self.filtered.len() {
            self.selected = self.filtered.len().saturating_sub(1);
        }
    }

    pub fn insert_char(&mut self, c: char) {
        self.input.insert_char(c);
        self.update_filtered();
    }

    pub fn backspace(&mut self) {
        self.input.backspace();
        self.update_filtered();
    }

    fn render_palette(&self, frame: &mut Frame, area: Rect) {
        let palette_width = (f32::from(area.width) * 0.6).clamp(40.0, 80.0) as u16;
        let search_height = 3u16;
        let max_list_height = 10u16;
        let list_height = (self.filtered.len() as u16).min(max_list_height);
        let palette_height = search_height + list_height + 2;

        let palette_area = Rect {
            x: area.x + (area.width - palette_width) / 2,
            y: area.y + (area.height - palette_height) / 3,
            width: palette_width,
            height: palette_height,
        };

        frame.render_widget(Clear, palette_area);

        let block = Block::default()
            .title("Command Palette")
            .borders(Borders::ALL)
            .border_style(colors::accent_system())
            .title_style(
                Style::default()
                    .fg(colors::accent_system())
                    .add_modifier(Modifier::BOLD),
            );

        let inner = palette_area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(1)])
            .split(inner);

        let search_prefix = "> ";
        let search_text = format!("{}{}", search_prefix, self.input.content());
        let search_para = Paragraph::new(search_text).style(
            Style::default()
                .fg(colors::text_primary())
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(search_para, chunks[0]);

        let separator = Block::default()
            .borders(Borders::BOTTOM)
            .border_style(Style::default().fg(colors::border()));
        let separator_area = Rect {
            x: inner.x,
            y: chunks[0].y + 1,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(separator, separator_area);

        if !self.filtered.is_empty() {
            let items: Vec<ListItem> = self
                .filtered
                .iter()
                .enumerate()
                .map(|(display_idx, cmd_idx)| {
                    let prefix = if display_idx == self.selected { "▸ " } else { "  " };
                    let content = format!("{}{}", prefix, self.commands[*cmd_idx].label);

                    let style = if display_idx == self.selected {
                        Style::default()
                            .fg(colors::accent_system())
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(colors::text_primary())
                    };

                    ListItem::new(content).style(style)
                })
                .collect();

            let list = List::new(items).block(Block::default());
            frame.render_widget(list, chunks[1]);
        } else if !self.input.is_empty() {
            let no_results = Paragraph::new("No matching commands")
                .alignment(Alignment::Center)
                .style(Style::default().fg(colors::text_muted()));
            frame.render_widget(no_results, chunks[1]);
        }

        frame.render_widget(block, palette_area);

        // Cursor position: after the "> " prefix in the search box
        // Use display width for proper handling of CJK/multi-byte characters
        let search_width = self.input.content().width() as u16;
        let cursor_x = chunks[0].x + 2 + search_width;
        let cursor_y = chunks[0].y;
        frame.set_cursor_position(tuirealm::ratatui::layout::Position::new(cursor_x, cursor_y));
    }
}

impl Default for CommandPalette {
    fn default() -> Self {
        Self::new()
    }
}

impl MockComponent for CommandPalette {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if self.visible {
            self.render_palette(frame, area);
        }
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom("show") => {
                self.visible = true;
                self.input.clear();
                self.selected = 0;
                self.update_filtered();
            }
            Attribute::Custom("hide") => {
                self.hide();
            }
            Attribute::Custom("search") => {
                if let AttrValue::String(search) = value {
                    self.set_search(search);
                }
            }
            _ => {
                self.props.set(attr, value);
            }
        }
    }

    fn state(&self) -> State {
        if let Some(id) = self.current_selection() {
            State::One(StateValue::String(id))
        } else {
            State::None
        }
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        if !self.visible {
            return CmdResult::None;
        }

        match cmd {
            Cmd::Move(tuirealm::command::Direction::Up) => {
                self.select_up();
                CmdResult::Changed(State::One(StateValue::Usize(self.selected)))
            }
            Cmd::Move(tuirealm::command::Direction::Down) => {
                self.select_down();
                CmdResult::Changed(State::One(StateValue::Usize(self.selected)))
            }
            Cmd::Submit => {
                if let Some(id) = self.current_selection() {
                    self.hide();
                    CmdResult::Submit(State::One(StateValue::String(id)))
                } else {
                    CmdResult::None
                }
            }
            Cmd::Cancel => {
                self.hide();
                CmdResult::Submit(State::None)
            }
            Cmd::Type(c) => {
                self.insert_char(c);
                CmdResult::Changed(State::One(StateValue::String(self.input.content().to_string())))
            }
            Cmd::Delete => {
                self.backspace();
                CmdResult::Changed(State::One(StateValue::String(self.input.content().to_string())))
            }
            _ => CmdResult::None,
        }
    }
}

#[derive(Debug)]
pub struct CommandPaletteComponent {
    component: CommandPalette,
}

impl CommandPaletteComponent {
    pub fn new() -> Self {
        Self {
            component: CommandPalette::with_default_commands(),
        }
    }

    pub fn show(&mut self, commands: Vec<Command>) {
        self.component.show(commands);
    }

    pub fn hide(&mut self) {
        self.component.hide();
    }

    pub const fn is_visible(&self) -> bool {
        self.component.is_visible()
    }
}

impl Default for CommandPaletteComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl MockComponent for CommandPaletteComponent {
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

impl Component<Msg, crate::msg::UserEvent> for CommandPaletteComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        use tuirealm::event::{Key, KeyEvent, KeyModifiers};
        use tuirealm::Event::Keyboard;

        if !self.component.is_visible() {
            return None;
        }

        match ev {
            Keyboard(KeyEvent {
                code: Key::Up,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.select_up();
                Some(Msg::Redraw)
            }
            Keyboard(KeyEvent {
                code: Key::Down,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.select_down();
                Some(Msg::Redraw)
            }
            Keyboard(KeyEvent {
                code: Key::Char('n'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.select_down();
                Some(Msg::Redraw)
            }
            Keyboard(KeyEvent {
                code: Key::Char('p'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.select_up();
                Some(Msg::Redraw)
            }
            Keyboard(KeyEvent {
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) => {
                if let Some(id) = self.component.current_selection() {
                    self.component.hide();
                    Some(Msg::CommandSelected(id))
                } else {
                    None
                }
            }
            Keyboard(KeyEvent {
                code: Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.hide();
                Some(Msg::CloseCommandPalette)
            }
            Keyboard(KeyEvent {
                code: Key::Char(c),
                modifiers: KeyModifiers::NONE | KeyModifiers::SHIFT,
            }) => {
                self.component.insert_char(c);
                Some(Msg::Redraw)
            }
            Keyboard(KeyEvent {
                code: Key::Backspace,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.backspace();
                Some(Msg::Redraw)
            }
            // Ctrl+A: move to start of line
            Keyboard(KeyEvent {
                code: Key::Char('a'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.input_mut().move_to_start_of_line();
                Some(Msg::Redraw)
            }
            // Ctrl+E: move to end of line
            Keyboard(KeyEvent {
                code: Key::Char('e'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.input_mut().move_to_end_of_line();
                Some(Msg::Redraw)
            }
            // Ctrl+U: delete to start of line
            Keyboard(KeyEvent {
                code: Key::Char('u'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.input_mut().kill_to_start_of_line();
                self.component.update_filtered();
                Some(Msg::Redraw)
            }
            // Ctrl+K: delete to end of line
            Keyboard(KeyEvent {
                code: Key::Char('k'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.input_mut().kill_to_end_of_line();
                self.component.update_filtered();
                Some(Msg::Redraw)
            }
            // Ctrl+W: delete word backward
            Keyboard(KeyEvent {
                code: Key::Char('w'),
                modifiers: KeyModifiers::CONTROL,
            }) => {
                self.component.input_mut().delete_word_backward();
                self.component.update_filtered();
                Some(Msg::Redraw)
            }
            // Alt+D: delete word forward
            Keyboard(KeyEvent {
                code: Key::Char('d'),
                modifiers: KeyModifiers::ALT,
            }) => {
                self.component.input_mut().delete_word_forward();
                self.component.update_filtered();
                Some(Msg::Redraw)
            }
            // Alt+B: move word backward
            Keyboard(KeyEvent {
                code: Key::Char('b'),
                modifiers: KeyModifiers::ALT,
            }) => {
                self.component.input_mut().move_word_left();
                Some(Msg::Redraw)
            }
            // Alt+F: move word forward
            Keyboard(KeyEvent {
                code: Key::Char('f'),
                modifiers: KeyModifiers::ALT,
            }) => {
                self.component.input_mut().move_word_right();
                Some(Msg::Redraw)
            }
            // Left arrow: move left
            Keyboard(KeyEvent {
                code: Key::Left,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.input_mut().move_left();
                Some(Msg::Redraw)
            }
            // Right arrow: move right
            Keyboard(KeyEvent {
                code: Key::Right,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.input_mut().move_right();
                Some(Msg::Redraw)
            }
            // Home: move to start of line
            Keyboard(KeyEvent {
                code: Key::Home,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.input_mut().move_to_start_of_line();
                Some(Msg::Redraw)
            }
            // End: move to end of line
            Keyboard(KeyEvent {
                code: Key::End,
                modifiers: KeyModifiers::NONE,
            }) => {
                self.component.input_mut().move_to_end_of_line();
                Some(Msg::Redraw)
            }
            _ => None,
        }
    }
}
