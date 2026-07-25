//! Info bar component for displaying streaming progress and notifications
//!
//! Shows a shimmering status word, token count, elapsed time on the left,
//! and notifications on the right.

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

use crate::{
    attr,
    components::chat_view::tool_verb,
    msg::Msg,
    theme::{chars, colors, shimmer_spans},
    utils::{
        text::{humanize_tool_name, truncate_by_width},
        TimedMessage,
    },
};
use kernel::utils::tokens;
use std::ops::{Deref, DerefMut};
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
            NotificationLevel::Unknown => colors::text_secondary(),
            NotificationLevel::Info => colors::accent_info(),
            NotificationLevel::Warn => colors::accent_warning(),
            NotificationLevel::Error => colors::accent_error(),
            NotificationLevel::Success => colors::accent_success(),
        }
    }

    /// Returns the prefix icon for this level, if any.
    fn icon(self) -> Option<&'static str> {
        match self {
            NotificationLevel::Unknown => None,
            NotificationLevel::Info => Some(" "),
            NotificationLevel::Warn => Some(" "),
            NotificationLevel::Error => Some(" "),
            NotificationLevel::Success => Some(" "),
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

    /// Create an unknown-level notification (no icon prefix, for custom content with emoji)
    pub fn unknown(content: impl Into<String>, duration_ms: u64) -> Self {
        Self::new(content, NotificationLevel::Unknown, duration_ms)
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
}

/// What kind of text is currently streaming; drives the status word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum StreamText {
    #[default]
    None,
    Thinking,
    Writing,
}

/// Ticks per shimmer sweep cycle (100ms tick → 2.4s sweep).
const SHIMMER_PERIOD_TICKS: usize = 24;

