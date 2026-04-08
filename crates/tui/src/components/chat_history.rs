//! Chat history component for tuirealm
//!
//! Displays historical messages (user and assistant).

use tuirealm::{
    command::{Cmd, CmdResult},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::Rect,
        style::{Modifier, Style},
        text::{Line, Span, Text},
        widgets::Paragraph,
    },
    Component, Frame, MockComponent, State,
};

use crate::{msg::Msg, theme::colors};

/// A chat message in history
#[derive(Debug, Clone)]
pub enum HistoryMessage {
    User(String),
    Assistant {
        content: String,
        thinking: Option<String>,
        thinking_folded: bool,
        thinking_elapsed_ms: Option<u64>,
    },
}

/// Component that displays chat history
#[derive(Debug, Default)]
pub struct ChatHistory {
    props: Props,
    messages: Vec<HistoryMessage>,
    scroll_offset: usize,
}

impl ChatHistory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(HistoryMessage::User(content));
    }

    pub fn add_assistant_message(
        &mut self,
        content: String,
        thinking: Option<String>,
        elapsed_ms: Option<u64>,
    ) {
        self.messages.push(HistoryMessage::Assistant {
            content,
            thinking,
            thinking_folded: true, // Default folded
            thinking_elapsed_ms: elapsed_ms,
        });
    }

    pub const fn scroll_up(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
    }

    pub const fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    pub const fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    fn render_message(&self, msg: &HistoryMessage) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        match msg {
            HistoryMessage::User(content) => {
                for (i, line) in content.lines().enumerate() {
                    let prefix = if i == 0 { "❯ " } else { "│ " };
                    lines.push(Line::from(vec![
                        Span::styled(
                            prefix,
                            Style::default()
                                .fg(colors::accent_user())
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            line.to_string(),
                            Style::default().fg(colors::text_primary()),
                        ),
                    ]));
                }
            }
            HistoryMessage::Assistant {
                content,
                thinking,
                thinking_folded,
                thinking_elapsed_ms,
            } => {
                // Render thinking summary (folded) or detail (expanded)
                if let Some(thinking) = thinking {
                    if !thinking.is_empty() {
                        let tokens = thinking.len() / 4;
                        let elapsed_str = thinking_elapsed_ms
                            .map(|ms| format!(" · {:.1}s", ms as f64 / 1000.0))
                            .unwrap_or_default();

                        if *thinking_folded {
                            // Folded: just show summary line
                            lines.push(Line::from(vec![
                                Span::styled("▶ ", Style::default().fg(colors::text_secondary())),
                                Span::styled(
                                    format!("Thinking ({tokens} tokens){elapsed_str}"),
                                    Style::default()
                                        .fg(colors::text_secondary())
                                        .add_modifier(Modifier::ITALIC),
                                ),
                            ]));
                        } else {
                            // Expanded: show all thinking content
                            lines.push(Line::from(vec![
                                Span::styled("▼ ", Style::default().fg(colors::text_secondary())),
                                Span::styled(
                                    format!("Thinking ({tokens} tokens){elapsed_str}"),
                                    Style::default()
                                        .fg(colors::text_secondary())
                                        .add_modifier(Modifier::ITALIC),
                                ),
                            ]));
                            for line in thinking.lines() {
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        "│ ",
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                    Span::styled(
                                        line.to_string(),
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                ]));
                            }
                            lines.push(Line::from(""));
                        }
                    }
                }

                // Render content
                let prefix_style = Style::default()
                    .fg(colors::accent_system())
                    .add_modifier(Modifier::BOLD);

                if content.is_empty() {
                    lines.push(Line::from(vec![Span::styled("◆ ", prefix_style)]));
                } else {
                    for (i, line) in content.lines().enumerate() {
                        if i == 0 {
                            lines.push(Line::from(vec![
                                Span::styled("◆ ", prefix_style),
                                Span::styled(
                                    line.to_string(),
                                    Style::default().fg(colors::text_primary()),
                                ),
                            ]));
                        } else {
                            lines.push(Line::from(Span::styled(
                                line.to_string(),
                                Style::default().fg(colors::text_primary()),
                            )));
                        }
                    }
                }
            }
        }

        // Add spacing between messages
        lines.push(Line::from(""));

        lines
    }

    pub fn toggle_last_thinking(&mut self) {
        for msg in self.messages.iter_mut().rev() {
            if let HistoryMessage::Assistant {
                thinking_folded, ..
            } = msg
            {
                *thinking_folded = !*thinking_folded;
                break;
            }
        }
    }
}

impl MockComponent for ChatHistory {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let mut all_lines: Vec<Line> = Vec::new();

        for msg in &self.messages {
            all_lines.extend(self.render_message(msg));
        }

        // Calculate scroll position (from bottom)
        let visible_height = area.height as usize;
        let total_lines = all_lines.len();

        let start_line = if total_lines > visible_height + self.scroll_offset {
            total_lines - visible_height - self.scroll_offset
        } else {
            0
        };

        let end_line = (start_line + visible_height).min(total_lines);
        let visible_lines: Vec<Line> = all_lines[start_line..end_line].to_vec();

        let paragraph = Paragraph::new(Text::from(visible_lines))
            .wrap(tuirealm::ratatui::widgets::Wrap { trim: false });

        frame.render_widget(paragraph, area);
    }

    fn query(&self, attr: Attribute) -> Option<AttrValue> {
        self.props.get(attr)
    }

    fn attr(&mut self, attr: Attribute, value: AttrValue) {
        match attr {
            Attribute::Custom(s) if s == "add_user_message" => {
                if let AttrValue::String(content) = value {
                    self.add_user_message(content);
                }
            }
            Attribute::Custom(s) if s == "add_assistant_message" => {
                if let AttrValue::String(content) = value {
                    self.add_assistant_message(content, None, None);
                }
            }
            Attribute::Custom(s) if s == "add_assistant_with_thinking" => {
                // Format: "content\x00thinking\x00elapsed_ms" where \x00 is a separator
                if let AttrValue::String(combined) = value {
                    let parts: Vec<&str> = combined.split('\x00').collect();
                    let content = (*parts.first().unwrap_or(&"")).to_string();
                    let thinking = parts
                        .get(1)
                        .filter(|s| !s.is_empty())
                        .map(|s| (*s).to_string());
                    let elapsed_ms = parts.get(2).and_then(|s| s.parse().ok());
                    self.add_assistant_message(content, thinking, elapsed_ms);
                }
            }
            Attribute::Custom(s) if s == "toggle_last_thinking" => {
                self.toggle_last_thinking();
            }
            Attribute::Custom(s) if s == "clear" => {
                self.messages.clear();
                self.scroll_offset = 0;
            }
            Attribute::Custom(s) if s == "scroll_to_bottom" => {
                self.scroll_to_bottom();
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

pub struct ChatHistoryComponent {
    component: ChatHistory,
}

impl Default for ChatHistoryComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatHistoryComponent {
    pub fn new() -> Self {
        Self {
            component: ChatHistory::new(),
        }
    }
}

impl MockComponent for ChatHistoryComponent {
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

impl Component<Msg, crate::msg::UserEvent> for ChatHistoryComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        // Chat history doesn't handle input events directly
        None
    }
}
