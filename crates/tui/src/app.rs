//! TUI Realm Application
//!
//! Main application using tuirealm framework for component-based TUI.

use anyhow::Result;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Result type returned by TUI
pub struct TuiResult {
    /// Input history entries collected during this session
    pub input_history: Vec<String>,
    /// Whether to create a new session after exiting
    pub should_create_new_session: bool,
    /// Session ID to switch to (for /sessions command)
    pub switch_to_session: Option<String>,
}

/// Callback type for input hook - called when user submits input
pub type OnInputHook = Box<dyn Fn(&str) + Send + Sync>;
use tokio::sync::mpsc;
use tuirealm::{
    application::{Application, PollStrategy},
    listener::EventListenerCfg,
    props::{AttrValue, Attribute},
    ratatui::layout::{Constraint, Direction, Layout},
    state::{State, StateValue},
    subscription::{EventClause, Sub, SubClause},
    terminal::{CrosstermTerminalAdapter, TerminalAdapter},
};
use unicode_width::UnicodeWidthStr;

use kernel::event::{ControlCommand, Event as AppEvent};
use kernel::permissions::Level;
use kernel::tools::TODO_WRITE_TOOL_NAME;
use kernel::types::{ContentBlock, Message};

use crate::{
    attr,
    components::{
        default_help_sections, info_bar::Notification, status_bar::Tip, tips::get_random_tip,
        ChatViewComponent, FuzzyPickerComponent, HelpDialog, InfoBarComponent, InputComponent,
        PickerConfig, PickerItem, SelectDialogComponent, StatusBarComponent, TodoListComponent,
    },
    id::Id,
    msg::{Msg, UserEvent},
    utils::text::{substring_by_chars, truncate_by_chars},
};

/// Format a session ID for display, truncating long IDs with ellipsis.
/// Uses character-based slicing for Unicode safety.
fn format_short_id(id: &str) -> String {
    let char_count = id.chars().count();
    if char_count > 12 {
        let start = substring_by_chars(id, 0, 6);
        let end = substring_by_chars(id, char_count.saturating_sub(4), char_count);
        format!("{start}...{end}")
    } else {
        id.to_string()
    }
}

/// Application mode - single source of truth for UI mode
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    #[default]
    Normal = 0,
    Browse = 1,
}

/// Streaming end status for cleanup
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamingStatus {
    Completed,
    Cancelled,
    Failed,
    MaxIterations,
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
    /// Initial message to send on startup (from CLI prompt arg)
    pub initial_message: Option<String>,
    /// Session ID to switch to on exit (for /sessions command)
    pub switch_to_session: Option<String>,
}

pub struct Model {
    /// Application
    pub app: Application<Id, Msg, UserEvent>,
    /// Application state flags
    pub state: AppState,
    pub terminal: CrosstermTerminalAdapter,
    /// Channel to receive events from kernel
    pub event_rx: mpsc::Receiver<AppEvent>,
    /// Channel to send input to kernel (supports multi-modal content blocks)
    pub input_tx: mpsc::Sender<Vec<ContentBlock>>,
    /// Channel to send control commands (cancel, permission responses, level changes, compaction)
    pub ctrl_tx: mpsc::Sender<ControlCommand>,
    /// Storage for loading sessions list
    storage: Arc<dyn kernel::storage::Storage>,
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
    /// Working directory (for file completion and session listing)
    working_dir: std::path::PathBuf,
    /// Session messages to display on startup (for resumed sessions)
    session_messages: Vec<Message>,
    /// Current session ID
    session_id: String,
    /// Current permission level (can be changed at runtime via YOLO mode)
    permission_level: Level,
    /// Queued message waiting to be sent when streaming ends (only one allowed)
    queued_message: Option<Vec<ContentBlock>>,
    /// Hook called when user submits input (for saving session, etc.)
    on_input_hook: Option<OnInputHook>,
}

