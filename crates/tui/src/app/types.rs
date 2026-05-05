//! Core types for the TUI application

use std::sync::Arc;

use tokio::sync::{broadcast, mpsc};
use tuirealm::application::Application;
use tuirealm::terminal::CrosstermTerminalAdapter;

use kernel::event::{ControlCommand, Event};
use kernel::permissions::Level;
use kernel::types::{ContentBlock, Message};

use crate::app::state::AppState;
use crate::id::Id;
use crate::msg::{Msg, UserEvent};

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

/// TUI Model holding application state
pub struct Model {
    /// Application
    pub app: Application<Id, Msg, UserEvent>,
    /// Application state flags
    pub state: AppState,
    pub terminal: CrosstermTerminalAdapter,
    /// Channel to receive events from kernel
    pub event_rx: broadcast::Receiver<Event>,
    /// Channel to send input to kernel (supports multi-modal content blocks)
    pub input_tx: mpsc::Sender<Vec<ContentBlock>>,
    /// Channel to send control commands (cancel, permission responses, level changes, compaction)
    pub ctrl_tx: mpsc::Sender<ControlCommand>,
    /// Storage for loading sessions list
    pub session_store: Arc<dyn kernel::storage::SessionStore>,
    /// Current assistant response content (for adding to history when complete)
    pub current_content: String,
    /// Current assistant thinking (for adding to history when complete)
    pub current_thinking: String,
    /// When thinking started (for calculating elapsed time)
    pub thinking_start_time: Option<std::time::Instant>,
    /// Pending permission request (`req_id`) waiting for user confirmation
    pub pending_permission: Option<String>,
    /// Input history for the current working directory (loaded + new)
    pub input_history: Vec<String>,
    /// Initial history length (to identify new entries on exit)
    pub initial_history_len: usize,
    /// Working directory (for file completion and session listing)
    pub working_dir: std::path::PathBuf,
    /// Session messages to display on startup (for resumed sessions)
    pub session_messages: Vec<Message>,
    /// Current session ID
    pub session_id: String,
    /// Current permission level (can be changed at runtime via YOLO mode)
    pub permission_level: Level,
    /// Queued message waiting to be sent when streaming ends (only one allowed)
    pub queued_message: Option<Vec<ContentBlock>>,
    /// Hook called when user submits input (for saving session, etc.)
    pub on_input_hook: Option<OnInputHook>,
}

impl Model {
    /// Get new history entries collected during this session
    pub fn get_new_history_entries(&self) -> Vec<String> {
        self.input_history[self.initial_history_len..].to_vec()
    }
}
