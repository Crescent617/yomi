//! Info bar component for displaying streaming progress
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

use crate::{msg::Msg, theme::colors, utils::token_utils};

/// Status state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InfoBarState {
    #[default]
    Idle,
    Streaming,
    Completed,
    Cancelled,
}

/// Info bar component showing streaming progress
#[derive(Debug, Default)]
pub struct InfoBar {
    props: Props,
    state: InfoBarState,
    tick_frame: usize,
    content: String,
    thinking: String,
    start_time: Option<std::time::Instant>,
}

impl InfoBar {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_state(&mut self, state: InfoBarState) {
        self.state = state;
        match state {
            InfoBarState::Streaming => {
                self.tick_frame = 0;
                self.content.clear();
                self.thinking.clear();
                self.start_time = Some(std::time::Instant::now());
            }
            InfoBarState::Idle | InfoBarState::Completed | InfoBarState::Cancelled => {
                self.start_time = None;
            }
        }
    }

    pub fn append_content(&mut self, text: &str) {
        self.content.push_str(text);
    }

    pub fn append_thinking(&mut self, text: &str) {
        self.thinking.push_str(text);
    }

    pub fn tick(&mut self) {
        if self.state == InfoBarState::Streaming {
            self.tick_frame = self.tick_frame.wrapping_add(1);
        }
    }

    fn render(&self) -> Line<'static> {
        // Show when streaming or has content
        if self.state == InfoBarState::Idle && self.content.is_empty() && self.thinking.is_empty() {
            return Line::from("");
        }

        let mut spans = Vec::new();

        // Indicator based on state
        let (indicator, indicator_style) = match self.state {
            InfoBarState::Streaming => {
                const FRAMES: &[&str] = &["∙∙", "●∙", "∙●"];
                let frame_idx = (self.tick_frame / 3) % FRAMES.len();
                (
                    FRAMES[frame_idx],
                    Style::default()
                        .fg(colors::accent_system())
                        .add_modifier(Modifier::BOLD),
                )
            }
            InfoBarState::Cancelled => (
                "✕",
                Style::default()
                    .fg(colors::accent_error())
                    .add_modifier(Modifier::BOLD),
            ),
            InfoBarState::Completed | InfoBarState::Idle => (
                "✓",
                Style::default()
                    .fg(colors::accent_success())
                    .add_modifier(Modifier::BOLD),
            ),
        };

        spans.push(Span::styled(format!("{indicator} "), indicator_style));

        // Token count using tiktoken
        let content_tokens = token_utils::count_tokens(&self.content);
        let thinking_tokens = token_utils::count_tokens(&self.thinking);
        let total_tokens = content_tokens + thinking_tokens;

        let token_style = Style::default().fg(colors::text_secondary());
        // Add ~ prefix to indicate these are estimated token counts
        let token_text = format!("~{total_tokens} tokens");
        spans.push(Span::styled(token_text, token_style));

        // Elapsed time (only when streaming)
        if self.state == InfoBarState::Streaming {
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

impl MockComponent for InfoBar {
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
            Attribute::Custom("start_streaming") => {
                self.set_state(InfoBarState::Streaming);
            }
            Attribute::Custom("stop_streaming") => {
                self.set_state(InfoBarState::Completed);
            }
            Attribute::Custom("cancel_streaming") => {
                self.set_state(InfoBarState::Cancelled);
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

impl MockComponent for InfoBarComponent {
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

impl Component<Msg, crate::msg::UserEvent> for InfoBarComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        match ev {
            tuirealm::Event::Tick => {
                self.component.tick();
                Some(Msg::Redraw)
            }
            // Note: Content updates come through attr() from app.rs, not here
            _ => None,
        }
    }
}
