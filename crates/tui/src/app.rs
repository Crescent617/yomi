//! TUI Realm Application
//!
//! Main application using tuirealm framework for component-based TUI.

use anyhow::Result;

/// Result type returned by TUI
pub struct TuiResult {
    /// Input history entries collected during this session
    pub input_history: Vec<String>,
    /// Whether to create a new session after exiting
    pub should_create_new_session: bool,
}
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tuirealm::SubEventClause;
use tuirealm::{
    application::PollStrategy,
    ratatui::layout::{Constraint, Direction, Layout},
    terminal::{CrosstermTerminalAdapter, TerminalBridge},
    Application, AttrValue, Attribute, EventListenerCfg, Sub, SubClause, Update,
};
use unicode_width::UnicodeWidthStr;

use kernel::event::{Event as AppEvent, PermissionCommand};
use kernel::permissions::Level;
use kernel::types::{ContentBlock, Message};

use crate::{
    components::{
        ChatViewComponent, InfoBarComponent, InputComponent, SelectDialogComponent,
        StatusBarComponent,
    },
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
/// Application state flags grouped to reduce struct bool count
#[derive(Debug, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct AppState {
    /// Indicates that the application must quit
    pub quit: bool,
    /// Tells whether to redraw interface
    pub should_redraw: bool,
    /// Whether we're currently streaming (showing streaming component)
    pub is_streaming: bool,
    /// Flag to indicate if a new session should be created on exit
    pub should_create_new_session: bool,
}

pub struct Model {
    /// Application
    pub app: Application<Id, Msg, UserEvent>,
    /// Application state flags
    pub state: AppState,
    pub terminal: TerminalBridge<CrosstermTerminalAdapter>,
    /// Channel to receive events from kernel
    pub event_rx: mpsc::Receiver<AppEvent>,
    /// Channel to send input to kernel (supports multi-modal content blocks)
    pub input_tx: mpsc::Sender<Vec<ContentBlock>>,
    /// Channel to send cancel requests
    pub cancel_tx: mpsc::Sender<()>,
    /// Channel to send permission commands (responses and level changes)
    pub permission_tx: mpsc::Sender<PermissionCommand>,
    /// Current assistant response content (for adding to history when complete)
    current_content: String,
    /// Current assistant thinking (for adding to history when complete)
    current_thinking: String,
    /// When thinking started (for calculating elapsed time)
    thinking_start_time: Option<Instant>,
    /// Whether we're currently streaming (showing streaming component)
    /// Application mode - single source of truth
    mode: AppMode,
    /// Pending permission request (`req_id`) waiting for user confirmation
    pending_permission: Option<String>,
    /// Input history for the current working directory (loaded + new)
    input_history: Vec<String>,
    /// Initial history length (to identify new entries on exit)
    initial_history_len: usize,
    /// Working directory (kept for future use)
    #[allow(dead_code)]
    working_dir: std::path::PathBuf,
    /// Session messages to display on startup (for resumed sessions)
    session_messages: Vec<Message>,
    /// Permission level for displaying YOLO mode indicator
    permission_level: Level,
}

