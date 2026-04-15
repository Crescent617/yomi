//! Generic select dialog component for TUI
//!
//! Provides a modal-like dialog for selecting from a list of options.
//! Used for permission confirmation and other user choices.

use tuirealm::{
    command::{Cmd, CmdResult, Direction as CmdDirection},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::{Alignment, Constraint, Direction, Layout, Margin, Rect},
        style::{Modifier, Style},
        widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    },
    Component, Frame, MockComponent, State,
};

use crate::{msg::Msg, theme::colors};

/// Dialog result type
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogResult {
    Selected(usize),
    Cancelled,
}

/// A generic select dialog component
#[derive(Debug)]
pub struct SelectDialog {
    props: Props,
    /// Dialog title
    title: String,
    /// Options to select from
    options: Vec<String>,
    /// Currently selected index
    selected: usize,
    /// Whether the dialog is active/visible
    active: bool,
    /// Optional message/body text (shown above options)
    message: Option<String>,
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
        }
    }

    /// Show the dialog with given options
    pub fn show(&mut self, options: Vec<String>, message: Option<String>) {
        self.options = options;
        self.selected = 0;
        self.message = message;
        self.active = true;
    }

    /// Hide the dialog
    pub const fn hide(&mut self) {
        self.active = false;
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

    fn render_dialog(&self, frame: &mut Frame, area: Rect) {
        // Calculate dialog size (centered, 60% width, auto height)
        let dialog_width = (f32::from(area.width) * 0.6).clamp(40.0, 80.0) as u16;
        let message_height = self
            .message
            .as_ref()
            .map_or(0, |m| m.lines().count() as u16);
        let dialog_height = (5 + message_height + self.options.len() as u16).min(area.height - 4);

        let dialog_area = Rect {
            x: area.x + (area.width - dialog_width) / 2,
            y: area.y + (area.height - dialog_height) / 2,
            width: dialog_width,
            height: dialog_height,
        };

        // Clear the background behind dialog
        frame.render_widget(Clear, dialog_area);

        // Create block with title
        let block = Block::default()
            .title(self.title.as_str())
            .borders(Borders::ALL)
            .border_style(colors::accent_system())
            .title_style(
                Style::default()
                    .fg(colors::accent_system())
                    .add_modifier(Modifier::BOLD),
            );

        // Split dialog area into message and list sections
        let inner = dialog_area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        let constraints = if message_height > 0 {
            vec![
                Constraint::Length(message_height + 1), // Message + padding
                Constraint::Min(1),                     // Options list
            ]
        } else {
            vec![Constraint::Min(1)]
        };

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(inner);

        // Render message if present
        if let Some(message) = &self.message {
            let message_para = Paragraph::new(message.as_str())
                .alignment(Alignment::Left)
                .style(Style::default().fg(colors::text_secondary()));
            frame.render_widget(message_para, chunks[0]);
        }

        // Render options as a list
        let list_area = if message_height > 0 {
            chunks[1]
        } else {
            chunks[0]
        };

        let items: Vec<ListItem> = self
            .options
            .iter()
            .enumerate()
            .map(|(idx, option)| {
                let prefix = if idx == self.selected { "▸ " } else { "  " };
                let content = format!("{prefix}{option}");

                let style = if idx == self.selected {
                    Style::default()
                        .fg(colors::accent_system())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(colors::text_primary())
                };

                ListItem::new(content).style(style)
            })
            .collect();

        let list = List::new(items)
            .block(Block::default())
            .highlight_style(Style::default());

        frame.render_widget(list, list_area);

        // Render the border block last (on top)
        frame.render_widget(block, dialog_area);
    }
}

impl MockComponent for SelectDialog {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        if self.active {
            self.render_dialog(frame, area);
        }
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom("show") => {
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
            Attribute::Custom("hide") => {
                self.hide();
            }
            _ => {
                self.props.set(attr, value);
            }
        }
    }

    fn state(&self) -> State {
        use tuirealm::StateValue;
        if let Some(idx) = self.current_selection() {
            // Use String to represent the selected index
            State::One(StateValue::String(idx.to_string()))
        } else {
            State::None
        }
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        use tuirealm::StateValue;
        if !self.active {
            return CmdResult::None;
        }

        match cmd {
            Cmd::Move(CmdDirection::Up) => {
                self.select_up();
                CmdResult::Changed(State::One(StateValue::String(self.selected.to_string())))
            }
            Cmd::Move(CmdDirection::Down) => {
                self.select_down();
                CmdResult::Changed(State::One(StateValue::String(self.selected.to_string())))
            }
            Cmd::Submit => {
                if let Some(idx) = self.current_selection() {
                    self.hide();
                    CmdResult::Submit(State::One(StateValue::String(idx.to_string())))
                } else {
                    CmdResult::None
                }
            }
            Cmd::Cancel => {
                self.hide();
                CmdResult::Submit(State::None)
            }
            _ => CmdResult::None,
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

    pub const fn hide(&mut self) {
        self.component.hide();
    }

    pub const fn is_active(&self) -> bool {
        self.component.is_active()
    }
}

impl MockComponent for SelectDialogComponent {
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

impl Component<Msg, crate::msg::UserEvent> for SelectDialogComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        use tuirealm::event::{Key, KeyEvent, KeyModifiers};
        use tuirealm::Event::Keyboard;

        tracing::trace!(
            "Dialog received event: {:?}, active={}",
            ev,
            self.component.is_active()
        );

        if !self.component.is_active() {
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
                code: Key::Enter,
                modifiers: KeyModifiers::NONE,
            }) => {
                tracing::info!(
                    "Dialog: Enter pressed, selection={:?}",
                    self.component.current_selection()
                );
                if let Some(idx) = self.component.current_selection() {
                    self.component.hide();
                    Some(Msg::DialogSelected(idx))
                } else {
                    None
                }
            }
            Keyboard(KeyEvent {
                code: Key::Esc,
                modifiers: KeyModifiers::NONE,
            }) => {
                tracing::info!("Dialog: Esc pressed");
                self.component.hide();
                Some(Msg::DialogCancelled)
            }
            _ => None,
        }
    }
}
