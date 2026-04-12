//! TUI Realm Application
//!
//! Main application using tuirealm framework for component-based TUI.

use anyhow::Result;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tuirealm::SubEventClause;
use tuirealm::{
    application::PollStrategy,
    ratatui::layout::{Constraint, Direction, Layout},
    terminal::{CrosstermTerminalAdapter, TerminalBridge},
    Application, AttrValue, Attribute, EventListenerCfg, Sub, SubClause, Update,
};

use kernel::event::Event as AppEvent;

use crate::{
    components::{ChatViewComponent, InfoBarComponent, InputComponent, StatusBarComponent},
    id::Id,
    msg::{Msg, UserEvent},
};

/// Application mode - single source of truth for UI mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    #[default]
    Normal,
    Browse,
}

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
    /// Application mode - single source of truth
    mode: AppMode,
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
            mode: AppMode::Normal,
        })
    }

    /// Initialize banner with real data (called once at startup)
    pub fn init_banner(&mut self, working_dir: String, skills: Vec<String>) -> Result<()> {
        self.update_banner(working_dir, skills)
    }

    /// Update banner data in `ChatView`
    pub fn update_banner(&mut self, working_dir: String, skills: Vec<String>) -> Result<()> {
        use crate::components::BannerData;
        let banner = BannerData::new(working_dir, skills);
        // Serialize banner data: working_dir|skill1,skill2,...
        let banner_str = format!("{}|{}", banner.working_dir, banner.skills.join(","));
        self.app.attr(
            &Id::ChatView,
            Attribute::Custom("set_banner"),
            AttrValue::String(banner_str),
        )?;
        Ok(())
    }

    /// Calculate input box height based on content (3-5 lines, including borders)
    fn calculate_input_height(&self) -> u16 {
        // Content lines (1-3), plus 2 for borders = total 3-5
        let content_lines = if let Ok(tuirealm::State::One(tuirealm::StateValue::String(content))) =
            self.app.state(&Id::InputBox)
        {
            // Count newlines + 1 to handle trailing newlines correctly
            // "hello\nworld" -> 1 newline + 1 = 2 lines
            // "hello\n" -> 1 newline + 1 = 2 lines (lines() would return 1)
            let line_count = content.matches('\n').count() + 1;
            (line_count.max(1) as u16).min(3)
        } else {
            1
        };
        content_lines + 2 // Add 2 for top/bottom borders
    }

    /// Save partial content (content and thinking) to chat history
    fn save_partial_content(&mut self) -> anyhow::Result<()> {
        if !self.current_content.is_empty() || !self.current_thinking.is_empty() {
            let elapsed_ms = self
                .thinking_start_time
                .map(|start| start.elapsed().as_millis() as u64);

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
        Ok(())
    }

    /// Clear streaming state (content, thinking, start time)
    fn clear_streaming_state(&mut self) {
        self.current_content.clear();
        self.current_thinking.clear();
        self.thinking_start_time = None;
    }

    pub fn view(&mut self) {
        let input_height = self.calculate_input_height();

        let _ = self.terminal.draw(|f| {
            if self.mode == AppMode::Browse {
                // Browse mode: full screen chat view with status bar
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(
                        [
                            Constraint::Min(3),    // Main content area (includes banner)
                            Constraint::Length(1), // Status bar
                        ]
                        .as_ref(),
                    )
                    .split(f.area());

                self.app.view(&Id::ChatView, f, chunks[0]);
                // Status bar shows current mode (vim-style)
                self.app.view(&Id::StatusBar, f, chunks[1]);
            } else {
                // Normal mode: show all components
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints(
                        [
                            Constraint::Min(3),               // Main content area (chat with banner)
                            Constraint::Length(1),            // Info bar (tokens/streaming)
                            Constraint::Length(input_height), // Input area
                            Constraint::Length(1),            // Status bar
                        ]
                        .as_ref(),
                    )
                    .split(f.area());

                // ChatView includes banner at top (scrolls with content)
                self.app.view(&Id::ChatView, f, chunks[0]);
                // Info bar shows streaming progress
                self.app.view(&Id::InfoBar, f, chunks[1]);
                // InputBox renders last and sets cursor position
                self.app.view(&Id::InputBox, f, chunks[2]);
                // Status bar shows current mode (vim-style)
                self.app.view(&Id::StatusBar, f, chunks[3]);
            }
        });
    }

    fn init_app() -> Result<Application<Id, Msg, UserEvent>> {
        let mut app = Application::init(
            EventListenerCfg::default()
                .crossterm_input_listener(Duration::from_millis(10), 10)
                .poll_timeout(Duration::from_millis(10))
                .tick_interval(Duration::from_millis(100)),
        );

        // Mount unified chat view component (includes scrollable banner)
        app.mount(
            Id::ChatView,
            Box::new(ChatViewComponent::new()),
            vec![Sub::new(SubEventClause::Tick, SubClause::Always)],
        )?;

        // Mount info bar component (token/streaming status)
        app.mount(
            Id::InfoBar,
            Box::new(InfoBarComponent::new()),
            vec![
                Sub::new(SubEventClause::Tick, SubClause::Always),
                Sub::new(SubEventClause::Any, SubClause::Always),
            ],
        )?;

        // Mount input component
        app.mount(Id::InputBox, Box::new(InputComponent::new()), vec![])?;

        // Mount status bar component (vim-style mode indicator at bottom)
        app.mount(
            Id::StatusBar,
            Box::new(StatusBarComponent::new()),
            vec![Sub::new(SubEventClause::Tick, SubClause::Always)],
        )?;

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
                            // Update InfoBar with content for token counting
                            self.app.attr(
                                &Id::InfoBar,
                                Attribute::Custom("append_content"),
                                AttrValue::String(text),
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
                            // Update InfoBar with thinking for token counting
                            self.app.attr(
                                &Id::InfoBar,
                                Attribute::Custom("append_thinking"),
                                AttrValue::String(thinking),
                            )?;
                        }
                        kernel::event::ContentChunk::RedactedThinking => {}
                    }
                    self.redraw = true;
                }
                AppEvent::Model(kernel::event::ModelEvent::Complete { .. }) => {
                    self.is_streaming = false;

                    // Stop status bar
                    self.app.attr(
                        &Id::InfoBar,
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
                        Attribute::Custom("cancel_streaming"),
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
                AppEvent::Model(kernel::event::ModelEvent::Compacting { active, .. }) => {
                    // Show/hide compacting status in InfoBar
                    let attr = if active {
                        Attribute::Custom("start_compacting")
                    } else {
                        Attribute::Custom("stop_compacting")
                    };
                    self.app.attr(&Id::InfoBar, attr, AttrValue::Flag(active))?;
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
                AppEvent::Agent(kernel::event::AgentEvent::Cancelled { .. }) => {
                    self.is_streaming = false;

                    // Save partial content and clear state
                    let _ = self.save_partial_content();
                    self.clear_streaming_state();

                    // Cancel streaming in ChatView and InfoBar
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("cancel_streaming"),
                        AttrValue::Flag(true),
                    )?;
                    self.app.attr(
                        &Id::InfoBar,
                        Attribute::Custom("cancel_streaming"),
                        AttrValue::Flag(true),
                    )?;
                    self.redraw = true;
                }
                AppEvent::Agent(kernel::event::AgentEvent::Failed { error, .. }) => {
                    self.is_streaming = false;

                    // Stop status bar
                    self.app.attr(
                        &Id::InfoBar,
                        Attribute::Custom("stop_streaming"),
                        AttrValue::Flag(true),
                    )?;

                    // Save partial content and clear state
                    let _ = self.save_partial_content();
                    self.clear_streaming_state();

                    // Clear streaming (both clear_streaming and cancel_streaming are needed for proper cleanup)
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("clear_streaming"),
                        AttrValue::Flag(true),
                    )?;
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("cancel_streaming"),
                        AttrValue::Flag(true),
                    )?;

                    // Display error message to user
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("add_error_message"),
                        AttrValue::String(format!("Agent error: {error}")),
                    )?;

                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("scroll_to_bottom"),
                        AttrValue::Flag(true),
                    )?;
                    self.redraw = true;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Run the main loop
    #[allow(clippy::future_not_send)]
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

    #[allow(clippy::future_not_send)]
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
                // Ignore input-related messages in Browse mode
                Msg::InputSubmit(content) => {
                    if self.mode == AppMode::Browse {
                        return None;
                    }
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
                        &Id::InfoBar,
                        Attribute::Custom("start_streaming"),
                        AttrValue::Flag(true),
                    );
                    // Send to kernel
                    let _ = self.input_tx.try_send(content);
                    None
                }
                // Scrolling - works in both modes
                Msg::ScrollUp => {
                    let amount = if self.mode == AppMode::Browse { 1 } else { 3 };
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("scroll_up"),
                        AttrValue::Number(amount as isize),
                    );
                    None
                }
                Msg::ScrollDown => {
                    let amount = if self.mode == AppMode::Browse { 1 } else { 3 };
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("scroll_down"),
                        AttrValue::Number(amount as isize),
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
                Msg::InputChanged(_) => {
                    // Ignore input changes in Browse mode
                    if self.mode == AppMode::Browse {
                        return None;
                    }
                    // Note: InputChanged is sent by InputComponent but doesn't need special handling here
                    // It's mainly used for tracking input state if needed
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
                Msg::ShowStatusMessage(msg, duration_ms) => {
                    // Format: "duration_ms|message"
                    let value = format!("{duration_ms}|{msg}");
                    let _ = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom("show_message"),
                        AttrValue::String(value),
                    );
                    None
                }
                // Mode switching
                Msg::ToggleBrowseMode => {
                    match self.mode {
                        AppMode::Normal => {
                            // Enter browse mode
                            self.mode = AppMode::Browse;
                            // Expand all blocks in browse mode
                            let _ = self.app.attr(
                                &Id::ChatView,
                                Attribute::Custom("expand_all"),
                                AttrValue::Flag(true),
                            );
                            // Update status bar to show BROWSE mode
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom("set_mode"),
                                AttrValue::Number(1),
                            );
                            // Update input box mode so it knows to use browse shortcuts
                            let _ = self.app.attr(
                                &Id::InputBox,
                                Attribute::Custom("mode"),
                                AttrValue::Number(1),
                            );
                            // Show help message for browse mode shortcuts (0 = no auto-clear)
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom("show_message"),
                                AttrValue::String(
                                    "0|C-o toggle, j/k scroll, q exit browse".to_string(),
                                ),
                            );
                        }
                        AppMode::Browse => {
                            // Exit browse mode
                            self.mode = AppMode::Normal;
                            // Collapse all blocks
                            let _ = self.app.attr(
                                &Id::ChatView,
                                Attribute::Custom("collapse_all"),
                                AttrValue::Flag(true),
                            );
                            // Update status bar to show NORMAL mode
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom("set_mode"),
                                AttrValue::Number(0),
                            );
                            // Update input box mode so it uses normal text input
                            let _ = self.app.attr(
                                &Id::InputBox,
                                Attribute::Custom("mode"),
                                AttrValue::Number(0),
                            );
                            // Clear any status message
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom("clear_message"),
                                AttrValue::Flag(true),
                            );
                        }
                    }
                    None
                }
                Msg::PageUp => {
                    let height = self.terminal.raw().size().map_or(20, |s| s.height as usize);
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("page_up"),
                        AttrValue::Number(height as isize),
                    );
                    None
                }
                Msg::PageDown => {
                    let height = self.terminal.raw().size().map_or(20, |s| s.height as usize);
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("page_down"),
                        AttrValue::Number(height as isize),
                    );
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
#[allow(clippy::future_not_send)]
pub async fn run_tui(
    event_rx: mpsc::Receiver<AppEvent>,
    input_tx: mpsc::Sender<String>,
    cancel_tx: mpsc::Sender<()>,
    working_dir: String,
    skills: Vec<String>,
) -> Result<()> {
    let mut model = Model::new(event_rx, input_tx, cancel_tx)?;
    model.init_banner(working_dir, skills)?;
    model.run().await
}
