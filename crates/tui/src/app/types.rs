//! Application types and state definitions

use kernel::permissions::Level;
use std::time::Instant;
use tokio::sync::{broadcast, mpsc};

use crate::{
    id::Id,
    msg::{Msg, UserEvent},
};
use kernel::event::{ControlCommand, Event};
use kernel::types::{ContentBlock, Message};
use std::sync::Arc;
use tuirealm::{application::Application, terminal::CrosstermTerminalAdapter};

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

/// Feature gates for optional functionality
#[derive(Debug, Clone, Copy, Default)]
pub struct FeatureGates {
    /// Enable desktop notifications when agent loop completes
    pub desktop_notify: bool,
}

impl FeatureGates {
    pub fn from_env() -> Self {
        let var_name = format!("{}DESKTOP_NOTIFY", kernel::ENV_PREFIX);
        let desktop_notify = kernel::utils::env::env_bool_opt(&var_name).unwrap_or(true);
        if desktop_notify {
            tracing::debug!("Desktop notifications enabled ({var_name})");
        }
        Self { desktop_notify }
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
pub enum StreamingStatus {
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

/// Main application model
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
    pub(crate) session_store: Arc<dyn kernel::storage::SessionStore>,
    /// Current assistant response content (for adding to history when complete)
    pub(crate) current_content: String,
    /// Current assistant thinking (for adding to history when complete)
    pub(crate) current_thinking: String,
    /// When thinking started (for calculating elapsed time)
    pub(crate) thinking_start_time: Option<Instant>,
    /// Application mode - single source of truth
    pub(crate) mode: AppMode,
    /// Pending permission request (`req_id`) waiting for user confirmation
    pub(crate) pending_permission: Option<String>,
    /// Input history for the current working directory (loaded + new)
    pub(crate) input_history: Vec<String>,
    /// Initial history length (to identify new entries on exit)
    pub(crate) initial_history_len: usize,
    /// Working directory (for file completion and session listing)
    pub(crate) working_dir: std::path::PathBuf,
    /// Session messages to display on startup (for resumed sessions)
    pub(crate) session_messages: Vec<Message>,
    /// Current session ID
    pub(crate) session_id: String,
    /// Current permission level (can be changed at runtime via YOLO mode)
    pub(crate) permission_level: Level,
    /// Queued message waiting to be sent when streaming ends (only one allowed)
    pub(crate) queued_message: Option<Vec<ContentBlock>>,
    /// Hook called when user submits input (for saving session, etc.)
    pub(crate) on_input_hook: Option<OnInputHook>,
}

/// Format a session ID for display, truncating long IDs with ellipsis.
/// Uses character-based slicing for Unicode safety.
pub fn format_short_id(id: &str) -> String {
    use crate::utils::text::substring_by_chars;

    let char_count = id.chars().count();
    if char_count > 12 {
        let start = substring_by_chars(id, 0, 6);
        let end = substring_by_chars(id, char_count.saturating_sub(4), char_count);
        format!("{start}...{end}")
    } else {
        id.to_string()
    }
}
