//! Main run loop and entry point

use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tuirealm::{application::PollStrategy, terminal::TerminalAdapter};

use kernel::client::CoordinatorApi;
use kernel::event::ControlCommand;
use kernel::types::ContentBlock;

use super::types::{Model, TuiResult};
use crate::msg::Msg;

/// RAII guard that restores terminal state on drop.
struct TerminalGuard;

impl TerminalGuard {
    fn new() -> Self {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableBracketedPaste);
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture);
        Self
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
        let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableBracketedPaste);
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );
    }
}

impl Model {
    /// Run the main loop
    #[allow(clippy::future_not_send)]
    pub async fn run(mut self) -> anyhow::Result<TuiResult> {
        // Enter alternate screen
        self.terminal.enter_alternate_screen()?;
        self.terminal.enable_raw_mode()?;

        // Hide cursor by default (will be shown by InputComponent when needed)
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide)?;

        // Spawn signal watcher that sets the shared quit flag on SIGINT/SIGTERM.
        let quit_flag = Arc::clone(&self.signal_quit);
        let signal_task = tokio::spawn(async move {
            #[cfg(unix)]
            {
                let mut sigterm =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).ok();
                let mut sigint =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()).ok();
                tokio::select! {
                    _ = async { sigterm.as_mut()?.recv().await } => {},
                    _ = async { sigint.as_mut()?.recv().await } => {},
                }
            }
            #[cfg(not(unix))]
            {
                let _ = tokio::signal::ctrl_c().await;
            }
            quit_flag.store(true, std::sync::atomic::Ordering::Relaxed);
        });

        let result = self.run_loop().await;

        // Abort the signal watcher so it doesn't leak after normal exit.
        signal_task.abort();
        let _ = signal_task.await;

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
        let _guard = TerminalGuard::new();

        // Send initial message if provided (from CLI prompt arg)
        if let Some(initial_msg) = self.state.initial_message.take() {
            let blocks = vec![ContentBlock::Text { text: initial_msg }];
            if let Err(e) = self.input_tx.try_send(blocks) {
                tracing::error!("Failed to send initial message: {}", e);
            }
        }

        while !self.state.quit && !self.signal_quit.load(std::sync::atomic::Ordering::Relaxed) {
            self.process_kernel_event().await;

            // Poll tuirealm events (blocking up to 10ms)
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

            // Drain async command results injected by background tasks.
            while let Ok(cmd_msg) = self.cmd_rx.try_recv() {
                let mut msg = Some(cmd_msg);
                while msg.is_some() {
                    msg = self.update(msg).await;
                }
            }

            // Drain completed clipboard reads and inject the result back into the UI.
            if let Some(handle) = self.clipboard_handle.take() {
                if handle.is_finished() {
                    if let Ok(Some(text)) = handle.await {
                        let mut msg = Some(Msg::ClipboardText(text));
                        while msg.is_some() {
                            msg = self.update(msg).await;
                        }
                    }
                } else {
                    self.clipboard_handle = Some(handle);
                }
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

        Ok(())
    }
}

/// Run the TUI application
#[allow(clippy::future_not_send, clippy::too_many_arguments)]
pub async fn run_tui(
    event_rx: kernel::comms::EventBusSubscriber,
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
    model.init_skills().await?;
    model.init_session_messages().await?;
    model.init_todo_list().await?;
    model.run().await
}
