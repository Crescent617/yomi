//! Status bar component for TUI
//!
//! Shows current mode at the bottom (vim-style) with three sections:
//! [LEFT: mode] [CENTER: tips] [RIGHT: context usage / scroll progress]

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, Props, QueryResult},
    ratatui::{
        layout::{Constraint, Direction, Layout, Rect},
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
        Frame,
    },
    state::State,
};

use crate::{msg::Msg, theme::colors, utils::text::truncate_by_width};
use kernel::permissions::Level;
use unicode_width::UnicodeWidthStr;

/// Tip message for status bar (center section)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tip {
    pub content: String,
    /// Duration in milliseconds, 0 = no timeout
    pub duration_ms: u64,
}

impl Tip {
    pub fn new(content: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            content: content.into(),
            duration_ms,
        }
    }

    /// Convert to `AttrValue` using `PropPayload::Any` for downcast
    pub fn to_attr_value(&self) -> tuirealm::props::AttrValue {
        use tuirealm::props::{AttrValue, PropPayload};
        AttrValue::Payload(PropPayload::Any(Box::new(self.clone())))
    }
}

/// Application mode for status bar display
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    #[default]
    Normal,
    Browse,
}

/// Status bar showing current mode (vim-style at bottom)
/// Layout: [mode] [center: tip] [right: ctx win usage or scroll progress]
#[derive(Debug, Default)]
pub struct StatusBar {
    props: Props,
    mode: AppMode,
    /// Current tip (center section)
    tip: Option<Tip>,
    tip_timeout: Option<std::time::Instant>,
    /// Current token usage and context window size (tokens, `context_window`)
    ctx_usage: Option<(u32, u32)>,
    /// Permission level for displaying YOLO mode
    permission_level: Option<Level>,
    /// Scroll progress in browse mode (`current_line`, `total_lines`)
    scroll_progress: Option<(usize, usize)>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn set_mode(&mut self, mode: AppMode) {
        self.mode = mode;
    }

    /// Show a tip with timeout
    pub fn show_tip(&mut self, tip: Tip) {
        if tip.duration_ms == 0 {
            // No timeout - persistent tip
            self.tip = Some(tip);
            self.tip_timeout = None;
        } else {
            self.tip_timeout =
                Some(std::time::Instant::now() + std::time::Duration::from_millis(tip.duration_ms));
            self.tip = Some(tip);
        }
    }

    /// Check timeout and clear expired tip
    pub fn check_timeout(&mut self) {
        if let Some(timeout) = self.tip_timeout {
            if std::time::Instant::now() > timeout {
                self.tip = None;
                self.tip_timeout = None;
            }
        }
    }

    /// Tick handler for timeout checking
    pub fn tick(&mut self) {
        self.check_timeout();
    }

    /// Update context window usage (current tokens, max tokens)
    pub const fn set_ctx_usage(&mut self, tokens: u32, context_window: u32) {
        self.ctx_usage = Some((tokens, context_window));
    }

    /// Set permission level for YOLO mode display
    pub fn set_permission_level(&mut self, level: Level) {
        self.permission_level = Some(level);
    }

    /// Set scroll progress for browse mode (`current_line`, `total_lines`)
    pub const fn set_scroll_progress(&mut self, current: usize, total: usize) {
        self.scroll_progress = Some((current, total));
    }

    /// Clear scroll progress (when exiting browse mode)
    pub const fn clear_scroll_progress(&mut self) {
        self.scroll_progress = None;
    }

    fn render_mode_section(&self) -> Span<'static> {
        let (bg, text) = match self.mode {
            AppMode::Normal => {
                // Use warning color for YOLO mode
                if self.permission_level == Some(Level::Dangerous) {
                    (colors::accent_warning(), " YOLO ".to_string())
                } else {
                    (colors::accent_success(), String::new())
                }
            }
            AppMode::Browse => (colors::accent_system(), " BROWSE ".to_string()),
        };
        let fg = colors::selected_bg();

