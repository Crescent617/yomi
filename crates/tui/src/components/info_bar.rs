//! Info bar component for displaying streaming progress and notifications
//!
//! Shows spinner, token count, elapsed time on the left, and notifications on the right.

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, PropPayload, QueryResult},
    ratatui::{
        layout::{Constraint, Direction, Layout, Rect},
        style::{Color, Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
        Frame,
    },
    state::State,
};

use crate::{msg::Msg, theme::colors, utils::text::truncate_by_width};
use kernel::utils::tokens;
use unicode_width::UnicodeWidthStr;

/// Notification level for info bar messages
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotificationLevel {
    #[default]
    Unknown,
    Info,
    Warn,
    Error,
    Success,
}

impl NotificationLevel {
    fn color(self) -> Color {
        match self {
            NotificationLevel::Unknown | NotificationLevel::Info => colors::text_secondary(),
            NotificationLevel::Warn => colors::accent_warning(),
            NotificationLevel::Error => colors::accent_error(),
            NotificationLevel::Success => colors::accent_success(),
        }
    }
}

/// Notification message for info bar
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub content: String,
    pub level: NotificationLevel,
    /// Duration in milliseconds, 0 = no timeout
    pub duration_ms: u64,
}

impl Notification {
    pub fn new(content: impl Into<String>, level: NotificationLevel, duration_ms: u64) -> Self {
        Self {
            content: content.into(),
            level,
            duration_ms,
        }
    }

    pub fn info(content: impl Into<String>, duration_ms: u64) -> Self {
        Self::new(content, NotificationLevel::Info, duration_ms)
    }

    pub fn warn(content: impl Into<String>, duration_ms: u64) -> Self {
        Self::new(content, NotificationLevel::Warn, duration_ms)
    }

    pub fn error(content: impl Into<String>, duration_ms: u64) -> Self {
        Self::new(content, NotificationLevel::Error, duration_ms)
    }

    pub fn success(content: impl Into<String>, duration_ms: u64) -> Self {
        Self::new(content, NotificationLevel::Success, duration_ms)
    }

    /// Convert to `AttrValue` using `PropPayload::Any` for downcast
    pub fn to_attr_value(&self) -> AttrValue {
        AttrValue::Payload(PropPayload::Any(Box::new(self.clone())))
    }
}

/// Status state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InfoBarState {
    #[default]
    Idle,
    Streaming,
    Compacting,
    Completed,
    Cancelled,
}

impl InfoBarState {
    /// Returns true if this state is active (shows spinner and elapsed time)
    const fn is_active(self) -> bool {
        matches!(self, Self::Streaming | Self::Compacting)
    }

    /// Returns true if this state clears the timer
    const fn clears_timer(self) -> bool {
        matches!(self, Self::Idle | Self::Completed | Self::Cancelled)
    }

    /// Get the spinner frame and style for this state
    fn spinner(self, tick_frame: usize) -> (String, Style, &'static str) {
        match self {
            Self::Streaming => {
                const FRAMES: &[&str] = &["∙∙", "●∙", "∙●"];
                let frame = FRAMES[(tick_frame / 3) % FRAMES.len()];
                (
                    frame.to_string(),
                    Style::default()
                        .fg(colors::accent_system())
                        .add_modifier(Modifier::BOLD),
                    "",
                )
            }
            Self::Compacting => {
                const FRAMES: &[&str] = &["∙∙", "●∙", "∙●"];
                let frame = FRAMES[(tick_frame / 3) % FRAMES.len()];
                (
                    frame.to_string(),
                    Style::default()
                        .fg(colors::accent_warning())
                        .add_modifier(Modifier::BOLD),
                    "Compacting...",
                )
            }
            Self::Cancelled => (
                "✕".to_string(),
                Style::default()
                    .fg(colors::accent_error())
                    .add_modifier(Modifier::BOLD),
                "",
            ),
            Self::Completed | Self::Idle => (
                "✓".to_string(),
                Style::default()
                    .fg(colors::accent_success())
                    .add_modifier(Modifier::BOLD),
                "",
            ),
        }
    }
}

/// Info bar component showing streaming progress and notifications
/// Layout: [LEFT: spinner/tokens/time] [RIGHT: notifications]
#[derive(Debug, Default)]
pub struct InfoBar {
    state: InfoBarState,
    tick_frame: usize,
    token_count: f64,
    start_time: Option<std::time::Instant>,
    /// Current notification with level
    notification: Option<Notification>,
    notification_timeout: Option<std::time::Instant>,
}