impl Model {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_rx: mpsc::Receiver<AppEvent>,
        input_tx: mpsc::Sender<Vec<ContentBlock>>,
        ctrl_tx: mpsc::Sender<ControlCommand>,
        storage: Arc<dyn kernel::storage::Storage>,
        input_history: Vec<String>,
        working_dir: std::path::PathBuf,
        session_messages: Vec<Message>,
        initial_message: Option<String>,
        session_id: String,
        on_input_hook: Option<OnInputHook>,
    ) -> Result<Self> {
        let terminal = CrosstermTerminalAdapter::new()?;
        let app = Self::init_app()?;

        Ok(Self {
            app,
            state: AppState {
                quit: false,
                should_redraw: true,
                is_streaming: false,
                should_create_new_session: false,
                initial_message,
                switch_to_session: None,
            },
            terminal,
            event_rx,
            input_tx,
            ctrl_tx,
            storage,
            current_content: String::new(),
            current_thinking: String::new(),
            thinking_start_time: None,
            mode: AppMode::Normal,
            pending_permission: None,
            initial_history_len: input_history.len(),
            input_history,
            working_dir,
            session_messages,
            session_id,
            permission_level: crate::config().auto_approve,
            queued_message: None,
            on_input_hook,
        })
    }

    /// Get new history entries collected during this session
    pub fn get_new_history_entries(&self) -> Vec<String> {
        self.input_history[self.initial_history_len..].to_vec()
    }

    /// Suspend process to background (Ctrl-Z)
    /// Restores terminal state, sends SIGSTOP to self, then reinitializes terminal on resume
    #[cfg(unix)]
    fn suspend_process(&mut self) {
        use nix::sys::signal::{kill, Signal};
        use nix::unistd::getpid;
        use std::io::Write;

        // Restore terminal state before suspending
        let _ = self.terminal.leave_alternate_screen();
        let _ = self.terminal.disable_raw_mode();
        let _ = self.terminal.disable_mouse_capture();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableBracketedPaste,
            crossterm::event::PopKeyboardEnhancementFlags
        );

        // Show cursor and print newline for clean shell prompt
        let _ = crossterm::execute!(std::io::stdout(), crossterm::cursor::Show);
        let _ = std::io::stdout().flush();

        // Send SIGSTOP to self - this suspends the process
        // The process will resume here when user runs `fg`
        let pid = getpid();
        if let Err(e) = kill(pid, Signal::SIGSTOP) {
            tracing::error!("Failed to send SIGSTOP: {}", e);
        }

        // Re-initialize terminal after resume (when `fg` is executed)
        // Small delay to let terminal stabilize
        std::thread::sleep(std::time::Duration::from_millis(50));

        let _ = self.terminal.enable_raw_mode();
        let _ = self.terminal.enter_alternate_screen();
        let _ = self.terminal.enable_mouse_capture();
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
            crossterm::cursor::Hide,
            crossterm::event::EnableBracketedPaste,
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );

        // Force a full terminal refresh by toggling to the opposite mode then back
        // This mimics what the user workaround does (toggle mode on then off)
        let current_mode = self.mode;
        let alt_mode = if current_mode == AppMode::Normal {
            AppMode::Browse
        } else {
            AppMode::Normal
        };

        // First: switch to opposite mode
        self.mode = alt_mode;
        let _ = self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SET_MODE),
            AttrValue::Number(alt_mode as isize),
        );
        let _ = self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::MODE),
            AttrValue::Number(alt_mode as isize),
        );

        // Render intermediate mode
        self.state.should_redraw = true;
        self.view();

        // Then: switch back to original mode
        self.mode = current_mode;
        let _ = self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SET_MODE),
            AttrValue::Number(current_mode as isize),
        );
        let _ = self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::MODE),
            AttrValue::Number(current_mode as isize),
        );

        // Final render
        self.state.should_redraw = true;
        self.view();
    }

    #[cfg(not(unix))]
    fn suspend_process(&mut self) {
        // Ctrl-Z not supported on non-Unix platforms
        tracing::warn!("Suspend not supported on this platform");
    }

    /// Initialize input history in the `InputBox` component
    pub fn init_input_history(&mut self) -> Result<()> {
        // Serialize history to JSON string
        let history_json = serde_json::to_string(&self.input_history)?;
        self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::HISTORY),
            AttrValue::String(history_json),
        )?;
        // Set working directory for file completion
        let working_dir_str = self.working_dir.to_string_lossy().to_string();
        let _ = self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::WORKING_DIR),
            AttrValue::String(working_dir_str),
        );
        Ok(())
    }

    /// Convert input history to picker items for fuzzy search
    fn history_items(&self) -> Vec<PickerItem> {
        self.input_history
            .iter()
            .enumerate()
            .map(|(idx, text)| {
                // Replace newlines with spaces and trim leading whitespace for preview
                let text_single_line = text.replace('\n', " ").trim_start().to_string();
                PickerItem::new(
                    format!("history_{idx}"),
                    truncate_by_chars(&text_single_line, 50),
                )
            })
            .rev() // Most recent first
            .collect()
    }

    /// Display session messages in `ChatView` and calculate initial token usage for `StatusBar`
    fn init_session_messages(&mut self) -> Result<()> {
        let context_window = crate::config().agent.compactor.context_window;

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
            Attribute::Custom(attr::INIT_HISTORY),
            AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(messages))),
        )?;
        Ok(())
    }

    /// Initialize banner (data comes from global config and `working_dir`)
    pub fn init_banner(&mut self) -> Result<()> {
        self.update_banner()
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
            Attribute::Custom(attr::SET_PERMISSION_LEVEL),
            AttrValue::Number(level_val),
        )?;

        // Inject a random tip on startup
        let tip = get_random_tip();
        self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SHOW_TIP),
            Tip::new(format!("💡 {tip}"), 10000).to_attr_value(),
        )?;

        Ok(())
    }

    /// Initialize context window display in status bar
    pub fn init_ctx_usage(&mut self, tokens: u32, context_window: u32) -> Result<()> {
        let usage_str = format!("{tokens}\x00{context_window}");
        self.app.attr(
            &Id::StatusBar,
            Attribute::Custom(attr::SET_CTX_USAGE),
            AttrValue::String(usage_str),
        )?;
        Ok(())
    }

    /// Initialize todo list from file storage
    pub fn init_todo_list(&mut self) -> Result<()> {
        use kernel::storage::TodoStorage;
        let todo_storage = TodoStorage::new(&crate::config().data_dir);
        if let Some(todo_json) = todo_storage.load(&self.session_id) {
            self.app.attr(
                &Id::TodoList,
                Attribute::Custom(attr::SET_TODOS),
                AttrValue::String(todo_json),
            )?;
        }
        Ok(())
    }

    /// Update banner in `ChatView` (data comes from global config and `working_dir`)
    pub fn update_banner(&mut self) -> Result<()> {
        let working_dir = self.working_dir.to_string_lossy().to_string();
        self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::SET_BANNER),
            AttrValue::String(working_dir),
        )?;
        Ok(())
    }

    /// Update scroll progress in status bar
    /// Shows scroll progress when user has scrolled up, clears when at bottom
    fn update_scroll_progress(&mut self) {
        // Query scroll progress from ChatView
        if let Ok(Some(query_result)) = self
            .app
            .query(&Id::ChatView, Attribute::Custom(attr::SCROLL_PROGRESS))
        {
            if let AttrValue::String(progress_str) = query_result.into_attr() {
                let parts: Vec<&str> = progress_str.split('\x00').collect();
                if parts.len() == 3 {
                    let is_scrolled = parts[2] == "1";
                    if is_scrolled {
                        // Set scroll progress (format: current\x00total)
                        let scroll_data = format!("{}\x00{}", parts[0], parts[1]);
                        let _ = self.app.attr(
                            &Id::StatusBar,
                            Attribute::Custom(attr::SET_SCROLL_PROGRESS),
                            AttrValue::String(scroll_data),
                        );
                    } else {
                        // At bottom, clear scroll progress
                        let _ = self.app.attr(
                            &Id::StatusBar,
                            Attribute::Custom(attr::CLEAR_SCROLL_PROGRESS),
                            AttrValue::Flag(true),
                        );
                    }
                }
            }
        }
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
                Attribute::Custom(attr::ADD_ASSISTANT_MSG),
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

    /// Start streaming - initialize UI components for streaming state
    fn start_streaming(&mut self) {
        self.state.is_streaming = true;
        self.clear_streaming_state();
        // Start ChatView streaming
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::START_STREAMING),
            AttrValue::Flag(true),
        );
        // Start InfoBar streaming
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr::START_STREAMING),
            AttrValue::Flag(true),
        );
        self.state.should_redraw = true;
    }

    /// Clear the tool call delta display from `InfoBar`.
    fn clear_tool_call_delta(&mut self) {
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr::CLEAR_TOOL_CALL),
            AttrValue::Flag(true),
        );
    }

    /// Stop streaming with given status - cleanup UI and save content
    fn stop_streaming(&mut self, status: StreamingStatus) {
        self.state.is_streaming = false;

        // Clear tool call state
        self.clear_tool_call_delta();

        match status {
            StreamingStatus::Completed => {
                let _ = self.app.attr(
                    &Id::InfoBar,
                    Attribute::Custom(attr::STOP_STREAMING),
                    AttrValue::Flag(true),
                );
                let _ = self.app.attr(
                    &Id::StatusBar,
                    Attribute::Custom(attr::CLEAR_MESSAGE),
                    AttrValue::Flag(true),
                );
                let _ = self.app.attr(
                    &Id::ChatView,
                    Attribute::Custom(attr::STOP_STREAMING),
                    AttrValue::Flag(true),
                );
                // Send queued message if any
                self.send_queued_message();
            }
            StreamingStatus::Cancelled
            | StreamingStatus::Failed
            | StreamingStatus::MaxIterations => {
                let _ = self.save_partial_content();
                let _ = self.app.attr(
                    &Id::InfoBar,
                    Attribute::Custom(attr::CANCEL_STREAMING),
                    AttrValue::Flag(true),
                );
                let _ = self.app.attr(
                    &Id::ChatView,
                    Attribute::Custom(attr::CANCEL_STREAMING),
                    AttrValue::Flag(true),
                );
                // Clear queued message on interruption
                self.clear_queued_message();
            }
        }
        self.clear_streaming_state();
        self.state.should_redraw = true;
    }

    /// Scroll chat view to bottom
    fn scroll_chat_to_bottom(&mut self) {
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::SCROLL_TO_BOTTOM),
            AttrValue::Flag(true),
        );
    }

    /// Show error message in chat view
    fn show_error_message(&mut self, message: impl Into<String>) {
        let msg = message.into();
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::ADD_ERROR_MESSAGE),
            AttrValue::String(msg),
        );
        // Note: scroll progress will be updated in next view() call (Browse mode)
    }

    /// Append streaming content to `ChatView` and `InfoBar`
    fn append_streaming_content(&mut self, text: &str, is_thinking: bool) {
        if is_thinking {
            if self.thinking_start_time.is_none() {
                self.thinking_start_time = Some(Instant::now());
            }
            self.current_thinking.push_str(text);
            let _ = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::APPEND_THINKING),
                AttrValue::String(text.to_string()),
            );
        } else {
            self.current_content.push_str(text);
            let _ = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::APPEND_CONTENT),
                AttrValue::String(text.to_string()),
            );
        }
        // Update InfoBar with content for token counting
        let attr = if is_thinking {
            "append_thinking"
        } else {
            "append_content"
        };
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr),
            AttrValue::String(text.to_string()),
        );
        self.state.should_redraw = true;
    }

    /// Save assistant message to chat history and clear streaming
    fn finalize_assistant_message(&mut self) {
        // Save if there's either content or thinking
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
            let _ = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::ADD_ASSISTANT_MSG),
                AttrValue::String(combined),
            );
        }
        // Clear streaming UI
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::CANCEL_STREAMING),
            AttrValue::Flag(true),
        );
    }

    /// Show notification in info bar
    fn show_notification(&mut self, notification: &Notification) {
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr::SHOW_NOTIFICATION),
            notification.to_attr_value(),
        );
    }

    /// Handle streaming error by stopping streaming and showing error message
    fn handle_streaming_error(
        &mut self,
        status: StreamingStatus,
        error_message: impl Into<String>,
    ) {
        self.stop_streaming(status);
        self.show_error_message(error_message);
        // Explicitly set should_redraw to ensure UI updates
        self.state.should_redraw = true;
    }

    /// Handle streaming cancellation with optional operation context
    fn handle_streaming_cancelled(&mut self, operation: Option<&str>) {
        let message =
            operation.map_or_else(|| "Cancelled".to_string(), |op| format!("Cancelled: {op}"));
        self.handle_streaming_error(StreamingStatus::Cancelled, message);
    }

    /// Set a queued message to be sent when streaming ends
    fn set_queued_message(&mut self, blocks: Vec<ContentBlock>) {
        // Check if there's already a queued message
        if self.queued_message.is_some() {
            tracing::info!("Overwriting existing queued message with new one");
        }
        // Serialize the queued message for display in ChatView
        let blocks_json = serde_json::to_string(&blocks).unwrap_or_default();
        if let Err(e) = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::SET_QUEUED_MESSAGE),
            AttrValue::String(blocks_json),
        ) {
            tracing::warn!("Failed to set queued message in ChatView: {}", e);
        }
        self.queued_message = Some(blocks);
        self.state.should_redraw = true;
    }

    /// Clear the queued message (e.g., when session is interrupted)
    fn clear_queued_message(&mut self) {
        if let Err(e) = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::CLEAR_QUEUED_MESSAGE),
            AttrValue::Flag(true),
        ) {
            tracing::warn!("Failed to clear queued message in ChatView: {}", e);
        }
        self.queued_message = None;
        self.state.should_redraw = true;
    }

    /// Send the queued message if any, returns true if a message was sent
    fn send_queued_message(&mut self) -> bool {
        if let Some(blocks) = self.queued_message.take() {
            // Clear the queued message display in ChatView
            if let Err(e) = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::CLEAR_QUEUED_MESSAGE),
                AttrValue::Flag(true),
            ) {
                tracing::warn!("Failed to clear queued message in ChatView: {}", e);
            }
            // Add user message to chat view
            let blocks_json = serde_json::to_string(&blocks).unwrap_or_default();
            if let Err(e) = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::ADD_USER_MESSAGE),
                AttrValue::String(blocks_json),
            ) {
                tracing::warn!("Failed to add user message in ChatView: {}", e);
            }
            self.scroll_chat_to_bottom();
            // Start streaming status
            if let Err(e) = self.app.attr(
                &Id::InfoBar,
                Attribute::Custom(attr::START_STREAMING),
                AttrValue::Flag(true),
            ) {
                tracing::warn!("Failed to start streaming in InfoBar: {}", e);
            }
            // Send to kernel
            if let Err(e) = self.input_tx.try_send(blocks) {
                tracing::error!("Failed to send queued message to kernel: {}", e);
            }
            self.state.should_redraw = true;
            true
        } else {
            false
        }
    }

    pub fn view(&mut self) {
        // Update scroll progress on each redraw (throttled by frame rate)
        // Shows progress when scrolled up, clears when at bottom
        self.update_scroll_progress();

        // Pre-fetch content to calculate height without borrowing self in closure
        let input_content =
            if let Ok(State::Single(StateValue::String(content))) = self.app.state(&Id::InputBox) {
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

            // Render history picker on top if active
            self.app.view(&Id::HistoryPicker, f, f.area());

            // Render session picker on top if active
            self.app.view(&Id::SessionPicker, f, f.area());

            // Render help dialog on top if active
            self.app.view(&Id::HelpDialog, f, f.area());

            // Render todo list floating panel (renders itself only if visible)
            self.app.view(&Id::TodoList, f, f.area());
        });
    }

    fn init_app() -> Result<Application<Id, Msg, UserEvent>> {
        let mut app = Application::init(
            EventListenerCfg::default()
                .crossterm_input_listener(Duration::from_millis(10), 10)
                .tick_interval(Duration::from_millis(100)),
        );

        // Mount unified chat view component (includes scrollable banner)
        app.mount(
            Id::ChatView,
            Box::new(ChatViewComponent::new()),
            vec![
                Sub::new(EventClause::Tick, SubClause::Always),
                Sub::new(EventClause::Any, SubClause::Always),
            ],
        )?;

        // Mount info bar component (token/streaming status)
        app.mount(
            Id::InfoBar,
            Box::new(InfoBarComponent::new()),
            vec![
                Sub::new(EventClause::Tick, SubClause::Always),
                Sub::new(EventClause::Any, SubClause::Always),
            ],
        )?;

        // Mount input component
        app.mount(Id::InputBox, Box::new(InputComponent::new()), vec![])?;

        // Mount status bar component (vim-style mode indicator at bottom)
        app.mount(
            Id::StatusBar,
            Box::new(StatusBarComponent::new()),
            vec![
                Sub::new(EventClause::Tick, SubClause::Always),
                Sub::new(EventClause::Any, SubClause::Always),
            ],
        )?;

        // Mount select dialog component (hidden by default, for permission confirmation)
        app.mount(
            Id::Dialog,
            Box::new(SelectDialogComponent::new("Dialog")),
            vec![Sub::new(EventClause::Any, SubClause::Always)],
        )?;

        // Mount history picker component (hidden by default, for C-r history search)
        let history_picker = FuzzyPickerComponent::new(
            PickerConfig::new("History").with_placeholder("Search history..."),
        )
        .with_callbacks(crate::msg::Msg::HistorySelected, || {
            crate::msg::Msg::CloseHistoryPicker
        });
        app.mount(
            Id::HistoryPicker,
            Box::new(history_picker),
            vec![Sub::new(EventClause::Any, SubClause::Always)],
        )?;

        // Mount session picker component (hidden by default, for /sessions command)
        let session_picker = FuzzyPickerComponent::new(
            PickerConfig::new("Switch Session")
                .with_placeholder("Search sessions...")
                .with_max_height(12),
        )
        .with_callbacks(crate::msg::Msg::SessionSelected, || {
            crate::msg::Msg::CloseSessionPicker
        });
        app.mount(
            Id::SessionPicker,
            Box::new(session_picker),
            vec![Sub::new(EventClause::Any, SubClause::Always)],
        )?;

        // Mount help dialog component (hidden by default)
        app.mount(
            Id::HelpDialog,
            Box::new(HelpDialog::new("Keyboard Shortcuts")),
            vec![Sub::new(EventClause::Any, SubClause::Always)],
        )?;

        // Mount todo list component (floating panel)
        app.mount(
            Id::TodoList,
            Box::new(TodoListComponent::new()),
            vec![Sub::new(EventClause::Tick, SubClause::Always)],
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
                    // Clear tool call delta when receiving regular content
                    self.clear_tool_call_delta();
                    match content {
                        kernel::event::ContentChunk::Text(text) => {
                            self.append_streaming_content(&text, false);
                        }
                        kernel::event::ContentChunk::Thinking { thinking, .. } => {
                            self.append_streaming_content(&thinking, true);
                        }
                        kernel::event::ContentChunk::RedactedThinking => {}
                    }
                }
                AppEvent::Model(kernel::event::ModelEvent::ToolCallDelta {
                    tool_name,
                    arguments_delta,
                    ..
                }) => {
                    // Update status bar to show tool call in progress
                    let attr = Attribute::Custom(attr::APPEND_TOOL_CALL_DELTA);
                    let value = AttrValue::String(format!("{tool_name}\x00{arguments_delta}"));
                    self.app.attr(&Id::InfoBar, attr, value)?;
                    self.state.should_redraw = true;
                }
                AppEvent::Model(kernel::event::ModelEvent::Completed { .. }) => {
                    self.finalize_assistant_message();
                    self.stop_streaming(StreamingStatus::Completed);
                    // Note: Don't scroll to bottom here - respect user's scroll position
                }
                AppEvent::Model(kernel::event::ModelEvent::Error { error, .. }) => {
                    // Model-level error: stop streaming and show error
                    self.handle_streaming_error(
                        StreamingStatus::Failed,
                        format!("Model error: {error}"),
                    );
                }
                AppEvent::Model(kernel::event::ModelEvent::Request { .. }) => {
                    self.start_streaming();
                }
                AppEvent::Model(kernel::event::ModelEvent::Compacting { active, .. }) => {
                    // Show/hide compacting status in InfoBar
                    let attr = if active {
                        Attribute::Custom(attr::START_COMPACTING)
                    } else {
                        Attribute::Custom(attr::STOP_COMPACTING)
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
                        Attribute::Custom(attr::SET_CTX_USAGE),
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
                    let args_str = arguments.clone().unwrap_or_default();
                    let combined = format!("{tool_id}\x00{tool_name}\x00{args_str}");
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::START_TOOL),
                        AttrValue::String(combined),
                    )?;

                    // Handle todoWrite tool - update todo list panel
                    if tool_name == TODO_WRITE_TOOL_NAME {
                        if let Some(args) = arguments {
                            self.app.attr(
                                &Id::TodoList,
                                Attribute::Custom(attr::SET_TODOS),
                                AttrValue::String(args),
                            )?;
                        }
                    }

                    self.state.should_redraw = true;
                }
                AppEvent::Tool(kernel::event::ToolEvent::Output {
                    tool_id,
                    output,
                    content_blocks,
                    elapsed_ms,
                    ..
                }) => {
                    // Clear tool call state from info bar (tool execution is complete)
                    self.clear_tool_call_delta();
                    // Show tool output in chat view
                    // Format: tool_id\x00output\x00elapsed_ms\x00content_blocks_json
                    let blocks_json = serde_json::to_string(&content_blocks).unwrap_or_default();
                    let combined =
                        format!("{tool_id}\x00{output}\x00{elapsed_ms}\x00{blocks_json}");
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::COMPLETE_TOOL),
                        AttrValue::String(combined),
                    )?;
                    self.state.should_redraw = true;
                    // Windows workaround: re-enable mouse capture after shell commands
                    // Shell tools may disable ENABLE_MOUSE_INPUT console mode on Windows
                    #[cfg(target_os = "windows")]
                    {
                        let _ = self.terminal.enable_mouse_capture();
                    }
                }
                AppEvent::Tool(kernel::event::ToolEvent::Error {
                    tool_id,
                    error,
                    elapsed_ms,
                    ..
                }) => {
                    // Clear tool call state from info bar (tool execution failed)
                    self.clear_tool_call_delta();
                    // Show tool error in chat view
                    let combined = format!("{tool_id}\x00{error}\x00{elapsed_ms}");
                    self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::FAIL_TOOL),
                        AttrValue::String(combined),
                    )?;
                    self.state.should_redraw = true;
                    // Windows workaround: re-enable mouse capture after shell commands
                    #[cfg(target_os = "windows")]
                    {
                        let _ = self.terminal.enable_mouse_capture();
                    }
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
                        Attribute::Custom(attr::UPDATE_TOOL_PROGRESS),
                        AttrValue::String(combined),
                    )?;
                    self.state.should_redraw = true;
                }
                AppEvent::Agent(kernel::event::AgentEvent::Cancelled { operation, .. }) => {
                    self.handle_streaming_cancelled(operation.as_deref());
                }
                AppEvent::Agent(kernel::event::AgentEvent::Failed { error, .. }) => {
                    self.handle_streaming_error(
                        StreamingStatus::Failed,
                        format!("Agent error: {error}"),
                    );
                }
                // Error events - recoverable or non-recoverable
                AppEvent::Agent(kernel::event::AgentEvent::Error {
                    phase,
                    error,
                    is_recoverable,
                    ..
                }) => {
                    let phase_str = format!("{phase:?}");
                    if is_recoverable {
                        // Recoverable error: show in status bar with warning color
                        let message = format!("{phase_str} error (will retry): {error}");
                        self.show_notification(&Notification::warn(message, 3000));
                        self.state.should_redraw = true;
                    } else {
                        // Non-recoverable error: stop streaming and add to chat view
                        self.handle_streaming_error(
                            StreamingStatus::Failed,
                            format!("{phase_str} error: {error}"),
                        );
                    }
                }
                // Retrying event - show in status bar
                AppEvent::Agent(kernel::event::AgentEvent::Retrying {
                    attempt,
                    max_attempts,
                    reason,
                    ..
                }) => {
                    let message = format!("Retrying ({attempt}/{max_attempts}): {reason}");
                    // 0 = no timeout, persists until cleared
                    self.show_notification(&Notification::info(message, 0));
                    self.state.should_redraw = true;
                }
                // Max iterations reached - show in chat view
                AppEvent::Agent(kernel::event::AgentEvent::MaxIterationsReached {
                    count, ..
                }) => {
                    self.handle_streaming_error(
                        StreamingStatus::MaxIterations,
                        format!("Reached maximum iterations ({count})"),
                    );
                }
                // Note: StateChanged is currently ignored to avoid UI noise
                // Could be shown in status bar for debugging if needed
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
                        Attribute::Custom(attr::DIALOG_SHOW),
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
            switch_to_session: self.state.switch_to_session.clone(),
        })
    }

    #[allow(clippy::future_not_send)]
    async fn run_loop(&mut self) -> Result<()> {
        // Enable mouse capture
        self.terminal.enable_mouse_capture()?;

        // Enable bracketed paste mode for paste event detection
        crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste)?;

        // Enable keyboard enhancement flags to support Shift+Enter and other modified keys
        // This enables the terminal to report key events with modifiers disambiguated
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );

        // Send initial message if provided (from CLI prompt arg)
        if let Some(initial_msg) = self.state.initial_message.take() {
            let blocks = vec![ContentBlock::Text { text: initial_msg }];
            // Send to coordinator
            if let Err(e) = self.input_tx.try_send(blocks.clone()) {
                tracing::error!("Failed to send initial message: {}", e);
            }
            // Display user message in chat with content blocks
            let blocks_json = serde_json::to_string(&blocks).unwrap_or_default();
            let _ = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::ADD_USER_MESSAGE),
                AttrValue::String(blocks_json),
            );
            // Start streaming indicator (InfoBar only - ChatView will be started by ModelEvent::Request)
            let _ = self.app.attr(
                &Id::InfoBar,
                Attribute::Custom(attr::START_STREAMING),
                AttrValue::Flag(true),
            );
        }

        while !self.state.quit {
            // Process kernel events
            self.process_kernel_events()?;

            // Tick the application
            match self.app.tick(PollStrategy::Once(Duration::from_millis(10))) {
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

        // Disable bracketed paste mode on exit
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);

        // Pop keyboard enhancement flags
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );

        Ok(())
    }
}