        Span::styled(
            text,
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        )
    }

    fn render_center_section(&self, width: usize) -> Span<'static> {
        let text = self.tip.as_ref().map_or("", |t| t.content.as_str());

        if text.is_empty() {
            return Span::styled("", Style::default());
        }

        // Center the message, truncate if too long (using display width for CJK)
        let text_width = text.width_cjk();
        let display = if text_width > width {
            truncate_by_width(text, width, "...")
        } else {
            let padding = (width.saturating_sub(text_width)) / 2;
            format!("{:>padding$}{}", "", text, padding = padding)
        };

        Span::styled(
            display,
            Style::default()
                .fg(colors::text_secondary())
                .add_modifier(Modifier::ITALIC),
        )
    }

    fn render_right_section(&self) -> Span<'static> {
        // In browse mode, show scroll progress
        if self.mode == AppMode::Browse {
            if let Some((current, total)) = self.scroll_progress {
                let text = format!("[{current}/{total}]");
                return Span::styled(
                    text,
                    Style::default()
                        .fg(colors::text_secondary())
                        .add_modifier(Modifier::BOLD),
                );
            }
            return Span::styled("[0/0]", Style::default().fg(colors::text_secondary()));
        }

        // Display context window usage: "Context: 0.5%"
        #[allow(clippy::cast_precision_loss)]
        if let Some((tokens, context_window)) = self.ctx_usage {
            let percentage = tokens as f32 / context_window as f32;
            let cw_k = context_window / 1000;
            let text = format!("{:>4.1}% ({}K)", percentage * 100.0, cw_k);

            // Color based on usage level
            let fg = if percentage >= 0.9 {
                colors::accent_error() // Red for high usage
            } else if percentage >= 0.7 {
                colors::accent_warning() // Yellow for medium-high usage
            } else {
                colors::text_secondary() // Default for normal usage
            };

            Span::styled(text, Style::default().fg(fg))
        } else {
            Span::styled("", Style::default())
        }
    }
}

impl Component for StatusBar {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Check for tip timeout
        self.check_timeout();

        // Split area into three sections: [mode] [center: tip] [right]
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(10), // Mode section (" NORMAL ")
                Constraint::Min(10),    // Center: tip
                Constraint::Length(14), // Right section: context usage or scroll progress
            ])
            .split(area);

        // Render mode section
        let mode_span = self.render_mode_section();
        let mode_line = Line::from(vec![mode_span]);
        frame.render_widget(Paragraph::new(mode_line), chunks[0]);

        // Render center tip section
        let center_width = chunks[1].width as usize;
        let center_span = self.render_center_section(center_width);
        let center_line = Line::from(vec![center_span]);
        frame.render_widget(Paragraph::new(center_line), chunks[1]);

        // Render right section
        let right_span = self.render_right_section();
        let right_line = Line::from(vec![right_span]);
        frame.render_widget(Paragraph::new(right_line), chunks[2]);
    }

    fn query(&self, attr: Attribute) -> Option<QueryResult<'_>> {
        self.props
            .get(attr)
            .map(|v| QueryResult::Borrowed(v.into()))
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom("set_mode") => {
                if let AttrValue::Number(mode_val) = value {
                    self.mode = match mode_val {
                        1 => AppMode::Browse,
                        _ => AppMode::Normal,
                    };
                }
            }
            Attribute::Custom("tick") => {
                self.check_timeout();
            }
            Attribute::Custom("show_tip") => {
                // Use downcast from PropPayload::Any
                use tuirealm::props::PropPayload;
                if let AttrValue::Payload(PropPayload::Any(payload)) = value {
                    let any = payload.as_any();
                    if let Some(tip) = any.downcast_ref::<Tip>() {
                        self.show_tip(tip.clone());
                    }
                }
            }
            Attribute::Custom("clear_tip") => {
                self.tip = None;
                self.tip_timeout = None;
            }
            Attribute::Custom("set_ctx_usage") => {
                // Parse "tokens\x00context_window" format
                if let AttrValue::String(value_str) = value {
                    let parts: Vec<&str> = value_str.split('\x00').collect();
                    if parts.len() == 2 {
                        if let (Ok(tokens), Ok(context_window)) =
                            (parts[0].parse::<u32>(), parts[1].parse::<u32>())
                        {
                            self.set_ctx_usage(tokens, context_window);
                        }
                    }
                }
            }
            Attribute::Custom("set_permission_level") => {
                // Parse permission level: 0 = Safe, 1 = Caution, 2 = Dangerous
                if let AttrValue::Number(level_val) = value {
                    self.permission_level = match level_val {
                        0 => Some(Level::Safe),
                        1 => Some(Level::Caution),
                        2 => Some(Level::Dangerous),
                        _ => None,
                    };
                }
            }
            Attribute::Custom("set_scroll_progress") => {
                // Parse "current\x00total" format
                if let AttrValue::String(value_str) = value {
                    let parts: Vec<&str> = value_str.split('\x00').collect();
                    if parts.len() == 2 {
                        if let (Ok(current), Ok(total)) =
                            (parts[0].parse::<usize>(), parts[1].parse::<usize>())
                        {
                            self.set_scroll_progress(current, total);
                        }
                    }
                }
            }
            Attribute::Custom("clear_scroll_progress") => {
                self.clear_scroll_progress();
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
        CmdResult::NoChange
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

impl Component for StatusBarComponent {
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

impl AppComponent<Msg, crate::msg::UserEvent> for StatusBarComponent {
    fn on(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        match *ev {
            Event::Tick => {
                self.component.tick();
                Some(Msg::Redraw)
            }
            _ => None,
        }
    }
}