impl InfoBar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_state(&mut self, state: InfoBarState) {
        self.state = state;
        if state.is_active() {
            self.tick_frame = 0;
            if state == InfoBarState::Streaming {
                self.token_count = 0.0;
            }
            self.start_time = Some(std::time::Instant::now());
        } else if state.clears_timer() {
            self.start_time = None;
        }
    }

    pub fn append_content(&mut self, text: &str) {
        self.token_count += tokens::estimate_tokens_f64(text);
    }

    pub fn append_thinking(&mut self, text: &str) {
        self.token_count += tokens::estimate_tokens_f64(text);
    }

    pub const fn tick(&mut self) {
        if self.state.is_active() {
            self.tick_frame = self.tick_frame.wrapping_add(1);
        }
    }

    /// Show a notification with level and timeout
    pub fn show_notification(&mut self, notification: Notification) {
        if notification.duration_ms == 0 {
            // No timeout - persistent notification
            self.notification = Some(notification);
            self.notification_timeout = None;
        } else {
            self.notification_timeout = Some(
                std::time::Instant::now()
                    + std::time::Duration::from_millis(notification.duration_ms),
            );
            self.notification = Some(notification);
        }
    }

    /// Check timeout and clear expired notification
    pub fn check_timeout(&mut self) {
        if let Some(timeout) = self.notification_timeout {
            if std::time::Instant::now() > timeout {
                self.notification = None;
                self.notification_timeout = None;
            }
        }
    }

    /// Format elapsed time for display (e.g., " · 1.5s" or " · 2m30s")
    fn format_elapsed(&self) -> Option<String> {
        let start = self.start_time?;
        let elapsed = start.elapsed().as_secs_f64();
        let time_str = if elapsed < 60.0 {
            format!(" · {elapsed:.1}s")
        } else {
            let mins = (elapsed / 60.0) as u64;
            let secs = (elapsed % 60.0) as u64;
            format!(" · {mins}m{secs:02}s")
        };
        Some(time_str)
    }

    /// Render the left section (spinner, tokens, elapsed time)
    fn render_left_section(&self) -> Line<'static> {
        // Show when streaming, compacting, or has tokens
        if self.state == InfoBarState::Idle && self.token_count == 0.0 {
            return Line::from("");
        }

        let mut spans = Vec::new();

        // Get spinner/indicator and style from state
        let (indicator, indicator_style, status_text) = self.state.spinner(self.tick_frame);
        spans.push(Span::styled(format!("{indicator} "), indicator_style));

        // Status text (e.g., "Compacting...")
        if !status_text.is_empty() {
            spans.push(Span::styled(format!("{status_text} "), indicator_style));
        }

        let token_style = Style::default().fg(colors::text_secondary());
        let token_text = format!(
            "{} tokens",
            tokens::format_token_count_f64(self.token_count)
        );
        spans.push(Span::styled(token_text, token_style));

        // Elapsed time (when active)
        if let Some(time_str) = self.format_elapsed() {
            spans.push(Span::styled(time_str, token_style));
        }

        Line::from(spans)
    }

    /// Render the right section (notification)
    fn render_right_section(&self, width: usize) -> Line<'static> {
        let (text, level) = self
            .notification
            .as_ref()
            .map_or(("", NotificationLevel::Unknown), |n| {
                (n.content.as_str(), n.level)
            });

        if text.is_empty() {
            return Line::from("");
        }

        // Use display width (accounts for CJK characters being 2 columns)
        let text_width = text.width_cjk();

        // Truncate if too long, right-aligned
        let display = if text_width > width {
            truncate_by_width(text, width, "...")
        } else {
            let padding = width.saturating_sub(text_width);
            format!("{:>padding$}{}", "", text, padding = padding)
        };

        let span = Span::styled(
            display,
            Style::default()
                .fg(level.color())
                .add_modifier(Modifier::ITALIC),
        );

        Line::from(vec![span])
    }
}

impl Component for InfoBar {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Check for notification timeout
        self.check_timeout();

        // Split area into two sections: [left info] [right notification]
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(20),    // Left: spinner/tokens/time
                Constraint::Length(40), // Right: notification (fixed width)
            ])
            .split(area);

        // Render left section
        let left_line = self.render_left_section();
        let left_paragraph = Paragraph::new(left_line);
        frame.render_widget(left_paragraph, chunks[0]);

        // Render right section (notification)
        let right_width = chunks[1].width as usize;
        let right_line = self.render_right_section(right_width);
        let right_paragraph = Paragraph::new(right_line);
        frame.render_widget(right_paragraph, chunks[1]);
    }

    fn query(&self, _attr: Attribute) -> Option<QueryResult<'_>> {
        None
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom("start_streaming") => {
                self.set_state(InfoBarState::Streaming);
            }
            Attribute::Custom("stop_streaming") => {
                self.set_state(InfoBarState::Completed);
            }
            Attribute::Custom("cancel_streaming") => {
                self.set_state(InfoBarState::Cancelled);
            }
            Attribute::Custom("start_compacting") => {
                self.set_state(InfoBarState::Compacting);
            }
            Attribute::Custom("stop_compacting") => {
                self.set_state(InfoBarState::Idle);
            }
            Attribute::Custom("append_content") => {
                if let AttrValue::String(text) = value {
                    self.append_content(&text);
                }
            }
            Attribute::Custom("append_thinking") => {
                if let AttrValue::String(text) = value {
                    self.append_thinking(&text);
                }
            }
            Attribute::Custom("tick") => {
                self.tick();
                self.check_timeout();
            }
            Attribute::Custom("show_notification") => {
                // Use downcast from PropPayload::Any
                if let AttrValue::Payload(PropPayload::Any(payload)) = value {
                    let any = payload.as_any();
                    if let Some(notification) = any.downcast_ref::<Notification>() {
                        self.show_notification(notification.clone());
                    }
                }
            }
            Attribute::Custom("clear_notification") => {
                self.notification = None;
                self.notification_timeout = None;
            }
            _ => {}
        }
    }

    fn state(&self) -> State {
        State::None
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::NoChange
    }
}

/// Component wrapper for `InfoBar`
pub struct InfoBarComponent {
    component: InfoBar,
}

impl Default for InfoBarComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl InfoBarComponent {
    pub fn new() -> Self {
        Self {
            component: InfoBar::new(),
        }
    }
}

impl Component for InfoBarComponent {
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

impl AppComponent<Msg, crate::msg::UserEvent> for InfoBarComponent {
    fn on(&mut self, ev: &Event<crate::msg::UserEvent>) -> Option<Msg> {
        match *ev {
            Event::Tick => {
                self.component.tick();
                Some(Msg::Redraw)
            }
            // Note: Content updates come through attr() from app.rs, not here
            _ => None,
        }
    }
}
