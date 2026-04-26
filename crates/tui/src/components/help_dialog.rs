//! Help dialog component for TUI
//!
//! A reusable component for displaying help information with keyboard shortcuts.
//! Supports scrolling for long content and can be closed with q/esc/ctrl-c.

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{
        layout::{Margin, Rect},
        style::{Modifier, Style},
        widgets::{
            Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
            ScrollbarState,
        },
        Frame,
    },
    state::{State, StateValue},
};

use crate::{msg::Msg, theme::colors};

/// A section of help content with a title and list of key bindings
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HelpSection {
    pub title: String,
    pub bindings: Vec<(String, String)>, // (key, description)
}

impl HelpSection {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            bindings: Vec::new(),
        }
    }

    #[must_use]
    pub fn add_binding(mut self, key: impl Into<String>, desc: impl Into<String>) -> Self {
        self.bindings.push((key.into(), desc.into()));
        self
    }
}

/// A generic help dialog component for displaying keyboard shortcuts
#[derive(Debug)]
pub struct HelpDialog {
    props: Props,
    /// Dialog title
    title: String,
    /// Help sections to display
    sections: Vec<HelpSection>,
    /// Whether the dialog is active/visible
    active: bool,
    /// Current scroll offset (line number at top)
    scroll_offset: usize,
}

impl HelpDialog {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            props: Props::default(),
            title: title.into(),
            sections: Vec::new(),
            active: false,
            scroll_offset: 0,
        }
    }

    /// Show the dialog with given help sections
    pub fn show(&mut self, sections: Vec<HelpSection>) {
        self.sections = sections;
        self.scroll_offset = 0;
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

    /// Scroll up by given number of lines
    pub fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    /// Scroll down by given number of lines
    pub fn scroll_down(&mut self, amount: usize) {
        let max_scroll = self.total_lines().saturating_sub(1);
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
    }

    /// Calculate total number of content lines
    fn total_lines(&self) -> usize {
        self.sections
            .iter()
            .map(|s| 1 + s.bindings.len() + 1) // title + bindings + empty line
            .sum()
    }

    /// Build the help text content
    fn build_content(&self) -> String {
        use std::fmt::Write;
        let mut content = String::new();
        for section in &self.sections {
            writeln!(content, "{}", section.title).unwrap();
            for (key, desc) in &section.bindings {
                writeln!(content, "  {key:<20} {desc}").unwrap();
            }
            content.push('\n');
        }
        content
    }

    fn render_dialog(&self, frame: &mut Frame, area: Rect) {
        // Calculate dialog size (centered, 70% width, 80% height)
        let dialog_width = (f32::from(area.width) * 0.7).clamp(50.0, 100.0) as u16;
        let dialog_width = dialog_width.min(area.width.saturating_sub(4));
        let dialog_height = (f32::from(area.height) * 0.8).clamp(15.0, 40.0) as u16;
        let dialog_height = dialog_height.min(area.height.saturating_sub(4));

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
            .border_type(BorderType::Rounded)
            .border_style(colors::accent_system())
            .title_style(
                Style::default()
                    .fg(colors::accent_system())
                    .add_modifier(Modifier::BOLD),
            );

        // Create layout for content
        let inner = dialog_area.inner(Margin {
            horizontal: 1,
            vertical: 1,
        });

        // Build and render content
        let content = self.build_content();
        let scroll_y = self.scroll_offset.min(u16::MAX as usize) as u16;
        let content_para = Paragraph::new(content)
            .style(Style::default().fg(colors::text_primary()))
            .scroll((scroll_y, 0));
        frame.render_widget(content_para, inner);

        // Render the border block last (on top)
        frame.render_widget(block, dialog_area);

        // Render scrollbar if content is scrollable
        let visible_lines = inner.height as usize;
        let total = self.total_lines();
        if total > visible_lines {
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .end_symbol(Some("▼"));
            let mut scrollbar_state = ScrollbarState::new(total).position(self.scroll_offset);
            frame.render_stateful_widget(
                scrollbar,
                inner.inner(Margin {
                    horizontal: 0,
                    vertical: 0,
                }),
                &mut scrollbar_state,
            );
        }
    }
}

