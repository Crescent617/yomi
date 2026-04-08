//! TUI Realm Application
//!
//! Main application using tuirealm framework for component-based TUI.

use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tuirealm::{
    application::PollStrategy,
    ratatui::layout::{Constraint, Direction, Layout},
    terminal::{CrosstermTerminalAdapter, TerminalBridge},
    Application, AttrValue, Attribute, EventListenerCfg, Sub, SubClause, SubEventClause, Update,
};

use kernel::event::Event as AppEvent;

use crate::{
    components::{ChatViewComponent, InputComponent, StatusBarComponent},
    id::Id,
    msg::{Msg, UserEvent},
};

/// TUI Model holding application state
pub struct Model {
    /// Application
    pub app: Application<Id, Msg, UserEvent>,
    /// Indicates that the application must quit
    pub quit: bool,
    /// Tells whether to redraw interface
    pub redraw: bool,
    /// Used to draw to terminal
    pub terminal: TerminalBridge<CrosstermTerminalAdapter>,
    /// Channel to receive events from kernel
    pub event_rx: mpsc::Receiver<AppEvent>,
    /// Channel to send input to kernel
    pub input_tx: mpsc::Sender<String>,
    /// Channel to send cancel requests
    pub cancel_tx: mpsc::Sender<()>,
    /// Current assistant response content (for adding to history when complete)
    current_content: String,
    /// Current assistant thinking (for adding to history when complete)
    current_thinking: String,
    /// When thinking started (for calculating elapsed time)
    thinking_start_time: Option<Instant>,
    /// Whether we're currently streaming (showing streaming component)
    is_streaming: bool,
}

impl Model {
    pub fn new(
        event_rx: mpsc::Receiver<AppEvent>,
        input_tx: mpsc::Sender<String>,
        cancel_tx: mpsc::Sender<()>,
    ) -> Result<Self> {
        let terminal = TerminalBridge::init_crossterm()?;
        let app = Self::init_app()?;

        Ok(Self {
            app,
            quit: false,
            redraw: true,
            terminal,
            event_rx,
            input_tx,
            cancel_tx,
            current_content: String::new(),
            current_thinking: String::new(),
            thinking_start_time: None,
            is_streaming: false,
        })
    }