impl Model {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_rx: mpsc::Receiver<AppEvent>,
        input_tx: mpsc::Sender<Vec<ContentBlock>>,
        cancel_tx: mpsc::Sender<()>,
        permission_tx: mpsc::Sender<PermissionCommand>,
        input_history: Vec<String>,
        working_dir: std::path::PathBuf,
        session_messages: Vec<Message>,
        permission_level: Level,
    ) -> Result<Self> {
        let terminal = TerminalBridge::init_crossterm()?;
        let app = Self::init_app()?;

        Ok(Self {
            app,
            state: AppState {
                quit: false,
                should_redraw: true,
                is_streaming: false,
                should_create_new_session: false,
            },
            terminal,
            event_rx,
            input_tx,
            cancel_tx,
            permission_tx,
            current_content: String::new(),
            current_thinking: String::new(),
            thinking_start_time: None,
            mode: AppMode::Normal,
            pending_permission: None,
            initial_history_len: input_history.len(),
            input_history,
            working_dir,
            session_messages,
            permission_level,
        })
    }

    /// Get new history entries collected during this session
    pub fn get_new_history_entries(&self) -> Vec<String> {
        self.input_history[self.initial_history_len..].to_vec()
    }

    /// Initialize input history in the `InputBox` component
    pub fn init_input_history(&mut self) -> Result<()> {
        // Serialize history to JSON string
        let history_json = serde_json::to_string(&self.input_history)?;
        self.app.attr(
            &Id::InputBox,
            Attribute::Custom("history"),
            AttrValue::String(history_json),
        )?;
        // Set working directory for file completion
        let working_dir_str = self.working_dir.to_string_lossy().to_string();
        let _ = self.app.attr(
            &Id::InputBox,
            Attribute::Custom("working_dir"),
            AttrValue::String(working_dir_str),
        );
        Ok(())
    }

    /// Display session messages in `ChatView` and calculate initial token usage for `StatusBar`
    fn init_session_messages(&mut self, context_window: u32) -> Result<()> {
        if self.session_messages.is_empty() {
            // Still initialize StatusBar with 0 tokens
            self.init_ctx_usage(0, context_window)?;
            return Ok(());
        }

        // Calculate initial token usage from messages
        let initial_tokens: u32 = self
            .session_messages
            .iter()
            .filter_map(|m| m.token_usage.map(|u| u.total_tokens))
            .next_back()
            .unwrap_or_else(|| {
                // Estimate tokens from all messages if no usage data
                use kernel::utils::tokens;
                self.session_messages
                    .iter()
                    .map(|m| tokens::estimate_tokens(&m.text_content()))
                    .sum::<usize>() as u32
            });

        // Initialize StatusBar with calculated tokens
        self.init_ctx_usage(initial_tokens, context_window)?;

        // Pass messages via Payload to avoid serialization
        let messages: Vec<kernel::types::Message> = std::mem::take(&mut self.session_messages);
        self.app.attr(
            &Id::ChatView,
            Attribute::Custom("init_history"),
            AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(messages))),
        )?;
        Ok(())
    }

    /// Initialize banner with real data (called once at startup)
    pub fn init_banner(&mut self, working_dir: String, skills: Vec<String>) -> Result<()> {
        self.update_banner(working_dir, skills)
    }

    /// Initialize status bar with permission level for YOLO mode display
    pub fn init_status_bar(&mut self) -> Result<()> {
        let level_val = match self.permission_level {
            Level::Safe => 0,
            Level::Caution => 1,
            Level::Dangerous => 2,
        };
        self.app.attr(
            &Id::StatusBar,
            Attribute::Custom("set_permission_level"),
            AttrValue::Number(level_val),
        )?;
        Ok(())
    }

    /// Initialize context window display in status bar
    pub fn init_ctx_usage(&mut self, tokens: u32, context_window: u32) -> Result<()> {
        let usage_str = format!("{tokens}\x00{context_window}");
        self.app.attr(
            &Id::StatusBar,
            Attribute::Custom("set_ctx_usage"),
            AttrValue::String(usage_str),
        )?;
        Ok(())
    }

    /// Update banner data in `ChatView`
    pub fn update_banner(&mut self, working_dir: String, skills: Vec<String>) -> Result<()> {
        use crate::components::BannerData;
        let banner = BannerData::new(working_dir, skills);
        // Serialize banner data: working_dir\x00skill1,skill2,...
        let banner_str = format!("{}\x00{}", banner.working_dir, banner.skills.join(","));
        self.app.attr(
            &Id::ChatView,
            Attribute::Custom("set_banner"),
            AttrValue::String(banner_str),
        )?;
        Ok(())
    }

    /// Calculate input box height based on content (3-10 lines, including borders)
    /// Accounts for text wrapping based on available terminal width
    fn calculate_input_height_for_content(content: &str, terminal_width: u16) -> u16 {
        // Account for borders and padding in the layout
        // Input area has left/right borders (2 chars)
        let content_width = (terminal_width.saturating_sub(2) as usize).max(1);

        // Get content and calculate visual lines
        let visual_lines = if content.is_empty() {
            1
        } else {
            // Calculate how many visual lines are needed considering wrap
            let lines: Vec<&str> = content.split('\n').collect();
            let mut total_visual_lines = 0;

            for line in lines {
                // Each line needs at least 1 visual line
                // Calculate how many lines it wraps to based on content width
                let line_width = line.width();
                let wrapped_lines = line_width.saturating_add(content_width).saturating_sub(1)
                    / content_width.max(1);
                total_visual_lines += wrapped_lines.max(1);
            }

            // Clamp between 1 and 8 content lines (to prevent excessive growth)
            total_visual_lines.clamp(1, 8)
        };

        visual_lines as u16 + 2 // Add 2 for top/bottom borders
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
        // Pre-fetch content to calculate height without borrowing self in closure
        let input_content = if let Ok(tuirealm::State::One(tuirealm::StateValue::String(content))) =
            self.app.state(&Id::InputBox)
        {
            content
        } else {
            String::new()
        };

        let _ = self.terminal.draw(|f| {
            // Calculate input height inside draw closure to access terminal area
            let input_height =
                Self::calculate_input_height_for_content(&input_content, f.area().width);

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

            // Render dialog on top if active (uses full screen for centering)
            self.app.view(&Id::Dialog, f, f.area());
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

        // Mount select dialog component (hidden by default, for permission confirmation)
        app.mount(
            Id::Dialog,
            Box::new(SelectDialogComponent::new("Dialog")),
            vec![Sub::new(SubEventClause::Any, SubClause::Always)],
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
                    self.state.is_streaming = true;
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
                    self.state.should_redraw = true;
                }
                AppEvent::Model(kernel::event::ModelEvent::Completed { .. }) => {
                    self.state.is_streaming = false;

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
                    self.state.should_redraw = true;
                }
                AppEvent::Model(kernel::event::ModelEvent::Request { .. }) => {
                    // Clear previous streaming content
                    self.state.is_streaming = true;
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
                    self.state.should_redraw = true;
                }
                AppEvent::Model(kernel::event::ModelEvent::Compacting { active, .. }) => {
                    // Show/hide compacting status in InfoBar
                    let attr = if active {
                        Attribute::Custom("start_compacting")
                    } else {
                        Attribute::Custom("stop_compacting")
                    };
                    self.app.attr(&Id::InfoBar, attr, AttrValue::Flag(active))?;
                    self.state.should_redraw = true;
                }
                AppEvent::Model(kernel::event::ModelEvent::TokenUsage {
                    total_tokens,
                    context_window,
                    ..
                }) => {
                    // Update context window usage in status bar
                    let usage_str = format!("{total_tokens}\x00{context_window}");
                    self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom("set_ctx_usage"),
                        AttrValue::String(usage_str),
                    )?;
                    self.state.should_redraw = true;
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
                    self.state.should_redraw = true;
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
                    self.state.should_redraw = true;
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
                    self.state.should_redraw = true;
                }
                AppEvent::Tool(kernel::event::ToolEvent::Progress {
                    tool_id,
                    message,
                    tokens,
                    ..
                }) => {
                    // Update tool progress in chat view
                    // Format: tool_id\x00message\x00tokens (tokens is optional)
                    let tokens_str = tokens.map(|t| t.to_string()).unwrap_or_default();
                    let combined = format!("{tool_id}\x00{message}\x00{tokens_str}");
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("update_tool_progress"),
                        AttrValue::String(combined),
                    )?;
                    self.state.should_redraw = true;
                }
                AppEvent::Agent(kernel::event::AgentEvent::Cancelled { .. }) => {
                    self.state.is_streaming = false;

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
                    self.state.should_redraw = true;
                }
                AppEvent::Agent(kernel::event::AgentEvent::Failed { error, .. }) => {
                    self.state.is_streaming = false;

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
                    self.state.should_redraw = true;
                }
                AppEvent::Agent(kernel::event::AgentEvent::PermissionRequest {
                    req_id,
                    tool_name,
                    tool_args,
                    tool_level,
                    ..
                }) => {
                    tracing::info!(
                        "TUI received PermissionRequest: {} for {}",
                        req_id,
                        tool_name
                    );
                    // Store the pending permission request
                    self.pending_permission = Some(req_id.clone());

                    // Show confirmation dialog with "Always approve" and "YOLO" options
                    let message =
                        format!("Tool: {tool_name}\nLevel: {tool_level}\nArgs: {tool_args}");
                    let dialog_data = format!(
                       "Can I run this tool?\x00Sure\x00Always allow this tool with level {tool_level}\x00Not now\x00YOLO - allow all dangerous tools\x00{message}"
                    );
                    tracing::debug!("Showing dialog with data: {dialog_data}",);
                    let _ = self.app.attr(
                        &Id::Dialog,
                        Attribute::Custom("show"),
                        AttrValue::String(dialog_data),
                    );
                    // Give focus to dialog so it receives keyboard events
                    let result = self.app.active(&Id::Dialog);
                    tracing::debug!("Dialog focus result: {:?}", result);
                    self.state.should_redraw = true;
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Run the main loop
    #[allow(clippy::future_not_send)]
    pub async fn run(mut self) -> Result<TuiResult> {
        // Enter alternate screen
        self.terminal.enter_alternate_screen()?;
        self.terminal.enable_raw_mode()?;

        // Hide cursor by default (will be shown by InputComponent when needed)
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide)?;

        let _result = self.run_loop().await;

        // Cleanup
        self.terminal.leave_alternate_screen()?;
        self.terminal.disable_raw_mode()?;

        // Return result with new history entries and session flag
        Ok(TuiResult {
            input_history: self.get_new_history_entries(),
            should_create_new_session: self.state.should_create_new_session,
        })
    }

    #[allow(clippy::future_not_send)]
    async fn run_loop(&mut self) -> Result<()> {
        // Enable mouse capture
        self.terminal.enable_mouse_capture()?;

        while !self.state.quit {
            // Process kernel events
            self.process_kernel_events()?;

            // Tick the application
            match self.app.tick(PollStrategy::Once) {
                Ok(messages) if !messages.is_empty() => {
                    self.state.should_redraw = true;
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
            if self.state.should_redraw {
                self.view();
                self.state.should_redraw = false;
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
            self.state.should_redraw = true;

            match msg {
                Msg::Quit => {
                    self.state.quit = true;
                    None
                }
                // Ignore input-related messages in Browse mode
                Msg::InputSubmit(blocks) => {
                    if self.mode == AppMode::Browse {
                        return None;
                    }
                    // Extract text content for history and display
                    let text_content: String = blocks
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => text.as_str(),
                            ContentBlock::ImageUrl { image_url: _ } => "[Image]",
                            ContentBlock::Thinking { .. }
                            | ContentBlock::RedactedThinking { .. } => "",
                            ContentBlock::Audio { .. } => "[Audio]",
                        })
                        .collect();
                    // Save to history for C-n/C-p navigation
                    if !text_content.trim().is_empty() {
                        self.input_history.push(text_content.clone());
                        let _ = self.init_input_history();
                    }
                    // Add user message to chat view
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("add_user_message"),
                        AttrValue::String(text_content),
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
                    // Send to kernel (supports multi-modal content)
                    let _ = self.input_tx.try_send(blocks);
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
                    self.state.should_redraw = true;
                    None
                }
                Msg::ShowStatusMessage(msg, duration_ms) => {
                    // Format: "duration_ms\x00message"
                    let value = format!("{duration_ms}\x00{msg}");
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
                                    "0\x00C-o toggle, j/k/g/G scroll, q exit".to_string(),
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
                Msg::GoToTop => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("scroll_to_top"),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::GoToBottom => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("scroll_to_bottom"),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::DialogSelected(idx) => {
                    // Send permission response based on selection
                    // idx: 0 = Approve, 1 = Always approve, 2 = Deny, 3 = YOLO
                    if let Some(req_id) = self.pending_permission.take() {
                        let (approved, remember) = match idx {
                            0 => (true, false), // Approve once
                            1 => (true, true),  // Always approve this tool
                            3 => {
                                // YOLO mode - enable Dangerous level
                                self.permission_level = Level::Dangerous;
                                // Update status bar to show YOLO
                                let _ = self.app.attr(
                                    &Id::StatusBar,
                                    Attribute::Custom("set_permission_level"),
                                    AttrValue::Number(2),
                                );
                                // Show status message
                                let _ = self.app.attr(
                                    &Id::StatusBar,
                                    Attribute::Custom("show_message"),
                                    AttrValue::String(
                                        "5000\x00YOLO mode enabled - all tools will be auto-approved".to_string(),
                                    ),
                                );
                                // Send command to kernel to update permission level
                                let _ = self
                                    .permission_tx
                                    .try_send(PermissionCommand::SetLevel(Level::Dangerous));
                                (true, false)
                            }
                            _ => (false, false), // Deny
                        };
                        let _ = self.permission_tx.try_send(PermissionCommand::Response {
                            req_id,
                            approved,
                            remember,
                        });
                    }
                    // Return focus to input box
                    let _ = self.app.active(&Id::InputBox);
                    None
                }
                Msg::DialogCancelled => {
                    // Deny the permission request if dialog is cancelled
                    if let Some(req_id) = self.pending_permission.take() {
                        let _ = self.permission_tx.try_send(PermissionCommand::Response {
                            req_id,
                            approved: false,
                            remember: false,
                        });
                    }
                    // Return focus to input box
                    let _ = self.app.active(&Id::InputBox);
                    None
                }
                // Slash commands
                Msg::CommandNew => {
                    // Signal that a new session should be created
                    self.state.should_create_new_session = true;
                    self.state.quit = true;
                    None
                }
                Msg::CommandClear => {
                    // Clear chat history
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom("clear_history"),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::CommandYolo => {
                    // Toggle YOLO mode via command
                    self.update(Some(Msg::ToggleYoloMode))
                }
                Msg::ToggleYoloMode => {
                    // Toggle between Safe and Dangerous permission levels
                    let new_level = if self.permission_level == Level::Dangerous {
                        Level::Safe
                    } else {
                        Level::Dangerous
                    };
                    self.permission_level = new_level;

                    // Update status bar
                    let level_num = match new_level {
                        Level::Safe => 0,
                        Level::Caution => 1,
                        Level::Dangerous => 2,
                    };
                    let _ = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom("set_permission_level"),
                        AttrValue::Number(level_num),
                    );

                    // Show status message
                    let msg = if new_level == Level::Dangerous {
                        "YOLO mode enabled - all tools will be auto-approved"
                    } else {
                        "YOLO mode disabled"
                    };
                    let _ = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom("show_message"),
                        AttrValue::String(format!("5000\x00{msg}")),
                    );

                    // Send command to kernel
                    let _ = self
                        .permission_tx
                        .try_send(PermissionCommand::SetLevel(new_level));

                    None
                }
                Msg::CommandBrowse => {
                    // Toggle browse mode
                    self.update(Some(Msg::ToggleBrowseMode))
                }
                Msg::CommandUnknown(cmd) => {
                    // Show unknown command message in status bar
                    let _ = self.app.attr(
                        &Id::StatusBar,
                        Attribute::Custom("message"),
                        AttrValue::Payload(tuirealm::props::PropPayload::One(
                            tuirealm::props::PropValue::Str(format!("Unknown command: {cmd}")),
                        )),
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
#[allow(clippy::too_many_arguments, clippy::future_not_send)]
pub async fn run_tui(
    event_rx: mpsc::Receiver<AppEvent>,
    input_tx: mpsc::Sender<Vec<ContentBlock>>,
    cancel_tx: mpsc::Sender<()>,
    permission_tx: mpsc::Sender<PermissionCommand>,
    working_dir: String,
    skills: Vec<String>,
    input_history: Vec<String>,
    session_messages: Vec<Message>,
    permission_level: Level,
    context_window: u32,
) -> Result<TuiResult> {
    let working_dir_path = std::path::PathBuf::from(&working_dir);
    let mut model = Model::new(
        event_rx,
        input_tx,
        cancel_tx,
        permission_tx,
        input_history,
        working_dir_path,
        session_messages,
        permission_level,
    )?;
    model.init_banner(working_dir, skills)?;
    model.init_status_bar()?;
    // Set input history after banner init
    model.init_input_history()?;
    // Display session messages and init ctx usage (for resumed sessions)
    model.init_session_messages(context_window)?;
    // run() consumes model and returns the new history entries
    model.run().await
}
