//! Main application - alt screen with transparent background

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    layout::{Constraint, Direction, Layout, Position},
    style::{Color, Modifier, Style, Stylize},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Paragraph, Wrap},
    DefaultTerminal, Frame,
};
use std::io::stdout;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use kernel::event::Event as AppEvent;

use crate::model::{ChatMessage, Model, Role};

pub struct App {
    event_rx: mpsc::Receiver<AppEvent>,
    input_tx: mpsc::Sender<String>,
    model: Model,
    should_quit: bool,
    last_ctrl_c: Option<Instant>,
    input_buffer: String,
    cursor_pos: usize,
    scroll_offset: usize,
    viewport_height: usize,
}

impl App {
    pub fn new(event_rx: mpsc::Receiver<AppEvent>, input_tx: mpsc::Sender<String>) -> Result<Self> {
        Ok(Self {
            event_rx,
            input_tx,
            model: Model::default(),
            should_quit: false,
            last_ctrl_c: None,
            input_buffer: String::new(),
            cursor_pos: 0,
            scroll_offset: 0,
            viewport_height: 10,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        stdout().execute(EnterAlternateScreen)?;
        let mut terminal = ratatui::init();

        let result = self.run_loop(&mut terminal).await;

        // Restore terminal
        ratatui::restore();
        stdout().execute(LeaveAlternateScreen)?;
        disable_raw_mode()?;

        result
    }

    async fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        let last_tick = Instant::now();

        loop {
            if self.should_quit {
                break;
            }

            // Draw UI
            terminal.draw(|frame| self.draw(frame))?;

            // Handle events
            let timeout = Duration::from_millis(50);
            if event::poll(timeout)? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == event::KeyEventKind::Press {
                        self.handle_key(key.code, key.modifiers).await?;
                    }
                }
            }

            // Check for app events from core
            if let Ok(event) = self.event_rx.try_recv() {
                self.handle_app_event(&event).await?;
            }

            // Auto-scroll to bottom on new content
            if self.model.streaming.is_active || self.scroll_offset == 0 {
                self.scroll_offset = 0;
            }
        }

        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        self.viewport_height = area.height as usize;

