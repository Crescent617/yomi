//! Status bar component for TUI
//!
//! Shows current mode at the bottom (vim-style) with three sections:
//! [LEFT: mode] [CENTER: temporary messages] [RIGHT: reserved]

use tuirealm::{
    command::{Cmd, CmdResult},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::{Constraint, Direction, Layout, Rect},
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
    },
    Component, Frame, MockComponent, State,
};

use crate::{msg::Msg, theme::colors};

/// Application mode for status bar display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    #[default]
    Normal,
    Browse,
}

impl AppMode {
    const fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "",
            Self::Browse => " BROWSE ",
        }
    }
}

/// Status bar showing current mode (vim-style at bottom)
/// Layout: [mode] [center message] [right reserved]
#[derive(Debug, Default)]
pub struct StatusBar {
    props: Props,
    mode: AppMode,
    center_message: Option<String>,
    message_timeout: Option<std::time::Instant>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
    }

    /// Show a temporary message in the center section
    pub fn show_message(&mut self, message: String, timeout_secs: u64) {
        self.center_message = Some(message);
        self.message_timeout =
            Some(std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs));
    }

    /// Clear message if timeout expired
    pub fn check_timeout(&mut self) {
        if let Some(timeout) = self.message_timeout {
            if std::time::Instant::now() > timeout {
                self.center_message = None;
                self.message_timeout = None;
            }
        }
    }

    /// Tick handler for timeout checking
    pub fn tick(&mut self) {
        self.check_timeout();
    }

    fn render_mode_section(&self) -> Span<'static> {
        let bg = match self.mode {
            AppMode::Normal => colors::accent_success(),
            AppMode::Browse => colors::accent_system(),
        };
        let fg = colors::code_bg();

        Span::styled(
            self.mode.as_str(),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        )
    }

    fn render_center_section(&self, width: usize) -> Span<'static> {
        let message = self.center_message.as_deref().unwrap_or("");
        // Center the message, truncate if too long
        let display = if message.len() > width {
            format!("{}...", &message[..width.saturating_sub(3)])
        } else {
            let padding = (width.saturating_sub(message.len())) / 2;
            format!("{:>padding$}{}", "", message, padding = padding)
        };

        Span::styled(
            display,
            Style::default()
                .fg(colors::text_secondary())
                .add_modifier(Modifier::ITALIC),
        )
    }

    fn render_right_section(&self) -> Span<'static> {
        // Reserved for future use (e.g., file info, cursor position)
        Span::styled("", Style::default())
    }
}

impl MockComponent for StatusBar {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Check for message timeout
        self.check_timeout();

        // Split area into three sections: [mode] [center] [right]
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(10), // Mode section (" NORMAL ")
                Constraint::Min(10),    // Center message section
                Constraint::Length(10), // Right reserved section
            ])
            .split(area);

        // Render mode section
        let mode_span = self.render_mode_section();
        let mode_line = Line::from(vec![mode_span]);
        frame.render_widget(Paragraph::new(mode_line), chunks[0]);

        // Render center message section
        let center_width = chunks[1].width as usize;
        let center_span = self.render_center_section(center_width);
        let center_line = Line::from(vec![center_span]);
        frame.render_widget(Paragraph::new(center_line), chunks[1]);

        // Render right section
        let right_span = self.render_right_section();
        let right_line = Line::from(vec![right_span]);
        frame.render_widget(Paragraph::new(right_line), chunks[2]);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(s) if s == "set_mode" => {
                if let AttrValue::Number(mode_val) = value {
                    self.mode = match mode_val {
                        0 => AppMode::Normal,
                        1 => AppMode::Browse,
                        _ => AppMode::Normal,
                    };
                }
            }
            Attribute::Custom(s) if s == "show_message" => {
                // Parse duration (ms) and message from "duration_ms|message" format
                if let AttrValue::String(value_str) = value {
                    let parts: Vec<&str> = value_str.splitn(2, '|').collect();
                    if parts.len() == 2 {
                        let duration_ms = parts[0].parse::<u64>().unwrap_or(0);
                        self.center_message = Some(parts[1].to_string());
                        // If duration is 0, don't set timeout (message persists until cleared)
                        self.message_timeout = if duration_ms == 0 {
                            None
                        } else {
                            Some(
                                std::time::Instant::now()
                                    + std::time::Duration::from_millis(duration_ms),
                            )
                        };
                    }
                }
            }
            Attribute::Custom(s) if s == "tick" => {
                self.check_timeout();
            }
            Attribute::Custom(s) if s == "clear_message" => {
                self.center_message = None;
                self.message_timeout = None;
            }
            _ => {
                self.props.set(attr, value);
            }
        }
    }

    fn state(&self) -> State {
        State::None
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::None
    }
}

/// Component wrapper for `StatusBar`
pub struct StatusBarComponent {
    component: StatusBar,
}

impl Default for StatusBarComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl StatusBarComponent {
    pub fn new() -> Self {
        Self {
            component: StatusBar::new(),
        }
    }
}

impl MockComponent for StatusBarComponent {
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

impl Component<Msg, crate::msg::UserEvent> for StatusBarComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        match ev {
            tuirealm::Event::Tick => {
                self.component.tick();
                Some(Msg::Redraw)
            }
            _ => None,
        }
    }
}
