//! Unified chat view component
//!
//! Displays chat history + streaming message in a single scrollable view.

use tuirealm::{
    command::{Cmd, CmdResult},
    props::{AttrValue, Attribute, Props},
    ratatui::{
        layout::Rect,
        style::{Color, Modifier, Style},
        text::{Line, Span, Text},
        widgets::Paragraph,
    },
    Component, Frame, MockComponent, State,
};

use unicode_width::UnicodeWidthStr;

use crate::{
    markdown_stream::StreamingMarkdownRenderer,
    msg::Msg,
    theme::colors,
};

/// Tool execution status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
}

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
    Tool {
        tool_name: String,
        tool_id: String,
        status: ToolStatus,
        output: Option<String>,
        error: Option<String>,
        folded: bool,
    },
}

/// Unified chat view component
#[derive(Debug)]
pub struct ChatView {
    props: Props,
    messages: Vec<HistoryMessage>,
    scroll_offset: usize,
    // Streaming state
    streaming_content: String,
    streaming_thinking: String,
    is_streaming: bool,
    tick_frame: usize,
    md_renderer: StreamingMarkdownRenderer,
    // Track if user manually scrolled up (to pause auto-scroll)
    user_scrolled: bool,
    // Track active tool executions
    active_tools: std::collections::HashMap<String, (String, ToolStatus)>, // tool_id -> (tool_name, status)
}

impl Default for ChatView {
    fn default() -> Self {
        Self {
            props: Props::default(),
            messages: Vec::new(),
            scroll_offset: 0,
            streaming_content: String::new(),
            streaming_thinking: String::new(),
            is_streaming: false,
            tick_frame: 0,
            md_renderer: StreamingMarkdownRenderer::new(),
            user_scrolled: false,
            active_tools: std::collections::HashMap::new(),
        }
    }
}