        // Layout: chat area (top) + status (optional) + input (bottom)
        let input_height = self.input_buffer.lines().count().max(1) as u16 + 1; // +1 for border
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),         // Chat area
                Constraint::Length(input_height), // Input area
            ])
            .split(area);

        let chat_area = chunks[0];
        let input_area = chunks[1];

        // Draw chat messages
        self.draw_chat(frame, chat_area);

        // Draw input
        self.draw_input(frame, input_area);
    }

    fn draw_chat(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        // Build message lines
        let mut lines: Vec<Line> = Vec::new();

        // Add welcome if no messages
        if self.model.messages.is_empty() && !self.model.streaming.is_active {
            lines.push(Line::from(""));
            lines.push(Line::from(
                Span::styled("Welcome to yomi", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
            ));
            lines.push(Line::from(
                Span::styled("Your AI coding assistant", Style::default().fg(Color::DarkGray))
            ));
            lines.push(Line::from(""));
            lines.push(Line::from(
                Span::styled("Press Ctrl+C twice to exit", Style::default().fg(Color::DarkGray))
            ));
        }

        // Add messages
        for msg in &self.model.messages {
            lines.extend(self.message_to_lines(msg));
            lines.push(Line::from("")); // spacing between messages
        }

        // Add streaming content if active
        if self.model.streaming.is_active {
            lines.extend(self.streaming_to_lines());
        }

        // Calculate scroll
        let total_lines = lines.len();
        let visible_lines = area.height as usize;
        let scroll = if total_lines > visible_lines {
            (total_lines - visible_lines).saturating_sub(self.scroll_offset)
        } else {
            0
        };

        // Create paragraph with scroll
        let paragraph = Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((scroll as u16, 0));

        frame.render_widget(paragraph, area);
    }

    fn message_to_lines(&self, msg: &ChatMessage) -> Vec<Line<'_>> {
        let mut lines = Vec::new();

        let (prefix, prefix_color) = match msg.role {
            Role::User => ("❯", Color::Magenta),
            Role::Assistant => ("◆", Color::Cyan),
            Role::System => ("▪", Color::DarkGray),
        };

        // Thinking block
        if let Some(ref thinking) = msg.thinking {
            if !thinking.is_empty() && !msg.thinking_folded {
                let tokens = thinking.len() / 4;
                lines.push(Line::from(vec![
                    Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("Thinking ({tokens} tokens)"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
                ]));
                for line in thinking.lines() {
                    lines.push(Line::from(vec![
                        Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                        Span::styled(line.to_string(), Style::default().fg(Color::DarkGray)),
                    ]));
                }
                lines.push(Line::from(""));
            } else if !thinking.is_empty() {
                let tokens = thinking.len() / 4;
                lines.push(Line::from(vec![
                    Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("Thinking ({tokens} tokens)"),
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
                ]));
            }
        }

        // Content
        for (i, line) in msg.content.lines().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled(format!("{prefix} "),
                        Style::default().fg(prefix_color).add_modifier(Modifier::BOLD)),
                    Span::styled(line.to_string(), Style::default().fg(Color::White)),
                ]));
            } else {
                let indent = if matches!(msg.role, Role::User) { "│ " } else { "  " };
                lines.push(Line::from(vec![
                    Span::styled(indent, Style::default().fg(prefix_color)),
                    Span::styled(line.to_string(), Style::default().fg(Color::White)),
                ]));
            }
        }

        lines
    }

    fn streaming_to_lines(&self) -> Vec<Line<'_>> {
        let mut lines = Vec::new();
        let streaming = &self.model.streaming;

        // Thinking
        if !streaming.thinking.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("▶ ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("Thinking ({} tokens)", streaming.thinking.len() / 4),
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
            ]));
            for line in streaming.thinking.lines() {
                lines.push(Line::from(vec![
                    Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(line.to_string(), Style::default().fg(Color::DarkGray)),
                ]));
            }
            lines.push(Line::from(""));
        }

        // Content with spinner
        for (i, line) in streaming.content.lines().enumerate() {
            if i == 0 {
                lines.push(Line::from(vec![
                    Span::styled("◆ ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
                    Span::styled(line.to_string(), Style::default().fg(Color::White)),
                ]));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("  ", Style::default()),
                    Span::styled(line.to_string(), Style::default().fg(Color::White)),
                ]));
            }
        }

        // Add spinner at end
        if !lines.is_empty() {
            let spinner = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
            let spinner_char = spinner[(streaming.spinner_frame / 2) % spinner.len()];
            if let Some(last) = lines.last_mut() {
                last.spans.push(Span::styled(
                    format!(" {spinner_char}"),
                    Style::default().fg(Color::Magenta),
                ));
            }
        }

        lines
    }

    fn draw_input(&self, frame: &mut Frame, area: ratatui::layout::Rect) {
        let lines: Vec<Line> = self.input_buffer
            .lines()
            .enumerate()
            .map(|(i, line)| {
                let prefix = if i == 0 { "❯ " } else { "│ " };
                Line::from(vec![
                    Span::styled(prefix, Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                    Span::styled(line.to_string(), Style::default().fg(Color::White)),
                ])
            })
            .collect();

        let text = if lines.is_empty() {
            Text::from(vec![Line::from(vec![
                Span::styled("❯ ", Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
                Span::styled("Type a message...", Style::default().fg(Color::DarkGray)),
            ])])
        } else {
            Text::from(lines)
        };

        let input_widget = Paragraph::new(text)
            .block(Block::default().borders(Borders::TOP).border_style(Color::DarkGray));

        frame.render_widget(input_widget, area);

        // Position cursor
        let cursor_line = self.input_buffer[..self.cursor_pos.min(self.input_buffer.len())]
            .chars()
            .filter(|&c| c == '\n')
            .count();
        let line_start = self.input_buffer[..self.cursor_pos.min(self.input_buffer.len())]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        // Calculate display column by counting Unicode width up to cursor
        let line_content = &self.input_buffer[line_start..self.cursor_pos.min(self.input_buffer.len())];
        let col = unicode_width::UnicodeWidthStr::width(line_content);

        let cursor_x = if cursor_line == 0 { 2 } else { 2 } + col;
        let cursor_y = area.y + cursor_line as u16 + 1; // +1 for border

        if cursor_y < area.y + area.height {
            frame.set_cursor_position(Position::new(cursor_x as u16, cursor_y));
        }
    }

    async fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
        let ctrl = modifiers.contains(KeyModifiers::CONTROL);

        match (code, ctrl) {
            (KeyCode::Char('c'), true) => {
                if self.handle_ctrl_c() {
                    return Ok(());
                }
            }
            (KeyCode::Char('j'), true) => self.insert_char('\n'),
            (KeyCode::Char('w'), true) => self.delete_word(),
            (KeyCode::Char('u'), true) => self.delete_to_start(),
            (KeyCode::Char('k'), true) => self.delete_to_end(),
            (KeyCode::Char('a'), true) => self.move_to_start(),
            (KeyCode::Char('e'), true) => self.move_to_end(),
            (KeyCode::Char('p'), true) => self.scroll_up(),
            (KeyCode::Char('n'), true) => self.scroll_down(),
            (KeyCode::Up, false) => self.scroll_up(),
            (KeyCode::Down, false) => self.scroll_down(),
            (KeyCode::PageUp, false) => self.scroll_page_up(),
            (KeyCode::PageDown, false) => self.scroll_page_down(),
            (KeyCode::Enter, false) => {
                if !self.input_buffer.is_empty() {
                    let content = self.input_buffer.clone();
                    self.input_tx.send(content.clone()).await?;
                    self.input_buffer.clear();
                    self.cursor_pos = 0;
                    self.model.add_user_message(content);
                }
            }
            (KeyCode::Char(c), false) => self.insert_char(c),
            (KeyCode::Backspace, false) => self.backspace(),
            (KeyCode::Delete, false) => self.delete_char(),
            (KeyCode::Left, false) => self.move_left(),
            (KeyCode::Right, false) => self.move_right(),
            (KeyCode::Tab, false) => self.toggle_fold(),
            _ => {}
        }

        Ok(())
    }

    fn handle_ctrl_c(&mut self) -> bool {
        let now = Instant::now();
        const THRESHOLD: Duration = Duration::from_millis(500);

        if let Some(last) = self.last_ctrl_c {
            if now.duration_since(last) < THRESHOLD {
                self.should_quit = true;
                return true;
            }
        }

        self.last_ctrl_c = Some(now);

        if self.model.streaming.is_active {
            let _ = self.input_tx.try_send("__CANCEL__".to_string());
        }

        false
    }

    async fn handle_app_event(&mut self, event: &AppEvent) -> Result<()> {
        match event {
            AppEvent::Model(kernel::event::ModelEvent::Chunk { content, .. }) => {
                match content {
                    kernel::event::ContentChunk::Text(text) => {
                        if !self.model.streaming.is_active {
                            self.model.start_streaming();
                        }
                        self.model.append_stream_content(text);
                    }
                    kernel::event::ContentChunk::Thinking { thinking, .. } => {
                        if !self.model.streaming.is_active {
                            self.model.start_streaming();
                        }
                        self.model.append_stream_thinking(thinking);
                    }
                    _ => {}
                }
            }
            AppEvent::Model(kernel::event::ModelEvent::Complete { .. }) => {
                if self.model.streaming.is_active {
                    let (content, thinking) = self.model.stop_streaming();
                    let thinking_opt = if thinking.is_empty() { None } else { Some(thinking) };
                    self.model.add_assistant_message(content, thinking_opt);
                }
            }
            AppEvent::Model(kernel::event::ModelEvent::Error { error, .. }) => {
                if self.model.streaming.is_active {
                    self.model.stop_streaming();
                }
                self.model.add_system_message(format!("Error: {error}"));
            }
            AppEvent::Tool(kernel::event::ToolEvent::Started { tool_name, .. }) => {
                self.model.add_system_message(format!("Running: {tool_name}"));
            }
            AppEvent::Tool(kernel::event::ToolEvent::Output { output, .. }) => {
                self.model.add_system_message(output.clone());
            }
            AppEvent::Tool(kernel::event::ToolEvent::Error { error, .. }) => {
                self.model.add_system_message(format!("Tool error: {error}"));
            }
            _ => {}
        }
        Ok(())
    }

    // Input operations
    fn insert_char(&mut self, c: char) {
        self.input_buffer.insert(self.cursor_pos, c);
        self.cursor_pos += c.len_utf8();
    }

    fn backspace(&mut self) {
        if self.cursor_pos > 0 {
            // Find the start of the previous UTF-8 character
            let mut idx = self.cursor_pos - 1;
            while idx > 0 && !self.input_buffer.is_char_boundary(idx) {
                idx -= 1;
            }
            self.input_buffer.drain(idx..self.cursor_pos);
            self.cursor_pos = idx;
        }
    }

    fn delete_char(&mut self) {
        if self.cursor_pos < self.input_buffer.len() {
            // Find the end of the current UTF-8 character
            let mut idx = self.cursor_pos + 1;
            while idx < self.input_buffer.len() && !self.input_buffer.is_char_boundary(idx) {
                idx += 1;
            }
            self.input_buffer.drain(self.cursor_pos..idx);
        }
    }

    fn move_left(&mut self) {
        if self.cursor_pos > 0 {
            // Move to the start of the previous UTF-8 character
            let mut idx = self.cursor_pos - 1;
            while idx > 0 && !self.input_buffer.is_char_boundary(idx) {
                idx -= 1;
            }
            self.cursor_pos = idx;
        }
    }

    fn move_right(&mut self) {
        if self.cursor_pos < self.input_buffer.len() {
            // Skip current character bytes to reach next character boundary
            let mut idx = self.cursor_pos + 1;
            while idx < self.input_buffer.len() && !self.input_buffer.is_char_boundary(idx) {
                idx += 1;
            }
            self.cursor_pos = idx;
        }
    }

    const fn move_to_start(&mut self) {
        self.cursor_pos = 0;
    }

    const fn move_to_end(&mut self) {
        self.cursor_pos = self.input_buffer.len();
    }

    fn delete_word(&mut self) {
        if self.cursor_pos == 0 {
            return;
        }
        let end = self.cursor_pos;

        // Helper to get previous character
        let prev_char = |pos: usize| -> Option<char> {
            if pos == 0 {
                return None;
            }
            let mut idx = pos - 1;
            while idx > 0 && !self.input_buffer.is_char_boundary(idx) {
                idx -= 1;
            }
            self.input_buffer[idx..pos].chars().next()
        };

        // Helper to move to previous character boundary
        let prev_boundary = |pos: usize| -> usize {
            if pos == 0 {
                return 0;
            }
            let mut idx = pos - 1;
            while idx > 0 && !self.input_buffer.is_char_boundary(idx) {
                idx -= 1;
            }
            idx
        };

        // Skip whitespace
        while self.cursor_pos > 0 && prev_char(self.cursor_pos).is_some_and(|c| c.is_whitespace()) {
            self.cursor_pos = prev_boundary(self.cursor_pos);
        }
        // Delete word chars
        while self.cursor_pos > 0 && prev_char(self.cursor_pos).is_some_and(|c| !c.is_whitespace()) {
            self.cursor_pos = prev_boundary(self.cursor_pos);
        }
        self.input_buffer.drain(self.cursor_pos..end);
    }

    fn delete_to_start(&mut self) {
        self.input_buffer.drain(0..self.cursor_pos);
        self.cursor_pos = 0;
    }

    fn delete_to_end(&mut self) {
        self.input_buffer.truncate(self.cursor_pos);
    }

    // Scroll operations
    const fn scroll_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(1);
    }

    const fn scroll_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(1);
    }

    const fn scroll_page_up(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_add(self.viewport_height / 2);
    }

    const fn scroll_page_down(&mut self) {
        self.scroll_offset = self.scroll_offset.saturating_sub(self.viewport_height / 2);
    }

    fn toggle_fold(&mut self) {
        if let Some(last) = self.model.messages.last_mut() {
            if last.thinking.is_some() {
                last.thinking_folded = !last.thinking_folded;
            }
        }
    }
}