impl Component for HelpDialog {
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
            Attribute::Custom("show") => {
                if let AttrValue::Payload(payload) = value {
                    if let Some(any_ref) = payload.as_any() {
                        if let Some(sections) = any_ref.downcast_ref::<Vec<HelpSection>>() {
                            self.show(sections.clone());
                        }
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
        if self.active {
            State::Single(StateValue::String("active".to_string()))
        } else {
            State::None
        }
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        if !self.active {
            return CmdResult::NoChange;
        }

        match cmd {
            Cmd::Move(tuirealm::command::Direction::Up) => {
                self.scroll_up(1);
                CmdResult::Changed(State::Single(StateValue::Usize(self.scroll_offset)))
            }
            Cmd::Move(tuirealm::command::Direction::Down) => {
                self.scroll_down(1);
                CmdResult::Changed(State::Single(StateValue::Usize(self.scroll_offset)))
            }
            Cmd::Cancel => {
                self.hide();
                CmdResult::Submit(State::None)
            }
            _ => CmdResult::NoChange,
        }
    }
}

/// Create default help sections for the TUI application
pub fn default_help_sections() -> Vec<HelpSection> {
    vec![
        HelpSection::new("Normal Mode")
            .add_binding("Enter", "Send message")
            .add_binding("Shift+Enter", "Insert newline")
            .add_binding("Ctrl+O", "Toggle browse mode")
            .add_binding("Ctrl+R", "Search history")
            .add_binding("Ctrl+C", "Clear input / Quit (double press)")
            .add_binding("Ctrl+Z", "Suspend to background")
            .add_binding("@", "Mention file")
            .add_binding("/", "Show slash commands"),
        HelpSection::new("Browse Mode")
            .add_binding("j / Down", "Scroll down")
            .add_binding("k / Up", "Scroll up")
            .add_binding("d / PageDown", "Page down")
            .add_binding("u / PageUp", "Page up")
            .add_binding("g", "Go to top")
            .add_binding("G", "Go to bottom")
            .add_binding("e", "Toggle expand all")
            .add_binding("q / Esc", "Exit browse mode"),
        HelpSection::new("Input Navigation")
            .add_binding("Ctrl+A / Home", "Start of line")
            .add_binding("Ctrl+E / End", "End of line")
            .add_binding("Alt+B", "Previous word")
            .add_binding("Alt+F", "Next word")
            .add_binding("Ctrl+U", "Clear to start")
            .add_binding("Ctrl+W", "Delete word back")
            .add_binding("Ctrl+P / Up", "Previous history")
            .add_binding("Ctrl+N / Down", "Next history"),
        HelpSection::new("Slash Commands")
            .add_binding("/new", "Create new session")
            .add_binding("/clear", "Clear chat history")
            .add_binding("/yolo", "Toggle YOLO mode")
            .add_binding("/browse", "Toggle browse mode")
            .add_binding("/compact", "Force message compaction")
            .add_binding("/help", "Show this help dialog"),
    ]
}

impl AppComponent<Msg, crate::msg::UserEvent> for HelpDialog {
    fn on(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        use tuirealm::event::{Key, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

        if !self.active {
            return None;
        }

        match *ev {
            // Mouse scroll up
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollUp,
                ..
            }) => {
                self.scroll_up(3);
                Some(Msg::Redraw)
            }
            // Mouse scroll down
            Event::Mouse(MouseEvent {
                kind: MouseEventKind::ScrollDown,
                ..
            }) => {
                self.scroll_down(3);
                Some(Msg::Redraw)
            }
            // Scroll up
            Event::Keyboard(KeyEvent {
                code: Key::Up | Key::Char('k'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.scroll_up(1);
                Some(Msg::Redraw)
            }
            // Scroll down
            Event::Keyboard(KeyEvent {
                code: Key::Down | Key::Char('j'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.scroll_down(1);
                Some(Msg::Redraw)
            }
            // Page up
            Event::Keyboard(KeyEvent {
                code: Key::PageUp | Key::Char('u'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.scroll_up(10);
                Some(Msg::Redraw)
            }
            // Page down
            Event::Keyboard(KeyEvent {
                code: Key::PageDown | Key::Char('d'),
                modifiers: KeyModifiers::NONE,
            }) => {
                self.scroll_down(10);
                Some(Msg::Redraw)
            }
            // Close dialog: q, Esc, or Ctrl+C
            Event::Keyboard(
                KeyEvent {
                    code: Key::Char('q') | Key::Esc,
                    modifiers: KeyModifiers::NONE,
                }
                | KeyEvent {
                    code: Key::Char('c'),
                    modifiers: KeyModifiers::CONTROL,
                },
            ) => {
                self.hide();
                Some(Msg::CloseHelpDialog)
            }
            _ => None,
        }
    }
}