impl ChatView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_user_message(&mut self, content: String) {
        self.messages.push(HistoryMessage::User(content));
        // Auto scroll to bottom on new message
        self.scroll_to_bottom();
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
            thinking_folded: false,
            thinking_elapsed_ms: elapsed_ms,
        });
        // Auto scroll to bottom on new message
        self.scroll_to_bottom();
    }

    pub fn start_tool(&mut self, tool_id: String, tool_name: String) {
        self.active_tools.insert(tool_id.clone(), (tool_name.clone(), ToolStatus::Running));
        self.messages.push(HistoryMessage::Tool {
            tool_name,
            tool_id,
            status: ToolStatus::Running,
            output: None,
            error: None,
            folded: false,
        });
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
    }

    pub fn complete_tool(&mut self, tool_id: String, output: String) {
        // Update the tool message in history
        for msg in self.messages.iter_mut().rev() {
            if let HistoryMessage::Tool { tool_id: id, status, output: out, .. } = msg {
                if id == &tool_id {
                    *status = ToolStatus::Completed;
                    *out = Some(output);
                    break;
                }
            }
        }
        // Update active tools tracking
        if let Some((name, _)) = self.active_tools.remove(&tool_id) {
            self.active_tools.insert(tool_id, (name, ToolStatus::Completed));
        }
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
    }

    pub fn fail_tool(&mut self, tool_id: String, error: String) {
        // Update the tool message in history
        for msg in self.messages.iter_mut().rev() {
            if let HistoryMessage::Tool { tool_id: id, status, error: err, .. } = msg {
                if id == &tool_id {
                    *status = ToolStatus::Failed;
                    *err = Some(error);
                    break;
                }
            }
        }
        // Update active tools tracking
        if let Some((name, _)) = self.active_tools.remove(&tool_id) {
            self.active_tools.insert(tool_id, (name, ToolStatus::Failed));
        }
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
    }

    pub fn start_streaming(&mut self) {
        self.is_streaming = true;
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.tick_frame = 0;
        self.scroll_offset = 0;
        // Reset user scrolled state for new streaming session
        self.user_scrolled = false;
    }

    pub fn stop_streaming(&mut self) {
        self.is_streaming = false;
    }

    pub fn clear_streaming(&mut self) {
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.is_streaming = false;
    }

    pub fn append_streaming_content(&mut self, text: &str) {
        self.streaming_content.push_str(text);
        self.md_renderer.append(text);
        // Auto scroll to bottom only if user hasn't manually scrolled up
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
    }

    pub fn append_streaming_thinking(&mut self, text: &str) {
        self.streaming_thinking.push_str(text);
        // Auto scroll to bottom only if user hasn't manually scrolled up
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
    }

    pub fn tick(&mut self) {
        if self.is_streaming {
            self.tick_frame = self.tick_frame.wrapping_add(1);
        }
    }

    pub fn scroll_up(&mut self, amount: usize) {
        let total_lines = self.calculate_total_lines();
        let max_scroll = total_lines.saturating_sub(5); // Keep at least 5 lines visible
        self.scroll_offset = (self.scroll_offset + amount).min(max_scroll);
        // User manually scrolled up, pause auto-scroll
        self.user_scrolled = true;
    }

    pub fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        // If scrolled to bottom, resume auto-scroll
        if self.scroll_offset == 0 {
            self.user_scrolled = false;
        }
    }

    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        // User scrolled to bottom, resume auto-scroll
        self.user_scrolled = false;
    }

    pub fn toggle_last_thinking(&mut self) {
        for msg in self.messages.iter_mut().rev() {
            if let HistoryMessage::Assistant { thinking_folded, .. } = msg {
                *thinking_folded = !*thinking_folded;
                break;
            }
        }
    }

    fn calculate_total_lines(&mut self) -> usize {
        let mut count = 0;
        for msg in &self.messages {
            count += self.count_message_lines(msg);
        }
        if self.is_streaming || !self.streaming_content.is_empty() {
            count += self.count_streaming_lines();
        }
        count
    }

    fn count_message_lines(&self, msg: &HistoryMessage) -> usize {
        let mut count = 0;
        match msg {
            HistoryMessage::User(content) => {
                count += content.lines().count();
            }
            HistoryMessage::Assistant {
                content,
                thinking,
                thinking_folded,
                ..
            } => {
                if let Some(thinking) = thinking {
                    if !thinking.is_empty() {
                        if *thinking_folded {
                            count += 1;
                        } else {
                            count += 2 + thinking.lines().count();
                        }
                    }
                }
                // Count markdown-rendered lines
                if content.is_empty() {
                    count += 1;
                } else {
                    let mut md_renderer = StreamingMarkdownRenderer::new();
                    md_renderer.set_content(content.clone());
                    count += md_renderer.lines().len();
                }
            }
            HistoryMessage::Tool {
                output,
                error,
                folded,
                ..
            } => {
                count += 1; // Header line
                if !*folded {
                    if let Some(err) = error {
                        count += err.lines().count();
                    } else if let Some(out) = output {
                        count += out.lines().count();
                    } else {
                        count += 1; // "Running..." placeholder
                    }
                    count += 1; // Extra line after content
                }
            }
        }
        count += 1; // spacing
        count
    }

    fn count_streaming_lines(&mut self) -> usize {
        let mut count = 0;
        if !self.streaming_thinking.is_empty() {
            count += 1 + self.streaming_thinking.lines().count();
        }
        let content_lines = self.md_renderer.lines().len().max(1);
        count += content_lines;
        count += 1; // spacing
        count
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
                        Span::styled(line.to_string(), Style::default().fg(Color::White)),
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
                            lines.push(Line::from(vec![
                                Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
                                Span::styled(
                                    format!("Thinking ({tokens} tokens){}", elapsed_str),
                                    Style::default()
                                        .fg(Color::DarkGray)
                                        .add_modifier(Modifier::ITALIC),
                                ),
                            ]));
                        } else {
                            lines.push(Line::from(vec![
                                Span::styled("▼ ", Style::default().fg(Color::DarkGray)),
                                Span::styled(
                                    format!("Thinking ({tokens} tokens){}", elapsed_str),
                                    Style::default()
                                        .fg(Color::DarkGray)
                                        .add_modifier(Modifier::ITALIC),
                                ),
                            ]));
                            for line in thinking.lines() {
                                lines.push(Line::from(vec![
                                    Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                                    Span::styled(
                                        line.to_string(),
                                        Style::default().fg(Color::DarkGray),
                                    ),
                                ]));
                            }
                            lines.push(Line::from(""));
                        }
                    }
                }

                // Render content with markdown (no indicator)
                if content.is_empty() {
                    lines.push(Line::from(""));
                } else {
                    let mut md_renderer = StreamingMarkdownRenderer::new();
                    md_renderer.set_content(content.clone());
                    let md_lines = md_renderer.lines();

                    for line in md_lines.iter() {
                        lines.push(line.clone());
                    }
                }
            }
            HistoryMessage::Tool {
                tool_name,
                status,
                output,
                error,
                folded,
                ..
            } => {
                let (icon, color) = match status {
                    ToolStatus::Running => ("⚡", Color::Yellow),
                    ToolStatus::Completed => ("✓", Color::Green),
                    ToolStatus::Failed => ("✗", Color::Red),
                };

                let fold_icon = if *folded { "▶ " } else { "▼ " };

                lines.push(Line::from(vec![
                    Span::styled(fold_icon, Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{} {} ", icon, tool_name),
                        Style::default().fg(color).add_modifier(Modifier::BOLD),
                    ),
                ]));

                if !*folded {
                    if let Some(err) = error {
                        for line in err.lines() {
                            lines.push(Line::from(vec![
                                Span::styled("│ ", Style::default().fg(Color::Red)),
                                Span::styled(line.to_string(), Style::default().fg(Color::Red)),
                            ]));
                        }
                    } else if let Some(out) = output {
                        for line in out.lines() {
                            lines.push(Line::from(vec![
                                Span::styled("│ ", Style::default().fg(colors::accent_system())),
                                Span::styled(line.to_string(), Style::default().fg(Color::White)),
                            ]));
                        }
                    } else if *status == ToolStatus::Running {
                        lines.push(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                            Span::styled("Running...", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
                        ]));
                    }
                    lines.push(Line::from(""));
                }
            }
        }

        lines.push(Line::from(""));
        lines
    }

    fn render_streaming(&mut self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Render thinking if present
        if !self.streaming_thinking.is_empty() {
            let tokens = self.streaming_thinking.len() / 4;
            lines.push(Line::from(vec![
                Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("Thinking ({tokens} tokens)"),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
            for line in self.streaming_thinking.lines() {
                lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(line.to_string(), Style::default().fg(Color::DarkGray)),
                ]));
            }
            lines.push(Line::from(""));
        }

        // Render content (no indicator, status shown in status bar)
        let md_lines = self.md_renderer.lines();

        for line in md_lines.iter() {
            lines.push(line.clone());
        }

        // Add empty line placeholder when no content yet
        if md_lines.is_empty() {
            lines.push(Line::from(""));
        }

        lines.push(Line::from(""));
        lines
    }
}

