//! TUI Realm Application
//!
//! Main application using tuirealm framework for component-based TUI.
//!
//! This module is organized into submodules:
//! - `types`: Core types including `Model`, `TuiResult`, and `OnInputHook`
//! - `state`: Application state (`AppState`, `AppMode`, `StreamingStatus`)
//! - `streaming`: Streaming content handling
//! - `events`: Kernel event processing
//! - `commands`: UI command/message handling
//! - `render`: UI rendering
//! - `init`: Application initialization
//! - `notifications`: Desktop notifications
//! - `ui_ops`: Common UI operations

pub mod commands;
pub mod events;
pub mod init;
pub mod notifications;
pub mod render;
pub mod state;
pub mod streaming;
pub mod types;
pub mod ui_ops;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tokio::sync::{broadcast, mpsc};
use tuirealm::application::PollStrategy;

use kernel::event::{ControlCommand, Event};
use kernel::types::{ContentBlock, Message};

use crate::app::commands::CommandHandler;
use crate::app::events::EventHandler;
use crate::app::init::{Init, ComponentInit};
use crate::app::render::Render;
use crate::app::types::Model;
use tuirealm::terminal::TerminalAdapter;

/// Run the main loop
#[allow(clippy::too_many_arguments)]
pub async fn run_tui(
    event_rx: broadcast::Receiver<Event>,
    input_tx: mpsc::Sender<Vec<ContentBlock>>,
    ctrl_tx: mpsc::Sender<ControlCommand>,
    session_store: Arc<dyn kernel::storage::SessionStore>,
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
        session_store,
        input_history,
        working_dir_path,
        session_messages,
        initial_message,
        session_id,
        on_input_hook,
    )?;

    // Initialize components
    model.init_banner()?;
    model.init_status_bar()?;
    model.init_input_history()?;
    model.init_session_messages()?;
    model.init_todo_list().await?;

    // Run the application
    model.run().await
}

/// Model extension trait for main loop
trait ModelRun {
    async fn run(&mut self) -> Result<TuiResult>;
    async fn run_loop(&mut self) -> Result<()>;
}

impl ModelRun for Model {
    async fn run(&mut self) -> Result<TuiResult> {
        // Enter alternate screen
        self.terminal.enter_alternate_screen()?;
        self.terminal.enable_raw_mode()?;

        // Hide cursor by default
        crossterm::execute!(std::io::stdout(), crossterm::cursor::Hide)?;

        let _result = self.run_loop().await;

        // Cleanup
        self.terminal.leave_alternate_screen()?;
        self.terminal.disable_raw_mode()?;

        Ok(TuiResult {
            input_history: self.get_new_history_entries(),
            should_create_new_session: self.state.should_create_new_session,
            switch_to_session: self.state.switch_to_session.clone(),
        })
    }

    async fn run_loop(&mut self) -> Result<()> {
        use tuirealm::props::{AttrValue, Attribute};

        // Enable mouse capture
        self.terminal.enable_mouse_capture()?;

        // Enable bracketed paste mode
        crossterm::execute!(
            std::io::stdout(),
            crossterm::event::EnableBracketedPaste
        )?;

        // Enable keyboard enhancement flags
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PushKeyboardEnhancementFlags(
                crossterm::event::KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
            )
        );

        // Send initial message if provided
        if let Some(initial_msg) = self.state.initial_message.take() {
            let blocks = vec![ContentBlock::Text { text: initial_msg }];
            if let Err(e) = self.input_tx.try_send(blocks.clone()) {
                tracing::error!("Failed to send initial message: {}", e);
            }
            let blocks_json = serde_json::to_string(&blocks).unwrap_or_default();
            let _ = self.app.attr(
                &crate::id::Id::ChatView,
                Attribute::Custom(crate::attr::ADD_USER_MESSAGE),
                AttrValue::String(blocks_json),
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
                            msg = self.handle_command(msg);
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

        // Disable bracketed paste mode
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableBracketedPaste
        );

        // Pop keyboard enhancement flags
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::PopKeyboardEnhancementFlags
        );

        Ok(())
    }
}

/// Re-exports for convenience
pub use crate::app::state::{AppMode, AppState, FeatureGates, StreamingStatus};
pub use crate::app::types::{OnInputHook, TuiResult};