impl Model {
    pub fn update(&mut self, msg: Option<Msg>) -> Option<Msg> {
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
                    // Extract text content for history navigation
                    let text_content: String = blocks
                        .iter()
                        .filter_map(|b| match b {
                            ContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    // Save to history for C-n/C-p navigation
                    if !text_content.trim().is_empty() {
                        // Remove duplicate if exists, keeping only the most recent
                        self.input_history.retain(|h| h != &text_content);
                        self.input_history.push(text_content.clone());
                        let _ = self.init_input_history();
                    }

                    // Call input hook if provided (e.g., for saving session)
                    if let Some(ref hook) = self.on_input_hook {
                        hook(&self.session_id);
                    }

                    // Check if we're currently streaming
                    if self.state.is_streaming {
                        // Queue the message to be sent when streaming ends (only one allowed)
                        self.set_queued_message(blocks);
                    } else {
                        // Add user message to chat view with content blocks
                        let blocks_json = serde_json::to_string(&blocks).unwrap_or_default();
                        let _ = self.app.attr(
                            &Id::ChatView,
                            Attribute::Custom(attr::ADD_USER_MESSAGE),
                            AttrValue::String(blocks_json),
                        );
                        self.scroll_chat_to_bottom();
                        // Start streaming status immediately when sending request
                        // (ChatView streaming will be started by ModelEvent::Request)
                        let _ = self.app.attr(
                            &Id::InfoBar,
                            Attribute::Custom(attr::START_STREAMING),
                            AttrValue::Flag(true),
                        );
                        // Send to kernel (supports multi-modal content)
                        let _ = self.input_tx.try_send(blocks);
                    }
                    None
                }
                // Scrolling - works in both modes
                Msg::ScrollUp => {
                    let amount = if self.mode == AppMode::Browse { 1 } else { 3 };
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_UP),
                        AttrValue::Number(amount as isize),
                    );
                    None
                }
                Msg::ScrollDown => {
                    let amount = if self.mode == AppMode::Browse { 1 } else { 3 };
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_DOWN),
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
                    let _ = self.ctrl_tx.try_send(ControlCommand::Cancel);
                    None
                }
                Msg::Redraw => {
                    self.state.should_redraw = true;
                    None
                }
                Msg::Notification(msg) => {
                    self.show_notification(&msg);
                    None
                }
                // Mode switching
                Msg::ToggleBrowseMode => {
                    match self.mode {
                        AppMode::Normal => {
                            // Enter browse mode
                            self.mode = AppMode::Browse;
                            // Update status bar to show BROWSE mode
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom(attr::SET_MODE),
                                AttrValue::Number(1),
                            );
                            // Update input box mode so it knows to use browse shortcuts
                            let _ = self.app.attr(
                                &Id::InputBox,
                                Attribute::Custom(attr::MODE),
                                AttrValue::Number(1),
                            );
                            // Show help message for browse mode shortcuts
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom(attr::SHOW_TIP),
                                Tip::new("C-o toggle, C-e expand, j/k/g/G scroll, q exit", 0)
                                    .to_attr_value(),
                            );
                            // Scroll progress will be updated in view() on next redraw
                        }
                        AppMode::Browse => {
                            // Exit browse mode
                            self.mode = AppMode::Normal;
                            // Collapse all blocks
                            let _ = self.app.attr(
                                &Id::ChatView,
                                Attribute::Custom(attr::COLLAPSE_ALL),
                                AttrValue::Flag(true),
                            );
                            // Update status bar to show NORMAL mode
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom(attr::SET_MODE),
                                AttrValue::Number(0),
                            );
                            // Update input box mode so it uses normal text input
                            let _ = self.app.attr(
                                &Id::InputBox,
                                Attribute::Custom(attr::MODE),
                                AttrValue::Number(0),
                            );
                            // Clear tip
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom(attr::CLEAR_TIP),
                                AttrValue::Flag(true),
                            );
                            // Clear scroll progress (restore context usage display)
                            let _ = self.app.attr(
                                &Id::StatusBar,
                                Attribute::Custom(attr::CLEAR_SCROLL_PROGRESS),
                                AttrValue::Flag(true),
                            );
                        }
                    }
                    None
                }
                Msg::PageHalfUp => {
                    let height = self
                        .terminal
                        .raw()
                        .size()
                        .map_or(20, |s| (s.height / 2) as usize);
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::PAGE_UP),
                        AttrValue::Number(height as isize),
                    );
                    None
                }
                Msg::PageHalfDown => {
                    let height = self
                        .terminal
                        .raw()
                        .size()
                        .map_or(20, |s| (s.height / 2) as usize);
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::PAGE_DOWN),
                        AttrValue::Number(height as isize),
                    );
                    None
                }
                Msg::GoToTop => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_TO_TOP),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::GoToBottom => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::SCROLL_TO_BOTTOM),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::ToggleExpandAll => {
                    let _ = self.app.attr(
                        &Id::ChatView,
                        Attribute::Custom(attr::TOGGLE_EXPAND_ALL),
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
                                    Attribute::Custom(attr::SET_PERMISSION_LEVEL),
                                    AttrValue::Number(2),
                                );
                                // Show notification
                                self.show_notification(&Notification::info(
                                    "YOLO mode enabled - all tools will be auto-approved",
                                    5000,
                                ));
                                // Send command to kernel to update permission level
                                let _ = self
                                    .ctrl_tx
                                    .try_send(ControlCommand::SetLevel(Level::Dangerous));
                                (true, false)
                            }
                            _ => (false, false), // Deny
                        };
                        let _ = self.ctrl_tx.try_send(ControlCommand::Response {
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
                        let _ = self.ctrl_tx.try_send(ControlCommand::Response {
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
                        Attribute::Custom(attr::CLEAR_HISTORY),
                        AttrValue::Flag(true),
                    );
                    // Clear todo list
                    let _ = self.app.attr(
                        &Id::TodoList,
                        Attribute::Custom(attr::CLEAR_TODOS),
                        AttrValue::Flag(true),
                    );
                    None
                }
                Msg::CommandTodos => {
                    // Toggle todo list visibility
                    let _ = self.app.attr(
                        &Id::TodoList,
                        Attribute::Custom(attr::TOGGLE_TODOS),
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
                        Attribute::Custom(attr::SET_PERMISSION_LEVEL),
                        AttrValue::Number(level_num),
                    );

                    // Show status message
                    let msg = if new_level == Level::Dangerous {
                        "YOLO mode enabled - all tools will be auto-approved"
                    } else {
                        "YOLO mode disabled"
                    };
                    self.show_notification(&Notification::info(msg, 5000));

                    // Send command to kernel
                    let _ = self.ctrl_tx.try_send(ControlCommand::SetLevel(new_level));

                    None
                }
                Msg::CommandBrowse => {
                    // Toggle browse mode
                    self.update(Some(Msg::ToggleBrowseMode))
                }
                Msg::CommandCompact => {
                    // Send compact request
                    let _ = self.ctrl_tx.try_send(ControlCommand::Compact);
                    self.show_notification(&Notification::info("Compacting messages...", 3000));
                    None
                }
                Msg::Suspend => {
                    // Suspend process to background (Ctrl-Z)
                    self.suspend_process();
                    None
                }
                // History picker messages
                Msg::ShowHistoryPicker => {
                    // Convert history to picker items (most recent first)
                    let items = self.history_items();
                    // Show the picker with history items
                    let _ = self.app.attr(
                        &Id::HistoryPicker,
                        Attribute::Custom(attr::PICKER_ITEMS),
                        AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(items))),
                    );
                    let _ = self.app.attr(
                        &Id::HistoryPicker,
                        Attribute::Custom(attr::DIALOG_SHOW),
                        AttrValue::Flag(true),
                    );
                    // Give focus to history picker
                    let _ = self.app.active(&Id::HistoryPicker);
                    None
                }
                Msg::HistorySelected(idx_str) => {
                    // Extract the actual index from "history_{idx}"
                    if let Some(idx_part) = idx_str.strip_prefix("history_") {
                        if let Ok(idx) = idx_part.parse::<usize>() {
                            if idx < self.input_history.len() {
                                let selected_text = self.input_history[idx].clone();
                                // Set the input box content using custom attribute
                                let _ = self.app.attr(
                                    &Id::InputBox,
                                    Attribute::Custom(attr::INPUT_CONTENT),
                                    AttrValue::String(selected_text),
                                );
                            }
                        }
                    }
                    // Return focus to input box and trigger redraw
                    let _ = self.app.active(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CloseHistoryPicker => {
                    // Return focus to input box and trigger redraw
                    let _ = self.app.active(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                // Help dialog messages
                Msg::CommandHelp => {
                    // Show help dialog with default help sections
                    let sections = default_help_sections();
                    if let Err(e) = self.app.attr(
                        &Id::HelpDialog,
                        Attribute::Custom(attr::DIALOG_SHOW),
                        AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(sections))),
                    ) {
                        tracing::warn!("Failed to show help dialog: {}", e);
                    }
                    // Give focus to help dialog so it receives keyboard events
                    if let Err(e) = self.app.active(&Id::HelpDialog) {
                        tracing::warn!("Failed to focus help dialog: {}", e);
                    }
                    self.state.should_redraw = true;
                    None
                }
                Msg::CommandSessions => {
                    // Load sessions for current working dir and show picker
                    let working_dir = self.working_dir.to_string_lossy().to_string();
                    let sessions = tokio::task::block_in_place(|| {
                        tokio::runtime::Handle::current()
                            .block_on(self.storage.list_sessions_by_working_dir(&working_dir))
                    })
                    .unwrap_or_default();

                    let items: Vec<crate::components::PickerItem> = sessions
                        .into_iter()
                        .map(|s| {
                            let age_str = s.format_age();
                            let preview = s
                                .title
                                .unwrap_or_else(|| "(no message)".to_string())
                                .replace('\n', " ");
                            let short_id = format_short_id(&s.id);
                            let label = format!("{short_id} - {age_str}");
                            crate::components::PickerItem::new(s.id, label).with_meta(preview)
                        })
                        .collect();

                    // Show the session picker
                    if let Err(e) = self.app.attr(
                        &Id::SessionPicker,
                        Attribute::Custom(attr::PICKER_ITEMS),
                        AttrValue::Payload(tuirealm::props::PropPayload::Any(Box::new(items))),
                    ) {
                        tracing::warn!("Failed to set session picker items: {}", e);
                    }
                    if let Err(e) = self.app.attr(
                        &Id::SessionPicker,
                        Attribute::Custom(attr::DIALOG_SHOW),
                        AttrValue::Flag(true),
                    ) {
                        tracing::warn!("Failed to show session picker: {}", e);
                    }
                    // Give focus to session picker
                    if let Err(e) = self.app.active(&Id::SessionPicker) {
                        tracing::warn!("Failed to focus session picker: {}", e);
                    }
                    self.state.should_redraw = true;
                    None
                }
                Msg::SessionSelected(session_id) => {
                    // Hide picker and set switch target
                    let _ = self.app.attr(
                        &Id::SessionPicker,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    self.state.switch_to_session = Some(session_id);
                    self.state.quit = true;
                    None
                }
                Msg::CloseSessionPicker => {
                    // Hide session picker and return focus to input box
                    let _ = self.app.attr(
                        &Id::SessionPicker,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    );
                    let _ = self.app.active(&Id::InputBox);
                    self.state.should_redraw = true;
                    None
                }
                Msg::CloseHelpDialog => {
                    // Hide help dialog and return focus to input box
                    if let Err(e) = self.app.attr(
                        &Id::HelpDialog,
                        Attribute::Custom(attr::DIALOG_HIDE),
                        AttrValue::Flag(true),
                    ) {
                        tracing::warn!("Failed to hide help dialog: {}", e);
                    }
                    if let Err(e) = self.app.active(&Id::InputBox) {
                        tracing::warn!("Failed to focus input box: {}", e);
                    }
                    self.state.should_redraw = true;
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
    ctrl_tx: mpsc::Sender<ControlCommand>,
    storage: Arc<dyn kernel::storage::Storage>,
    working_dir: String,
    input_history: Vec<String>,
    session_messages: Vec<Message>,
    initial_message: Option<String>,
    session_id: String,
    on_input_hook: Option<OnInputHook>,
) -> Result<TuiResult> {
    let working_dir_path = std::path::PathBuf::from(&working_dir);
    let mut model = Model::new(
        event_rx,
        input_tx,
        ctrl_tx,
        storage,
        input_history,
        working_dir_path,
        session_messages,
        initial_message,
        session_id,
        on_input_hook,
    )?;
    model.init_banner()?;
    model.init_status_bar()?;
    // Set input history after banner init
    model.init_input_history()?;
    // Display session messages and init ctx usage (for resumed sessions)
    model.init_session_messages()?;
    // Initialize todo list from file
    model.init_todo_list()?;
    // run() consumes model and returns the new history entries
    model.run().await
}