impl MockComponent for ChatView {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let mut all_lines: Vec<Line> = Vec::new();

        // Render history
        for msg in &self.messages {
            all_lines.extend(self.render_message(msg));
        }

        // Render streaming content (if any)
        if self.is_streaming || !self.streaming_content.is_empty() {
            all_lines.extend(self.render_streaming());
        }

        // Calculate scroll position with wrap support
        let visible_height = area.height as usize;
        let width = area.width as usize;

        // Calculate wrapped line counts and find start line
        let start_line = if self.scroll_offset == 0 {
            // At bottom: work backwards to find which lines fit
            let mut wrapped_lines = 0;
            let mut start = 0;
            for (i, line) in all_lines.iter().enumerate().rev() {
                let line_width: usize = line.spans.iter()
                    .map(|s| unicode_width::UnicodeWidthStr::width(s.content.as_ref()))
                    .sum();
                let wrapped_height = (line_width + width.saturating_sub(1)) / width.max(1);
                let wrapped_height = wrapped_height.max(1);

                if wrapped_lines + wrapped_height > visible_height {
                    start = i + 1;
                    break;
                }
                wrapped_lines += wrapped_height;
                if i == 0 {
                    break;
                }
            }
            start
        } else {
            // Manual scroll: use simple line-based calculation
            let total_lines = all_lines.len();
            if total_lines > visible_height + self.scroll_offset {
                total_lines - visible_height - self.scroll_offset
            } else {
                0
            }
        };