    pub fn view(&mut self) {
        let _ = self.terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints(
                    [
                        Constraint::Min(3),    // Main content area
                        Constraint::Length(1), // Status bar
                        Constraint::Length(3), // Input area
                    ]
                    .as_ref(),
                )
                .split(f.area());

            // Always show ChatView (unified history + streaming)
            self.app.view(&Id::ChatView, f, chunks[0]);
            // Status bar shows streaming progress
            self.app.view(&Id::StatusBar, f, chunks[1]);
            // InputBox renders last and sets cursor position
            self.app.view(&Id::InputBox, f, chunks[2]);
        });
    }

    fn init_app() -> Result<Application<Id, Msg, UserEvent>> {
        let mut app = Application::init(
            EventListenerCfg::default()
                .crossterm_input_listener(Duration::from_millis(10), 10)
                .poll_timeout(Duration::from_millis(10))
                .tick_interval(Duration::from_millis(100)),
        );

        // Mount unified chat view component
        app.mount(
            Id::ChatView,
            Box::new(ChatViewComponent::new()),
            vec![Sub::new(SubEventClause::Tick, SubClause::Always)],
        )?;

        // Mount status bar component
        app.mount(
            Id::StatusBar,
            Box::new(StatusBarComponent::new()),
            vec![Sub::new(SubEventClause::Tick, SubClause::Always)],
        )?;

        // Mount input component
        app.mount(Id::InputBox, Box::new(InputComponent::new()), vec![])?;

        // Set focus to input box
        app.active(&Id::InputBox)?;

        Ok(app)
    }

    /// Process events from kernel
    pub fn process_kernel_events(&mut self) -> Result<()> {
        while let Ok(event) = self.event_rx.try_recv() {
            match event {
                AppEvent::Model(kernel::event::ModelEvent::Chunk { content, .. }) => {
                    self.is_streaming = true;
                    match content {
                        kernel::event::ContentChunk::Text(text) => {
                            self.current_content.push_str(&text);
                            self.app.attr(
                                &Id::ChatView,
                                Attribute::Custom("append_content"),
                                AttrValue::String(text.clone()),
                            )?;
                            // Update status bar with token counts
                            let content_tokens = self.current_content.len() / 4;
                            let thinking_tokens = self.current_thinking.len() / 4;
                            self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom("set_tokens"),
                                AttrValue::String(format!("{content_tokens}, {thinking_tokens}")),
                            )?;
                        }
                        kernel::event::ContentChunk::Thinking { thinking, .. } => {
                            // Track thinking start time
                            if self.thinking_start_time.is_none() {
                                self.thinking_start_time = Some(Instant::now());
                            }
                            self.current_thinking.push_str(&thinking);
                            // Show thinking in streaming view
                            self.app.attr(
                                &Id::ChatView,
                                Attribute::Custom("append_thinking"),
                                AttrValue::String(thinking.clone()),
                            )?;
                            // Update status bar with token counts
                            let content_tokens = self.current_content.len() / 4;
                            let thinking_tokens = self.current_thinking.len() / 4;
                            self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom("set_tokens"),
                                AttrValue::String(format!("{content_tokens}, {thinking_tokens}")),
                            )?;
                        }
                        _ => {}
                    }
                    self.redraw = true;
                }
                AppEvent::Model(kernel::event::ModelEvent::Complete { .. }) => {
                    self.is_streaming = false;

                    // Stop status bar
                    self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom("stop_streaming"),
                        AttrValue::Flag(true),
                    )?;

                    // Add completed assistant message to history
                    // Save if there's either content or thinking
                    if !self.current_content.is_empty() || !self.current_thinking.is_empty() {
                        // Calculate thinking elapsed time
                        let elapsed_ms = self
                            .thinking_start_time
                            .map(|start| start.elapsed().as_millis() as u64);

                        // Combine content, thinking and elapsed with null separator
                        let combined = if self.current_thinking.is_empty() {
                            if let Some(ms) = elapsed_ms {
                                format!("{}\x00\x00{}", self.current_content, ms)
                            } else {
                                self.current_content.clone()
                            }
                        } else {
                            format!(
                                "{}\x00{}\x00{}",
                                self.current_content,
                                self.current_thinking,
                                elapsed_ms.unwrap_or(0)
                            )
                        };
                        self.app.attr(
                            &Id::ChatView,
                            Attribute::Custom("add_assistant_with_thinking"),
                            AttrValue::String(combined),
                        )?;
                    }
                    // Clear tracking
                    self.current_content.clear();
                    self.current_thinking.clear();
                    self.thinking_start_time = None;

                    // Clear streaming message to avoid duplication with history
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("clear_streaming"),
                        AttrValue::Flag(true),
                    )?;

                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("stop_streaming"),
                        AttrValue::Flag(true),
                    )?;
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("scroll_to_bottom"),
                        AttrValue::Flag(true),
                    )?;
                    self.redraw = true;
                }
                AppEvent::Model(kernel::event::ModelEvent::Request { .. }) => {
                    // Clear previous streaming content
                    self.is_streaming = true;
                    self.current_content.clear();
                    self.current_thinking.clear();
                    self.thinking_start_time = None;
                    // Note: Status bar already started in InputSubmit
                    // Start ChatView streaming
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("start_streaming"),
                        AttrValue::Flag(true),
                    )?;
                    self.redraw = true;
                }
                AppEvent::Tool(kernel::event::ToolEvent::Started {
                    tool_id,
                    tool_name,
                    arguments,
                    ..
                }) => {
                    // Show tool execution start in chat view
                    let args_str = arguments.unwrap_or_default();
                    let combined = format!("{tool_id}\x00{tool_name}\x00{args_str}");
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("start_tool"),
                        AttrValue::String(combined),
                    )?;
                    self.redraw = true;
                }
                AppEvent::Tool(kernel::event::ToolEvent::Output {
                    tool_id,
                    output,
                    elapsed_ms,
                    ..
                }) => {
                    // Show tool output in chat view
                    let combined = format!("{tool_id}\x00{output}\x00{elapsed_ms}");
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("complete_tool"),
                        AttrValue::String(combined),
                    )?;
                    self.redraw = true;
                }
                AppEvent::Tool(kernel::event::ToolEvent::Error {
                    tool_id,
                    error,
                    elapsed_ms,
                    ..
                }) => {
                    // Show tool error in chat view
                    let combined = format!("{tool_id}\x00{error}\x00{elapsed_ms}");
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("fail_tool"),
                        AttrValue::String(combined),
                    )?;
                    self.redraw = true;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Run the main loop
    pub async fn run(mut self) -> Result<()> {
        // Enter alternate screen
        self.terminal.enter_alternate_screen()?;
        self.terminal.enable_raw_mode()?;

        // Hide cursor by default (will be shown by InputComponent when needed)
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide)?;

        let result = self.run_loop().await;

        // Cleanup
        self.terminal.leave_alternate_screen()?;
        self.terminal.disable_raw_mode()?;

        result
    }

    async fn run_loop(&mut self) -> Result<()> {
        // Enable mouse capture
        self.terminal.enable_mouse_capture()?;

        while !self.quit {
            // Process kernel events
            self.process_kernel_events()?;

            // Tick the application
            match self.app.tick(PollStrategy::Once) {
                Ok(messages) if !messages.is_empty() => {
                    self.redraw = true;
                    for msg in messages {
                        let mut msg = Some(msg);
                        while msg.is_some() {
                            msg = self.update(msg);
                        }
                    }
                }
                _ => {}
            }

            // Redraw if needed
            if self.redraw {
                self.view();
                self.redraw = false;
            }

            // Small yield to allow tokio to process other tasks
            tokio::task::yield_now().await;
        }

        // Disable mouse capture before exit
        self.terminal.disable_mouse_capture()?;

        Ok(())
    }
}

