//! Model struct and core methods

use anyhow::Result;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};
use tuirealm::{
    props::{AttrValue, Attribute},
    terminal::{CrosstermTerminalAdapter, TerminalAdapter},
};

use crate::{attr, components::info_bar::Notification, id::Id};
use kernel::event::{ControlCommand, Event};
use kernel::types::{ContentBlock, Message};

use super::types::{AppMode, AppState, Model, OnInputHook, StreamingStatus};

impl Model {
    /// Set focus to a component and re-enable mouse capture on Windows.
    /// This is a workaround for Windows console losing mouse capture after focus changes.
    pub(crate) fn set_focus(&mut self, id: &Id) {
        let _ = self.app.active(id);
        #[cfg(target_os = "windows")]
        {
            let _ = self.terminal.enable_mouse_capture();
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        event_rx: broadcast::Receiver<Event>,
        input_tx: mpsc::Sender<Vec<ContentBlock>>,
        ctrl_tx: mpsc::Sender<ControlCommand>,
        session_store: Arc<dyn kernel::storage::SessionStore>,
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
            session_store,
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
    pub(crate) fn suspend_process(&mut self) {
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
    pub(crate) fn suspend_process(&mut self) {
        // Ctrl-Z not supported on non-Unix platforms
        tracing::warn!("Suspend not supported on this platform");
    }

    /// Update scroll progress in status bar
    /// Shows scroll progress when user has scrolled up, clears when at bottom
    pub(crate) fn update_scroll_progress(&mut self) {
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
    pub(crate) fn calculate_input_height_for_content(content: &str, terminal_width: u16) -> u16 {
        use unicode_width::UnicodeWidthStr;

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
    pub(crate) fn save_partial_content(&mut self) -> anyhow::Result<()> {
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
    pub(crate) fn clear_streaming_state(&mut self) {
        self.current_content.clear();
        self.current_thinking.clear();
        self.thinking_start_time = None;
    }

    /// Scroll chat view to bottom
    pub(crate) fn scroll_chat_to_bottom(&mut self) {
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::SCROLL_TO_BOTTOM),
            AttrValue::Flag(true),
        );
    }

    /// Show error message in chat view
    pub(crate) fn show_error_message(&mut self, message: impl Into<String>) {
        let msg = message.into();
        let _ = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::ADD_ERROR_MESSAGE),
            AttrValue::String(msg),
        );
        // Note: scroll progress will be updated in next view() call (Browse mode)
    }

    /// Show notification in info bar
    pub(crate) fn show_notification(&mut self, notification: &Notification) {
        let _ = self.app.attr(
            &Id::InfoBar,
            Attribute::Custom(attr::SHOW_NOTIFICATION),
            notification.to_attr_value(),
        );
    }

    /// Send desktop notification via notify-rust (if enabled in feature gates)
    pub(crate) fn send_desktop_notification(title: &str, message: &str) {
        // Only send if desktop notifications are enabled
        if !crate::feature_gates().desktop_notify {
            return;
        }

        let title = title.to_string();
        let message = message.to_string();

        // Run in blocking task to avoid blocking async runtime
        tokio::task::spawn_blocking(move || {
            let _ = notify_rust::Notification::new()
                .summary(&title)
                .body(&message)
                .appname("Yomi")
                .timeout(notify_rust::Timeout::Milliseconds(5000))
                .show();
        });
    }

    /// Handle streaming error by stopping streaming and showing error message
    pub(crate) fn handle_streaming_error(
        &mut self,
        status: StreamingStatus,
        error_message: impl Into<String>,
    ) {
        self.stop_streaming(status);
        self.show_error_message(error_message);
        // Explicitly set should_redraw to ensure UI updates
        self.state.should_redraw = true;
    }

    /// Set a queued message to be sent when streaming ends
    pub(crate) fn set_queued_message(&mut self, blocks: Vec<ContentBlock>) {
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
        // Update InputComponent to know there's a queued message
        if let Err(e) = self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::HAS_QUEUED_MESSAGE),
            AttrValue::Flag(true),
        ) {
            tracing::warn!("Failed to set has_queued_message in InputBox: {}", e);
        }
        self.queued_message = Some(blocks);
        self.state.should_redraw = true;
    }

    /// Clear the queued message (e.g., when session is interrupted)
    pub(crate) fn clear_queued_message(&mut self) {
        if let Err(e) = self.app.attr(
            &Id::ChatView,
            Attribute::Custom(attr::CLEAR_QUEUED_MESSAGE),
            AttrValue::Flag(true),
        ) {
            tracing::warn!("Failed to clear queued message in ChatView: {}", e);
        }
        // Update InputComponent to know there's no queued message
        if let Err(e) = self.app.attr(
            &Id::InputBox,
            Attribute::Custom(attr::HAS_QUEUED_MESSAGE),
            AttrValue::Flag(false),
        ) {
            tracing::warn!("Failed to clear has_queued_message in InputBox: {}", e);
        }
        self.queued_message = None;
        self.state.should_redraw = true;
    }

    /// Send the queued message if any, returns true if a message was sent
    pub(crate) fn send_queued_message(&mut self) -> bool {
        if let Some(blocks) = self.queued_message.take() {
            // Clear the queued message display in ChatView
            if let Err(e) = self.app.attr(
                &Id::ChatView,
                Attribute::Custom(attr::CLEAR_QUEUED_MESSAGE),
                AttrValue::Flag(true),
            ) {
                tracing::warn!("Failed to clear queued message in ChatView: {}", e);
            }
            // Update InputComponent to know there's no queued message
            if let Err(e) = self.app.attr(
                &Id::InputBox,
                Attribute::Custom(attr::HAS_QUEUED_MESSAGE),
                AttrValue::Flag(false),
            ) {
                tracing::warn!("Failed to clear has_queued_message in InputBox: {}", e);
            }
            // Send to kernel (streaming will be started by ModelEvent::Request)
            if let Err(e) = self.input_tx.try_send(blocks) {
                tracing::error!("Failed to send queued message to kernel: {}", e);
            }
            self.state.should_redraw = true;
            true
        } else {
            false
        }
    }

    /// Convert input history to picker items for fuzzy search
    pub(crate) fn history_items(&self) -> Vec<crate::components::PickerItem> {
        use crate::utils::text::truncate_by_chars;

        self.input_history
            .iter()
            .enumerate()
            .map(|(idx, text)| {
                // Replace newlines with spaces and trim leading whitespace for preview
                let text_single_line = text.replace('\n', " ").trim_start().to_string();
                crate::components::PickerItem::new(
                    format!("history_{idx}"),
                    truncate_by_chars(&text_single_line, 50),
                )
            })
            .rev() // Most recent first
            .collect()
    }
}
