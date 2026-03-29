use crate::theme::Theme;
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nekoclaw_core::bus::EventBus;
use nekoclaw_core::event::{Event as AppEvent, ContentChunk, AgentEvent, ModelEvent, ToolEvent, UserEvent};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    text::Text,
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame, Terminal,
};
use std::io;
use tokio::sync::broadcast;

pub struct App {
    input: String,
    events: Vec<DisplayEvent>,
    theme: Theme,
    event_rx: broadcast::Receiver<AppEvent>,
    input_tx: tokio::sync::mpsc::Sender<String>,
    should_quit: bool,
}

#[derive(Debug, Clone)]
enum DisplayEvent {
    User(String),
    Assistant { text: String, thinking: Option<String> },
    Thinking(String),
    Tool { name: String, output: String },
    System(String),
    Error(String),
}

impl App {
    pub fn new(
        event_bus: &EventBus,
        input_tx: tokio::sync::mpsc::Sender<String>,
    ) -> Self {
        Self {
            input: String::new(),
            events: Vec::new(),
            theme: Theme::default(),
            event_rx: event_bus.subscribe(),
            input_tx,
            should_quit: false,
        }
    }

    pub async fn run(&mut self) -> Result<()> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        let result = self.run_loop(&mut terminal).await;
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;
        result
    }

    async fn run_loop<B: Backend>(
        &mut self, terminal: &mut Terminal<B>
    ) -> Result<()> {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(50));
        while !self.should_quit {
            terminal.draw(|f| self.draw(f))?;
            tokio::select! {
                _ = interval.tick() => {}
                Ok(event) = self.event_rx.recv() => {
                    self.handle_app_event(event);
                }
                _ = tokio::task::spawn_blocking(|| {
                    event::poll(std::time::Duration::from_millis(10))
                }) => {
                    if let Ok(true) = event::poll(std::time::Duration::from_millis(0)) {
                        if let Ok(Event::Key(key)) = event::read() {
                            if key.kind == KeyEventKind::Press {
                                self.handle_key(key.code).await?;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_app_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::User(UserEvent::Message { content }) => {
                self.events.push(DisplayEvent::User(content));
            }
            AppEvent::Model(ModelEvent::Chunk { content: ContentChunk::Text(text), .. }) => {
                if let Some(DisplayEvent::Assistant { text: ref mut existing, .. }) = self.events.last_mut() {
                    existing.push_str(&text);
                } else {
                    self.events.push(DisplayEvent::Assistant { text, thinking: None });
                }
            }
            AppEvent::Model(ModelEvent::Chunk { content: ContentChunk::Thinking { thinking, .. }, .. }) => {
                if let Some(DisplayEvent::Assistant { thinking: ref mut existing, .. }) = self.events.last_mut() {
                    if let Some(ref mut t) = existing {
                        t.push_str(&thinking);
                    } else {
                        *existing = Some(thinking);
                    }
                } else {
                    self.events.push(DisplayEvent::Thinking(thinking));
                }
            }
            AppEvent::Model(ModelEvent::Chunk { content: ContentChunk::RedactedThinking, .. }) => {
                self.events.push(DisplayEvent::Thinking("[Thinking redacted]".to_string()));
            }
            AppEvent::Tool(ToolEvent::Output { tool_id, output, .. }) => {
                self.events.push(DisplayEvent::Tool { name: tool_id, output });
            }
            AppEvent::Tool(ToolEvent::Error { error, .. }) => {
                self.events.push(DisplayEvent::Error(error));
            }
            AppEvent::Agent(AgentEvent::Failed { error, .. }) => {
                self.events.push(DisplayEvent::Error(error));
            }
            _ => {}
        }
    }

    async fn handle_key(&mut self, key: KeyCode) -> Result<()> {
        match key {
            KeyCode::Char('q') => { self.should_quit = true; }
            KeyCode::Char('c') => { self.input.clear(); }
            KeyCode::Char(c) => { self.input.push(c); }
            KeyCode::Backspace => { self.input.pop(); }
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    let content = self.input.clone();
                    self.input.clear();
                    self.input_tx.send(content).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn draw(&self, frame: &mut Frame) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(frame.area());
        let event_items: Vec<ListItem> = self.events.iter().map(|event| {
            let (prefix, style, content): (&str, _, String) = match event {
                DisplayEvent::User(text) => ("You: ", self.theme.user(), text.clone()),
                DisplayEvent::Assistant { text, thinking } => {
                    let display = if let Some(t) = thinking {
                        format!("[Thinking] {}\n\n{}", t, text)
                    } else {
                        text.clone()
                    };
                    ("AI: ", self.theme.assistant(), display)
                }
                DisplayEvent::Thinking(t) => ("Thinking: ", self.theme.thinking(), t.clone()),
                DisplayEvent::Tool { name, output } => {
                    let prefix = format!("Tool {}: ", name);
                    ("", self.theme.system(), format!("{}{}", prefix, output))
                }
                DisplayEvent::System(text) => ("System: ", self.theme.system(), text.clone()),
                DisplayEvent::Error(text) => ("Error: ", self.theme.error(), text.clone()),
            };
            let text = Text::styled(format!("{}{}", prefix, content), style);
            ListItem::new(text)
        }).collect();
        let events_widget = List::new(event_items)
            .block(Block::default().borders(Borders::ALL).title("Nekoclaw").border_style(self.theme.border()))
            .style(self.theme.base());
        frame.render_widget(events_widget, chunks[0]);
        let input_widget = Paragraph::new(self.input.as_str())
            .block(Block::default().borders(Borders::ALL).title("Input (Enter to send, q to quit)").border_style(self.theme.border()))
            .style(self.theme.base());
        frame.render_widget(input_widget, chunks[1]);
    }
}
