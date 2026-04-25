//! Info bar component for displaying streaming progress
//!
//! Shows spinner, token count, and elapsed time above the input box.

use tuirealm::{
    command::{Cmd, CmdResult},
    component::{AppComponent, Component},
    event::Event,
    props::{AttrValue, Attribute, QueryResult},
    ratatui::{
        layout::Rect,
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
        Frame,
    },
    state::State,
};

use crate::{msg::Msg, theme::colors};
use kernel::utils::tokens;

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

/// Info bar component showing streaming progress
#[derive(Debug, Default)]
pub struct InfoBar {
    state: InfoBarState,
    tick_frame: usize,
    token_count: f64,
    start_time: Option<std::time::Instant>,
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

    fn render(&self) -> Line<'static> {
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
}

impl Component for InfoBar {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let line = self.render();
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
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
