//! Main run loop and entry point

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tuirealm::{application::PollStrategy, terminal::TerminalAdapter};

use kernel::client::CoordinatorApi;
use kernel::event::{ControlCommand, Event};
use kernel::types::ContentBlock;

use super::types::{Model, TuiResult};

impl Model {
    /// Run the main loop
    #[allow(clippy::future_not_send)]
    pub async fn run(mut self) -> anyhow::Result<TuiResult> {
        // Enter alternate screen
        self.terminal.enter_alternate_screen()?;
        self.terminal.enable_raw_mode()?;

        // Hide cursor by default (will be shown by InputComponent when needed)
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide)?;

        let result = self.run_loop().await;

        // Cleanup
        self.terminal.leave_alternate_screen()?;
        self.terminal.disable_raw_mode()?;

        result?;

        // Return result with new history entries and session flag
        Ok(TuiResult {
            input_history: self.get_new_history_entries(),
            should_create_new_session: self.state.should_create_new_session,
            switch_to_session: self.state.switch_to_session.clone(),
        })
    }

    #[allow(clippy::future_not_send)]
    async fn run_loop(&mut self) -> anyhow::Result<()> {
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
        // Note: We only send to coordinator here. The user message will be displayed
        // via kernel's Event::User, avoiding duplicate display.
        if let Some(initial_msg) = self.state.initial_message.take() {
            let blocks = vec![ContentBlock::Text { text: initial_msg }];
            // Send to coordinator (display will be handled by process_kernel_event)
            if let Err(e) = self.input_tx.try_send(blocks) {
                tracing::error!("Failed to send initial message: {}", e);
            }
        }

        while !self.state.quit {
            // Process kernel events (fast, non-blocking)
            self.process_kernel_event().await;

            // If a background reconnect task finished, swap in the new
            // receiver and notify the user.
            if let Some(task) = self.reconnect_task.take() {
                if task.is_finished() {
                    match task.await {
                        Ok(Some(new_rx)) => self.finish_reconnect(new_rx),
                        Ok(None) => tracing::warn!("Event channel reconnect timed out"),
                        Err(e) => tracing::error!("Reconnect task panicked: {e}"),
                    }
                } else {
                    self.reconnect_task = Some(task);
                }
            }

            // Tick the application
            match self.app.tick(PollStrategy::Once(Duration::from_millis(10))) {
                Ok(messages) if !messages.is_empty() => {
                    self.state.should_redraw = true;
                    for msg in messages {
                        let mut msg = Some(msg);
                        while msg.is_some() {
                            msg = self.update(msg).await;
                        }
                    }
                }
                _ => {}
            }

            // Detect terminal resize and force redraw
            if let Ok(size) = self.terminal.raw().size() {
                let new_size = (size.width, size.height);
                if new_size != self.last_terminal_size {
                    self.last_terminal_size = new_size;
                    self.state.should_redraw = true;
                }
            }

            // Redraw if needed
            if self.state.should_redraw {
                self.view();
                self.state.should_redraw = false;
            }
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

/// Run the TUI application
#[allow(clippy::future_not_send, clippy::too_many_arguments)]
pub async fn run_tui(
    event_rx: broadcast::Receiver<Event>,
    input_tx: mpsc::Sender<Vec<ContentBlock>>,
    ctrl_tx: mpsc::Sender<ControlCommand>,
    coordinator: Arc<dyn CoordinatorApi>,
    working_dir: String,
    input_history: Vec<String>,
    initial_message: Option<String>,
    session_id: String,
) -> anyhow::Result<TuiResult> {
    let working_dir_path = std::path::PathBuf::from(&working_dir);
    let mut model = Model::new(
        event_rx,
        input_tx,
        ctrl_tx,
        coordinator,
        input_history,
        working_dir_path,
        initial_message,
        session_id,
    )?;
    model.init_status_bar()?;
    model.init_input_history()?;
    // Display session messages and init ctx usage (for resumed sessions)
    model.init_session_messages().await?;
    // Initialize todo list from file
    model.init_todo_list().await?;
    // run() consumes model and returns the new history entries
    model.run().await
}
