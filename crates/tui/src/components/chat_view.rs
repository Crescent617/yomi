//! Unified chat view component
//!
//! Displays chat history + streaming message in a single scrollable view.

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

use crate::{chars, markdown_stream::StreamingMarkdownRenderer, msg::Msg, theme::colors};

/// Tool execution status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
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
        arguments: Option<String>,
        elapsed_ms: Option<u64>,
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
    // Expand all mode (ctrl-o): show all thinking and tool details
    expand_all: bool,
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
            expand_all: false,
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
            thinking_folded: !self.expand_all,
            thinking_elapsed_ms: elapsed_ms,
        });
        // Auto scroll to bottom on new message
        self.scroll_to_bottom();
    }

    pub fn start_tool(&mut self, tool_id: String, tool_name: String, arguments: Option<String>) {
        // Flush any pending streaming content before starting tool
        self.flush_streaming();

        self.active_tools
            .insert(tool_id.clone(), (tool_name.clone(), ToolStatus::Running));
        self.messages.push(HistoryMessage::Tool {
            tool_name,
            tool_id,
            status: ToolStatus::Running,
            output: None,
            error: None,
            folded: !self.expand_all,
            arguments,
            elapsed_ms: None,
        });
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
    }

    pub fn complete_tool(&mut self, tool_id: String, output: String, elapsed_ms: u64) {
        // Update the tool message in history
        for msg in self.messages.iter_mut().rev() {
            if let HistoryMessage::Tool {
                tool_id: id,
                status,
                output: out,
                elapsed_ms: elapsed,
                ..
            } = msg
            {
                if id == &tool_id {
                    *status = ToolStatus::Completed;
                    *out = Some(output);
                    *elapsed = Some(elapsed_ms);
                    break;
                }
            }
        }
        // Update active tools tracking
        if let Some((name, _)) = self.active_tools.remove(&tool_id) {
            self.active_tools
                .insert(tool_id, (name, ToolStatus::Completed));
        }
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
    }

    pub fn fail_tool(&mut self, tool_id: String, error: String, elapsed_ms: u64) {
        // Update the tool message in history
        for msg in self.messages.iter_mut().rev() {
            if let HistoryMessage::Tool {
                tool_id: id,
                status,
                error: err,
                elapsed_ms: elapsed,
                ..
            } = msg
            {
                if id == &tool_id {
                    *status = ToolStatus::Failed;
                    *err = Some(error);
                    *elapsed = Some(elapsed_ms);
                    break;
                }
            }
        }
        // Update active tools tracking
        if let Some((name, _)) = self.active_tools.remove(&tool_id) {
            self.active_tools
                .insert(tool_id, (name, ToolStatus::Failed));
        }
        if !self.user_scrolled {
            self.scroll_offset = 0;
        }
    }

    /// Flush pending streaming content to history
    /// Called when a new block starts (tool, code block, etc.) to preserve current content
    pub fn flush_streaming(&mut self) {
        // If there's pending thinking content, save it as an assistant message
        if !self.streaming_thinking.is_empty() {
            self.messages.push(HistoryMessage::Assistant {
                content: String::new(),
                thinking: Some(self.streaming_thinking.clone()),
                thinking_folded: !self.expand_all,
                thinking_elapsed_ms: None,
            });
            self.streaming_thinking.clear();
        }

        // If there's pending content, save it as an assistant message
        if !self.streaming_content.is_empty() {
            self.messages.push(HistoryMessage::Assistant {
                content: self.streaming_content.clone(),
                thinking: None,
                thinking_folded: true,
                thinking_elapsed_ms: None,
            });
            self.streaming_content.clear();
            self.md_renderer = StreamingMarkdownRenderer::new();
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

    pub const fn stop_streaming(&mut self) {
        self.is_streaming = false;
    }

    pub fn clear_streaming(&mut self) {
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.is_streaming = false;
    }

    /// Cancel streaming - flush partial content and mark running tools as cancelled
    pub fn cancel_streaming(&mut self) {
        // Note: Content is already saved by app.rs via add_assistant_with_thinking
        // Just clear streaming buffers without flushing to avoid duplicates
        self.streaming_content.clear();
        self.streaming_thinking.clear();
        self.md_renderer = StreamingMarkdownRenderer::new();
        self.is_streaming = false;
        // Mark any running tools as cancelled
        for (tool_id, (_, status)) in &mut self.active_tools {
            if *status == ToolStatus::Running {
                *status = ToolStatus::Cancelled;
                for msg in &mut self.messages {
                    if let HistoryMessage::Tool { tool_id: id, status: s, .. } = msg {
                        if id == tool_id {
                            *s = ToolStatus::Cancelled;
                        }
                    }
                }
            }
        }
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

    pub const fn tick(&mut self) {
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

    pub const fn scroll_down(&mut self, amount: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
        // If scrolled to bottom, resume auto-scroll
        if self.scroll_offset == 0 {
            self.user_scrolled = false;
        }
    }

    pub const fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
        // User scrolled to bottom, resume auto-scroll
        self.user_scrolled = false;
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

    pub fn toggle_expand_all(&mut self) {
        self.expand_all = !self.expand_all;
        // Update all messages to reflect expand_all state
        for msg in &mut self.messages {
            match msg {
                HistoryMessage::Assistant {
                    thinking_folded, ..
                } => {
                    *thinking_folded = !self.expand_all;
                }
                HistoryMessage::Tool { folded, .. } => {
                    *folded = !self.expand_all;
                }
                _ => {}
            }
        }
    }

    fn calculate_total_lines(&mut self) -> usize {
        let mut count = 0;
        for msg in &self.messages {
            count += self.count_message_lines(msg);
        }
        if self.is_streaming
            || !self.streaming_content.is_empty()
            || !self.streaming_thinking.is_empty()
        {
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
                            lines.push(Line::from(vec![Span::styled(
                                format!("Thinking ({tokens} tokens){elapsed_str}"),
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::ITALIC),
                            )]));
                        } else {
                            lines.push(Line::from(vec![Span::styled(
                                format!("Thinking ({tokens} tokens){elapsed_str}"),
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::ITALIC),
                            )]));
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
                        }
                    }
                }

                // Add separator between thinking and content if both exist
                if thinking.as_ref().map_or(false, |t| !t.is_empty()) && !content.is_empty() {
                    lines.push(Line::from(""));
                }

                // Render content with markdown (no indicator)
                // Note: no empty line here, thinking already adds one if present
                if !content.is_empty() {
                    let mut md_renderer = StreamingMarkdownRenderer::new();
                    md_renderer.set_content(content.clone());
                    let md_lines = md_renderer.lines();

                    for line in md_lines {
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
                arguments,
                elapsed_ms,
                ..
            } => {
                let (icon, color) = match status {
                    ToolStatus::Running => ("", colors::accent_warning()),
                    ToolStatus::Completed => ("", colors::accent_success()),
                    ToolStatus::Failed => ("", colors::accent_error()),
                    ToolStatus::Cancelled => ("", colors::text_secondary()),
                };

                // Build header with execution time (only show if >= 1s)
                let time_str = elapsed_ms
                    .filter(|ms| *ms >= 1000)
                    .map(|ms| format!(" {:.1}s", ms as f64 / 1000.0))
                    .unwrap_or_default();

                // Peek args in folded mode (max 30 chars, compact whitespace)
                let peek_args = if *folded {
                    arguments.as_ref().and_then(|args| {
                        // Compact whitespace: replace newlines/tabs with single space
                        let compact = args;
                        if compact.is_empty() {
                            None
                        } else {
                            let peek = if compact.len() > 80 {
                                format!("{}...", &compact[..77])
                            } else {
                                compact.to_string()
                            };
                            Some(peek)
                        }
                    })
                } else {
                    None
                };

                // Build header line
                // Capitalize first letter of tool_name
                let mut chars = tool_name.chars();
                let tool_name_capitalized = chars
                    .next()
                    .map(|c| c.to_uppercase().to_string() + chars.as_str())
                    .unwrap_or_default();
                let mut header_spans = vec![Span::styled(
                    format!("{icon} {tool_name_capitalized}{time_str}"),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                )];
                if let Some(peek) = peek_args {
                    header_spans.push(Span::styled(
                        format!(" {peek}"),
                        Style::default().fg(colors::text_secondary()),
                    ));
                }
                lines.push(Line::from(header_spans));

                // Output peek in folded mode (max 50 chars, indented)
                if *folded {
                    let peek_output = error.as_ref().or(output.as_ref()).and_then(|out| {
                        let trimmed = out.trim();
                        if trimmed.is_empty() {
                            None
                        } else {
                            let peek = if trimmed.len() > 200 {
                                format!("{}...", &trimmed[..197])
                            } else {
                                trimmed.to_string()
                            };
                            Some(peek.split_whitespace().collect::<Vec<_>>().join(" "))
                        }
                    });
                    if let Some(peek) = peek_output {
                        lines.push(Line::from(vec![
                            Span::styled("⎿ ", Style::default().fg(colors::text_secondary())),
                            Span::styled(peek, Style::default().fg(colors::text_secondary())),
                        ]));
                    }
                }

                if !*folded {
                    // Show tool arguments if available
                    if let Some(args) = arguments {
                        if !args.is_empty() {
                            lines.push(Line::from(vec![
                                Span::styled("│ ", Style::default().fg(colors::text_secondary())),
                                Span::styled(
                                    "Arguments:",
                                    Style::default()
                                        .fg(colors::text_secondary())
                                        .add_modifier(Modifier::BOLD),
                                ),
                            ]));
                            for line in args.lines() {
                                lines.push(Line::from(vec![
                                    Span::styled(
                                        "│   ",
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                    Span::styled(
                                        line.to_string(),
                                        Style::default().fg(colors::text_secondary()),
                                    ),
                                ]));
                            }
                        }
                    }

                    if let Some(err) = error {
                        for line in err.lines() {
                            lines.push(Line::from(vec![
                                Span::styled("│ ", Style::default().fg(colors::accent_error())),
                                Span::styled(
                                    line.to_string(),
                                    Style::default().fg(colors::accent_error()),
                                ),
                            ]));
                        }
                    } else if let Some(out) = output {
                        lines.push(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(colors::text_secondary())),
                            Span::styled(
                                "Output:",
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::BOLD),
                            ),
                        ]));
                        for line in out.lines() {
                            lines.push(Line::from(vec![
                                Span::styled("│ ", Style::default().fg(colors::accent_system())),
                                Span::styled(
                                    line.to_string(),
                                    Style::default().fg(colors::text_primary()),
                                ),
                            ]));
                        }
                    } else if *status == ToolStatus::Running {
                        lines.push(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(colors::text_secondary())),
                            Span::styled(
                                "Running...",
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                    } else if *status == ToolStatus::Cancelled {
                        lines.push(Line::from(vec![
                            Span::styled("│ ", Style::default().fg(colors::text_secondary())),
                            Span::styled(
                                "Cancelled",
                                Style::default()
                                    .fg(colors::text_secondary())
                                    .add_modifier(Modifier::ITALIC),
                            ),
                        ]));
                    }
                }
            }
        }

        lines
    }

    fn render_streaming(&mut self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();

        // Render thinking if present (collapsed by default, expanded in expand_all mode)
        if !self.streaming_thinking.is_empty() {
            let tokens = self.streaming_thinking.len() / 4;
            lines.push(Line::from(vec![Span::styled(
                format!("Thinking ({tokens} tokens)"),
                Style::default()
                    .fg(colors::text_secondary())
                    .add_modifier(Modifier::ITALIC),
            )]));
            // Show thinking content only in expand_all mode
            if self.expand_all {
                for line in self.streaming_thinking.lines() {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(colors::text_secondary())),
                        Span::styled(
                            line.to_string(),
                            Style::default().fg(colors::text_secondary()),
                        ),
                    ]));
                }
            }
        }

        // Render content (no indicator, status shown in status bar)
        // Add separator between thinking and content
        if !self.streaming_thinking.is_empty() && !self.streaming_content.is_empty() {
            lines.push(Line::from(""));
        }
        let md_lines = self.md_renderer.lines();

        for line in md_lines {
            lines.push(line.clone());
        }

        // Add empty line placeholder only if no thinking (thinking already adds one)
        if md_lines.is_empty() && self.streaming_thinking.is_empty() {
            lines.push(Line::from(""));
        }

        lines
    }
}