impl Update<Msg> for Model {
    fn update(&mut self, msg: Option<Msg>) -> Option<Msg> {
        if let Some(msg) = msg {
            self.redraw = true;

            match msg {
                Msg::Quit => {
                    self.quit = true;
                    None
                }
                Msg::InputSubmit(content) => {
                    // Add user message to chat view
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("add_user_message"),
                        AttrValue::String(content.clone()),
                    );
                    // Scroll to bottom after adding user message
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("scroll_to_bottom"),
                        AttrValue::Flag(true),
                    );
                    // Start streaming status immediately when sending request
                    let _ = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom("start_streaming"),
                        AttrValue::Flag(true),
                    );
                    // Send to kernel
                    let _ = self.input_tx.try_send(content);
                    None
                }
                Msg::ScrollUp => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("scroll_up"),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::ScrollDown => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("scroll_down"),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::ToggleThinking => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("toggle_thinking"),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::ToggleExpandAll => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("toggle_expand_all"),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::CancelRequest => {
                    let _ = self.cancel_tx.try_send(());
                    None
                }
                Msg::Redraw => {
                    self.redraw = true;
                    None
                }
                _ => None,
            }
        } else {
            None
        }
    }
}

/// Run the TUI application
pub async fn run_tui(
    event_rx: mpsc::Receiver<AppEvent>,
    input_tx: mpsc::Sender<String>,
    cancel_tx: mpsc::Sender<()>,
) -> Result<()> {
    let model = Model::new(event_rx, input_tx, cancel_tx)?;
    model.run().await
}