/// Info bar component showing streaming progress and notifications
/// Layout: [LEFT: status/tokens/time] [RIGHT: notifications]
#[derive(Debug, Default)]
pub struct InfoBar {
    state: InfoBarState,
    tick_frame: usize,
    token_count: f64,
    start_time: Option<std::time::Instant>,
    /// Current notification with level and timeout
    notification: TimedMessage<Notification>,
    /// Current tool call being streamed (`tool_name`)
    current_tool_call: Option<String>,
    /// Kind of text currently streaming (thinking vs writing)
    stream_text: StreamText,
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
                self.current_tool_call = None;
                self.stream_text = StreamText::None;
            }
            self.start_time = Some(std::time::Instant::now());
        } else if state.clears_timer() {
            self.start_time = None;
        }
    }

    pub fn append_content(&mut self, text: &str) {
        self.stream_text = StreamText::Writing;
        self.token_count += tokens::estimate_tokens_f64(text);
    }

    pub fn append_thinking(&mut self, text: &str) {
        self.stream_text = StreamText::Thinking;
        self.token_count += tokens::estimate_tokens_f64(text);
    }

    /// Tick handler for animation. Returns true while active: the shimmer
    /// wave advances every tick (10 Hz).
    pub fn tick(&mut self) -> bool {
        if self.state.is_active() {
            self.tick_frame = self.tick_frame.wrapping_add(1);
            true
        } else {
            false
        }
    }

    /// Show a notification with level and timeout
    pub fn show_notification(&mut self, notification: Notification) {
        let duration_ms = notification.duration_ms;
        if duration_ms == 0 {
            // No timeout - persistent notification
            self.notification.set(notification);
        } else {
            self.notification
                .set_with_timeout(notification, std::time::Duration::from_millis(duration_ms));
        }
    }

    /// Check timeout and clear expired notification
    pub fn check_timeout(&mut self) {
        self.notification.check_timeout();
    }

    /// Format elapsed time for display (e.g., " · 4s" or " · 2m03s").
    /// Whole-second granularity: the bar redraws every tick while active,
    /// and sub-second digits would churn at 10 Hz.
    fn format_elapsed(&self) -> Option<String> {
        let start = self.start_time?;
        let elapsed = start.elapsed().as_secs_f64();
        let time_str = if elapsed < 60.0 {
            format!(" · {}s", elapsed as u64)
        } else {
            let mins = (elapsed / 60.0) as u64;
            let secs = (elapsed % 60.0) as u64;
            format!(" · {mins}m{secs:02}s")
        };
        Some(time_str)
    }

    /// Shimmer wave position (0.0..1.0) for the current tick frame.
    #[allow(clippy::cast_precision_loss)]
    fn shimmer_phase(&self) -> f32 {
        (self.tick_frame % SHIMMER_PERIOD_TICKS) as f32 / SHIMMER_PERIOD_TICKS as f32
    }

    /// Status word shown with the shimmer sweep: the tool verb while a
    /// tool runs ("Running"), otherwise what the agent is currently doing.
    fn status_word(&self) -> Option<String> {
        match self.state {
            InfoBarState::Compacting => Some("Compacting".to_string()),
            InfoBarState::Streaming => Some(match self.current_tool_call.as_deref() {
                Some(name) => {
                    let verb = tool_verb(name);
                    if verb == "Calling" {
                        format!("Calling {}", humanize_tool_name(name))
                    } else {
                        verb.to_string()
                    }
                }
                None => match self.stream_text {
                    StreamText::Writing => "Writing".to_string(),
                    StreamText::None | StreamText::Thinking => "Thinking".to_string(),
                },
            }),
            _ => None,
        }
    }

    /// Render the left section (status word, tokens, elapsed)
    fn render_left_section(&self) -> Line<'static> {
        // Show when streaming, compacting, or has tokens, or has tool call
        if self.state == InfoBarState::Idle
            && self.token_count == 0.0
            && self.current_tool_call.is_none()
        {
            return Line::from("");
        }

        let mut spans = Vec::new();

        // Static indicator for terminal states; active states rely on the
        // shimmering word alone (no spinner glyph)
        match self.state {
            InfoBarState::Cancelled => spans.push(Span::styled(
                format!("{} ", chars::CANCELLED),
                Style::default()
                    .fg(colors::accent_error())
                    .add_modifier(Modifier::BOLD),
            )),
            InfoBarState::Completed | InfoBarState::Idle => spans.push(Span::styled(
                format!("{} ", chars::COMPLETED),
                Style::default()
                    .fg(colors::accent_success())
                    .add_modifier(Modifier::BOLD),
            )),
            _ => {}
        }

        // Status word with a shimmer wave sweeping through: `Running... `
        if let Some(word) = self.status_word() {
            let phase = self.shimmer_phase();
            let (base, peak) = match self.state {
                InfoBarState::Compacting => (colors::accent_warning(), colors::text_primary()),
                _ => (colors::text_secondary(), colors::text_primary()),
            };
            spans.extend(shimmer_spans(&format!("{word}..."), phase, base, peak));
            spans.push(Span::raw(" "));
        }

        let token_style = Style::default().fg(colors::text_secondary());
        let token_text = format!(
            "{} tokens",
            tokens::format_estimated_tokens_f64(self.token_count)
        );
        spans.push(Span::styled(token_text, token_style));

        // Elapsed time (when active)
        if let Some(time_str) = self.format_elapsed() {
            spans.push(Span::styled(time_str, token_style));
        }

        // Interrupt hint during streaming
        if self.state == InfoBarState::Streaming {
            spans.push(Span::styled(
                " · esc to interrupt".to_string(),
                Style::default()
                    .fg(colors::text_secondary())
                    .add_modifier(Modifier::ITALIC),
            ));
        }

        Line::from(spans)
    }

    /// Render the right section (notification)
    fn render_right_section(&self, width: usize) -> Line<'static> {
        let (text, level) = self
            .notification
            .content()
            .map_or(("", NotificationLevel::Unknown), |n| {
                (n.content.as_str(), n.level)
            });

        if text.is_empty() {
            return Line::from("");
        }

        // Add level icon prefix (Unknown level has no icon to allow custom emoji)
        let icon = level.icon().unwrap_or("");
        let full_text = format!("{icon}{text}");

        // Use display width (accounts for CJK characters being 2 columns)
        let text_width = full_text.width_cjk();

        // Truncate if too long, right-aligned
        let display = if text_width > width {
            truncate_by_width(&full_text, width, "...")
        } else {
            let padding = width.saturating_sub(text_width);
            format!("{:>padding$}{}", "", full_text, padding = padding)
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
                Constraint::Min(0), // Left: spinner/tokens/time
                Constraint::Min(0), // Right: notification (fixed width)
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
            Attribute::Custom(attr::START_STREAMING) => {
                self.set_state(InfoBarState::Streaming);
            }
            Attribute::Custom(attr::STOP_STREAMING) => {
                self.set_state(InfoBarState::Completed);
                self.current_tool_call = None;
                self.stream_text = StreamText::None;
            }
            Attribute::Custom(attr::CANCEL_STREAMING) => {
                self.set_state(InfoBarState::Cancelled);
                self.current_tool_call = None;
                self.stream_text = StreamText::None;
            }
            Attribute::Custom(attr::START_COMPACTING) => {
                self.set_state(InfoBarState::Compacting);
            }
            Attribute::Custom(attr::STOP_COMPACTING) => {
                self.set_state(InfoBarState::Idle);
            }
            Attribute::Custom(attr::APPEND_CONTENT) => {
                if let AttrValue::String(text) = value {
                    self.append_content(&text);
                }
            }
            Attribute::Custom(attr::APPEND_THINKING) => {
                if let AttrValue::String(text) = value {
                    self.append_thinking(&text);
                }
            }
            Attribute::Custom(attr::TICK) => {
                self.tick();
                self.check_timeout();
            }
            Attribute::Custom(attr::SHOW_NOTIFICATION) => {
                // Use downcast from PropPayload::Any
                if let AttrValue::Payload(PropPayload::Any(payload)) = value {
                    let any = payload.as_any();
                    if let Some(notification) = any.downcast_ref::<Notification>() {
                        self.show_notification(notification.clone());
                    }
                }
            }
            Attribute::Custom(attr::CLEAR_NOTIFICATION) => {
                self.notification.clear();
            }
            Attribute::Custom(attr::APPEND_TOOL_CALL_DELTA) => {
                // Format: "tool_name\x00arguments_delta"
                // arguments_delta contains only the newly added fragment
                if let AttrValue::String(data) = value {
                    let parts: Vec<&str> = data.split('\x00').collect();
                    if parts.len() >= 2 {
                        let tool_name = parts[0].to_string();
                        let arguments_delta = parts[1];

                        // Count tokens for the delta fragment
                        self.token_count += tokens::estimate_tokens_f64(arguments_delta);
                        self.current_tool_call = Some(tool_name);
                    }
                }
            }
            Attribute::Custom(attr::CLEAR_TOOL_CALL) => {
                self.current_tool_call = None;
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

impl Deref for InfoBarComponent {
    type Target = InfoBar;

    fn deref(&self) -> &Self::Target {
        &self.component
    }
}

impl DerefMut for InfoBarComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.component
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
                // Tick returns true if spinner frame changed
                let spinner_changed = self.component.tick();
                // Also redraw if notification expired
                let notification_expired = self.component.notification.check_timeout();
                if spinner_changed || notification_expired {
                    Some(Msg::Redraw)
                } else {
                    None
                }
            }
            // Note: Content updates come through attr() from app.rs, not here
            _ => None,
        }
    }
}