        let end_line = (start_line + visible_height).min(all_lines.len());
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
            Attribute::Custom(s) if s == "add_assistant_with_thinking" => {
                if let AttrValue::String(combined) = value {
                    let parts: Vec<&str> = combined.split('\x00').collect();
                    let content = parts.get(0).unwrap_or(&"").to_string();
                    let thinking = parts.get(1).filter(|s| !s.is_empty()).map(|s| s.to_string());
                    let elapsed_ms = parts.get(2).and_then(|s| s.parse().ok());
                    self.add_assistant_message(content, thinking, elapsed_ms);
                }
            }
            Attribute::Custom(s) if s == "start_streaming" => {
                self.start_streaming();
            }
            Attribute::Custom(s) if s == "stop_streaming" => {
                self.stop_streaming();
            }
            Attribute::Custom(s) if s == "clear_streaming" => {
                self.clear_streaming();
            }
            Attribute::Custom(s) if s == "append_content" => {
                if let AttrValue::String(text) = value {
                    self.append_streaming_content(&text);
                }
            }
            Attribute::Custom(s) if s == "append_thinking" => {
                if let AttrValue::String(text) = value {
                    self.append_streaming_thinking(&text);
                }
            }
            Attribute::Custom(s) if s == "scroll_up" => {
                self.scroll_up(3);
            }
            Attribute::Custom(s) if s == "scroll_down" => {
                self.scroll_down(3);
            }
            Attribute::Custom(s) if s == "scroll_to_bottom" => {
                self.scroll_to_bottom();
            }
            Attribute::Custom(s) if s == "toggle_thinking" => {
                self.toggle_last_thinking();
            }
            Attribute::Custom(s) if s == "start_tool" => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.get(0).unwrap_or(&"").to_string();
                    let tool_name = parts.get(1).unwrap_or(&"tool").to_string();
                    self.start_tool(tool_id, tool_name);
                }
            }
            Attribute::Custom(s) if s == "complete_tool" => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.get(0).unwrap_or(&"").to_string();
                    let output = parts.get(1).unwrap_or(&"").to_string();
                    self.complete_tool(tool_id, output);
                }
            }
            Attribute::Custom(s) if s == "fail_tool" => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = parts.get(0).unwrap_or(&"").to_string();
                    let error = parts.get(1).unwrap_or(&"").to_string();
                    self.fail_tool(tool_id, error);
                }
            }
            _ => {
                self.props.set(attr, value);
            }
        }
    }

    fn state(&self) -> State {
        State::None
    }

    fn perform(&mut self, cmd: Cmd) -> CmdResult {
        match cmd {
            Cmd::Move(tuirealm::command::Direction::Up) => {
                self.scroll_up(1);
                CmdResult::None
            }
            Cmd::Move(tuirealm::command::Direction::Down) => {
                self.scroll_down(1);
                CmdResult::None
            }
            _ => CmdResult::None,
        }
    }
}

/// Component wrapper
pub struct ChatViewComponent {
    component: ChatView,
}

impl Default for ChatViewComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatViewComponent {
    pub fn new() -> Self {
        Self {
            component: ChatView::new(),
        }
    }
}

impl MockComponent for ChatViewComponent {
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

impl Component<Msg, crate::msg::UserEvent> for ChatViewComponent {
    fn on(&mut self, ev: tuirealm::Event<crate::msg::UserEvent>) -> Option<Msg> {
        // Keyboard events are handled at app level via InputComponent
        // Only handle Tick here for the blinking indicator
        if let tuirealm::Event::Tick = ev {
            self.component.tick();
            Some(Msg::Redraw)
        } else {
            None
        }
    }
}
