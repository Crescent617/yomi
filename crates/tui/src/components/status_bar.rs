//! Status bar component for displaying streaming progress
//!
//! Shows spinner, token count, and elapsed time above the input box.

use tuirealm::{
    command::{Cmd, CmdResult},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::Rect,
        style::{Modifier, Style},
        text::{Line, Span},
        widgets::Paragraph,
    },
    Component, Frame, MockComponent, State,
};

use crate::msg::Msg;
use crate::theme::colors;

/// Status state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusState {
    #[default]
    Idle,
    Streaming,
    Completed,
    Cancelled,
}

/// Status bar component showing streaming progress
#[derive(Debug, Default)]
pub struct StatusBar {
    props: Props,
    state: StatusState,
    tick_frame: usize,
    content_tokens: usize,
    thinking_tokens: usize,
    start_time: Option<std::time::Instant>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_state(&mut self, state: StatusState) {
        self.state = state;
        match state {
            StatusState::Streaming => {
                self.tick_frame = 0;
                self.content_tokens = 0;
                self.thinking_tokens = 0;
                self.start_time = Some(std::time::Instant::now());
            }
            StatusState::Idle | StatusState::Completed | StatusState::Cancelled => {
                self.start_time = None;
            }
        }
    }

    pub const fn set_tokens(&mut self, content_tokens: usize, thinking_tokens: usize) {
        self.content_tokens = content_tokens;
        self.thinking_tokens = thinking_tokens;
    }

    pub fn tick(&mut self) {
        if self.state == StatusState::Streaming {
            self.tick_frame = self.tick_frame.wrapping_add(1);
        }
    }

    fn render(&self) -> Line<'static> {
        // Show when streaming or has tokens (keep showing after complete)
        if self.state == StatusState::Idle
            && self.content_tokens == 0
            && self.thinking_tokens == 0
        {
            return Line::from("");
        }

        let mut spans = Vec::new();

        // Indicator based on state
        let (indicator, indicator_style) = match self.state {
            StatusState::Streaming => {
                const FRAMES: &[&str] = &["∙∙", "●∙", "∙●"];
                let frame_idx = (self.tick_frame / 3) % FRAMES.len();
                (
                    FRAMES[frame_idx],
                    Style::default()
                        .fg(colors::accent_system())
                        .add_modifier(Modifier::BOLD),
                )
            }
            StatusState::Cancelled => (
                "✕",
                Style::default()
                    .fg(colors::accent_error())
                    .add_modifier(Modifier::BOLD),
            ),
            StatusState::Completed | StatusState::Idle => (
                "✓",
                Style::default()
                    .fg(colors::accent_success())
                    .add_modifier(Modifier::BOLD),
            ),
        };

        spans.push(Span::styled(format!("{indicator} "), indicator_style));

        // Token count with separate thinking count
        let token_style = Style::default().fg(colors::text_secondary());
        let total_tokens = self.content_tokens + self.thinking_tokens;

        if self.thinking_tokens > 0 {
            spans.push(Span::styled(
                format!("{} tokens (+{} think)", total_tokens, self.thinking_tokens),
                token_style,
            ));
        } else {
            spans.push(Span::styled(format!("{total_tokens} tokens"), token_style));
        }

        // Elapsed time (only when streaming)
        if self.state == StatusState::Streaming {
            if let Some(start) = self.start_time {
                let elapsed = start.elapsed().as_secs_f64();
                let time_str = if elapsed < 60.0 {
                    format!(" · {elapsed:.1}s")
                } else {
                    let mins = (elapsed / 60.0) as u64;
                    let secs = (elapsed % 60.0) as u64;
                    format!(" · {mins}m{secs:02}s")
                };
                spans.push(Span::styled(time_str, token_style));
            }
        }

        Line::from(spans)
    }
}

impl MockComponent for StatusBar {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let line = self.render();
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(s) if s == "start_streaming" => {
                self.set_state(StatusState::Streaming);
            }
            Attribute::Custom(s) if s == "stop_streaming" => {
                self.set_state(StatusState::Completed);
            }
            Attribute::Custom(s) if s == "cancel_streaming" => {
                self.set_state(StatusState::Cancelled);
            }
            Attribute::Custom(s) if s == "set_tokens" => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split(',').collect();
                    let content = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
                    let thinking = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    self.set_tokens(content, thinking);
                }
            }
            Attribute::Custom(s) if s == "tick" => {
                self.tick();
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
            tuirealm::Event::User(crate::msg::UserEvent::AppEvent(
                kernel::event::Event::Model(kernel::event::ModelEvent::Chunk { .. }),
            )) => {
                Some(Msg::Redraw)
            }
            _ => None,
        }
    }
}
