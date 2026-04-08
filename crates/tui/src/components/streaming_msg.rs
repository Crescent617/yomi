//! Streaming message component for tuirealm
//!
//! Displays streaming content with thinking and main content areas.

use tuirealm::{
    command::{Cmd, CmdResult},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::Rect,
        style::{Color, Modifier, Style},
        text::{Line, Span, Text},
        widgets::Paragraph,
    },
    Component, Frame, MockComponent, State, StateValue,
};

use crate::{
    markdown_stream::StreamingMarkdownRenderer,
    msg::Msg,
};

/// Mock component that displays streaming AI response
#[derive(Debug, Default)]
pub struct StreamingMessageMock {
    props: Props,
    thinking: String,
    content: String,
    is_active: bool,
    tick_frame: usize,
    md_renderer: StreamingMarkdownRenderer,
}

impl StreamingMessageMock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_streaming(&mut self) {
        self.is_active = true;
        self.thinking.clear();
        self.content.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.tick_frame = 0;
    }

    pub fn stop_streaming(&mut self) -> (String, String) {
        self.is_active = false;
        (self.content.clone(), self.thinking.clone())
    }

    pub fn append_thinking(&mut self, text: &str) {
        self.thinking.push_str(text);
    }

    pub fn append_content(&mut self, text: &str) {
        self.content.push_str(text);
        self.md_renderer.append(text);
    }

    pub fn is_active(&self) -> bool {
        self.is_active
    }

    pub fn tick(&mut self) {
        if self.is_active {
            self.tick_frame = self.tick_frame.wrapping_add(1);
        }
    }

    fn render_thinking(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        if self.thinking.is_empty() {
            return lines;
        }

        let tokens = self.thinking.len() / 4;
        lines.push(Line::from(vec![
            Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("Thinking ({tokens} tokens)"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
        ]));

        lines.push(Line::from(""));
        lines
    }

    fn render_content(&mut self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        let md_lines = self.md_renderer.lines().to_vec();

        // Blinking indicator when active
        let indicator_style = if self.is_active {
            // Blink every 8 frames (约 800ms at 10fps)
            let visible = (self.tick_frame / 8) % 2 == 0;
            if visible {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::DIM)
            }
        } else {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        };

        if md_lines.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "◆ ",
                indicator_style,
            )]));
        } else {
            for (i, line) in md_lines.into_iter().enumerate() {
                if i == 0 {
                    let mut first_line = vec![Span::styled(
                        "◆ ",
                        indicator_style,
                    )];
                    first_line.extend(line.spans);
                    lines.push(Line::from(first_line));
                } else {
                    lines.push(line);
                }
            }
        }

        lines
    }
}

impl MockComponent for StreamingMessageMock {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        // Don't render anything if not streaming and no content
        if !self.is_active && self.content.is_empty() && self.thinking.is_empty() {
            return;
        }

        let thinking_lines = self.render_thinking();
        let mut content_lines = self.render_content();

        let mut all_lines = thinking_lines;
        all_lines.append(&mut content_lines);

        let paragraph = Paragraph::new(Text::from(all_lines))
            .wrap(tuirealm::ratatui::widgets::Wrap { trim: false });

        frame.render_widget(paragraph, area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(s) if s == "start_streaming" => {
                self.start_streaming();
            }
            Attribute::Custom(s) if s == "stop_streaming" => {
                self.stop_streaming();
            }
            Attribute::Custom(s) if s == "clear" => {
                self.content.clear();
                self.thinking.clear();
                self.md_renderer = StreamingMarkdownRenderer::new();
            }
            Attribute::Custom(s) if s == "append_thinking" => {
                if let AttrValue::String(text) = value {
                    self.append_thinking(&text);
                }
            }
            Attribute::Custom(s) if s == "append_content" => {
                if let AttrValue::String(text) = value {
                    self.append_content(&text);
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
        State::One(StateValue::String(self.content.clone()))
    }

    fn perform(&mut self, _cmd: Cmd) -> CmdResult {
        CmdResult::None
    }
}

/// Component wrapper for StreamingMessageMock
pub struct StreamingMessageComponent {
    component: StreamingMessageMock,
}

impl Default for StreamingMessageComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamingMessageComponent {
    pub fn new() -> Self {
        Self {
            component: StreamingMessageMock::new(),
        }
    }
}

impl MockComponent for StreamingMessageComponent {
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

impl Component<Msg, crate::msg::UserEvent> for StreamingMessageComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        // Handle tick events for spinner animation
        if let tuirealm::Event::Tick = ev {
            self.component.tick();
            return Some(Msg::Redraw);
        }
        None
    }
}