impl MockComponent for ChatView {
    fn view(&mut self, frame: &mut Frame, area: Rect) {
        let mut all_lines: Vec<Line> = Vec::new();

        // Render history with unified spacing
        for (i, msg) in self.messages.iter().enumerate() {
            all_lines.extend(self.render_message(msg));
            // Add spacing between messages (but not after the last one)
            if i < self.messages.len() - 1 {
                all_lines.push(Line::from(""));
            }
        }

        // Render streaming content (if any)
        // Add spacing if there's history before streaming
        if !self.messages.is_empty()
            && (self.is_streaming
                || !self.streaming_content.is_empty()
                || !self.streaming_thinking.is_empty())
        {
            all_lines.push(Line::from(""));
        }
        if self.is_streaming
            || !self.streaming_content.is_empty()
            || !self.streaming_thinking.is_empty()
        {
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
                let line_width: usize = line
                    .spans
                    .iter()
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
                    let content = (*parts.first().unwrap_or(&"")).to_string();
                    let thinking = parts
                        .get(1)
                        .filter(|s| !s.is_empty())
                        .map(|s| (*s).to_string());
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
            Attribute::Custom(s) if s == "cancel_streaming" => {
                self.cancel_streaming();
            }
            Attribute::Custom(s) if s == "cancel_streaming_with_content" => {
                if let AttrValue::String(combined) = value {
                    let parts: Vec<&str> = combined.split(' ').collect();
                    let content = (*parts.first().unwrap_or(&"")).to_string();
                    let thinking = parts.get(1).filter(|s| !s.is_empty()).map(|s| s.to_string());
                    
                    // Add to history with cancelled flag
                    self.messages.push(HistoryMessage::Assistant {
                        content,
                        thinking,
                        thinking_folded: !self.expand_all,
                        thinking_elapsed_ms: parts.get(2).and_then(|s| s.parse().ok()),
                    });
                    
                    // Mark running tools as cancelled
                    for (tool_id, (_, status)) in self.active_tools.iter_mut() {
                        if *status == ToolStatus::Running {
                            *status = ToolStatus::Cancelled;
                            for msg in &mut self.messages {
                                if let HistoryMessage::Tool { tool_id: id, status: s, .. } = msg {
                                    if id == tool_id {
                                        *s = ToolStatus::Cancelled;
                                    }
                                }
                            }
                        }
                    }
                    
                    // Clear streaming state
                    self.streaming_content.clear();
                    self.streaming_thinking.clear();
                    self.md_renderer = StreamingMarkdownRenderer::new();
                    self.is_streaming = false;
                }
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
            Attribute::Custom(s) if s == "toggle_expand_all" => {
                self.toggle_expand_all();
            }
            Attribute::Custom(s) if s == "start_tool" => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = (*parts.first().unwrap_or(&"")).to_string();
                    let tool_name = (*parts.get(1).unwrap_or(&"tool")).to_string();
                    let arguments = parts.get(2).map(|s| s.to_string());
                    self.start_tool(tool_id, tool_name, arguments);
                }
            }
            Attribute::Custom(s) if s == "complete_tool" => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = (*parts.first().unwrap_or(&"")).to_string();
                    let output = (*parts.get(1).unwrap_or(&"")).to_string();
                    let elapsed_ms = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    self.complete_tool(tool_id, output, elapsed_ms);
                }
            }
            Attribute::Custom(s) if s == "fail_tool" => {
                if let AttrValue::String(text) = value {
                    let parts: Vec<&str> = text.split('\x00').collect();
                    let tool_id = (*parts.first().unwrap_or(&"")).to_string();
                    let error = (*parts.get(1).unwrap_or(&"")).to_string();
                    let elapsed_ms = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    self.fail_tool(tool_id, error, elapsed_ms);
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
        if ev == tuirealm::Event::Tick {
            self.component.tick();
            Some(Msg::Redraw)
        } else {
            None
        }
    }
}
