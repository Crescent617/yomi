//! Application types and state definitions

use kernel::client::KernelApi;
use kernel::permission::Level;
use std::time::Instant;
use tokio::sync::mpsc;

use super::event_pump::TaggedEvent;
use crate::{
    id::Id,
    msg::{Msg, UserEvent},
};
use kernel::event::Command;
use kernel::types::ContentBlock;
use std::sync::Arc;
use tuirealm::{application::Application, terminal::CrosstermTerminalAdapter};

// =============================================================================
// Timing Constants
// =============================================================================

/// Frame budget: cap processing time to avoid UI stalls (~120 FPS)
pub const FRAME_BUDGET_MS: u64 = 8;

/// Subscribe timeout before retry (milliseconds)
pub const SUBSCRIBE_TIMEOUT_MS: u64 = 5000;

/// Result type returned by TUI
pub struct TuiResult {
    /// Input history entries collected during this session
    pub input_history: Vec<String>,
    /// Whether to create a new session after exiting
    pub should_create_new_session: bool,
    /// Session ID to switch to on exit (for /sessions command)
    pub switch_to_session: Option<String>,
}

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
    /// Channel to receive events from kernel (via transparent pump)
    pub event_rx: mpsc::Receiver<TaggedEvent>,
    /// Channel to send input to kernel (supports multi-modal content blocks)
    pub input_tx: mpsc::Sender<Vec<ContentBlock>>,
    /// Channel to send control commands (cancel, permission responses, level changes, compaction)
    pub ctrl_tx: mpsc::Sender<Command>,
    /// Kernel API for storage operations (sessions, checkpoints, todos)
    pub(crate) kernel: Arc<dyn KernelApi>,
    /// Current assistant response content (for adding to history when complete)
    pub(crate) current_content: String,
    /// Current assistant thinking (for adding to history when complete)
    pub(crate) current_thinking: String,
    /// When thinking started (for calculating elapsed time)
    pub(crate) thinking_start_time: Option<Instant>,
    /// Application mode - single source of truth
    pub(crate) mode: AppMode,
    /// Pending permission request: (`req_id`, `session_id`) waiting for user confirmation
    pub(crate) pending_permission: Option<(String, String)>,
    /// Pending ask-user request: (`req_id`, `session_id`, remaining questions, collected answers)
    #[allow(clippy::type_complexity)]
    pub(crate) pending_ask_user: Option<(
        String,
        String,
        std::collections::VecDeque<kernel::tools::AskQuestion>,
        std::collections::HashMap<String, String>,
    )>,
    /// Input history for the current working directory (loaded + new)
    pub(crate) input_history: Vec<String>,
    /// New entries collected during this session (not yet persisted)
    pub(crate) new_history_entries: Vec<String>,
    /// Working directory (for file completion and session listing)
    pub(crate) working_dir: std::path::PathBuf,
    /// Current session ID
    pub(crate) session_id: String,
    /// Current permission level (can be changed at runtime via YOLO mode)
    pub(crate) permission_level: Level,
    /// Queued message waiting to be sent when streaming ends (only one allowed)
    pub(crate) queued_message: Option<Vec<ContentBlock>>,
    /// Last known terminal size to detect resize events
    pub(crate) last_terminal_size: (u16, u16),
    /// Pending async clipboard read handle.
    pub(crate) clipboard_handle: Option<tokio::task::JoinHandle<Option<String>>>,
    /// Shared flag set by signal handler to request graceful exit.
    pub(crate) signal_quit: Arc<std::sync::atomic::AtomicBool>,
    /// Channel for async command results to be injected back into the event loop.
    pub(crate) cmd_tx: tokio::sync::mpsc::UnboundedSender<Msg>,
    pub(crate) cmd_rx: tokio::sync::mpsc::UnboundedReceiver<Msg>,
    /// Transparent event pump that hides broadcast churn and auto-reconnects.
    pub(crate) _event_pump: super::event_pump::EventPump,
    /// Display name of the model for this session (resolved from session store, not global config).
    pub(crate) model_name: String,
    /// Context window size for the current session's model (resolved from session store).
    pub(crate) context_window: u32,
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

/// Build picker items for the model switcher (/models).
///
/// The current model is marked with `●` and sorted first; the rest keep the
/// order returned by `list_models` (alphabetical by key).
pub fn model_picker_items(
    models: &[kernel::kernel::ModelInfo],
    current: &str,
) -> Vec<crate::components::PickerItem> {
    use crate::components::PickerItem;

    let mut items: Vec<PickerItem> = Vec::with_capacity(models.len());
    for m in models {
        let is_current = m.name == current;
        let marker = if is_current { "●" } else { " " };
        let label = format!("{marker} {}", m.name);
        let ctx_k = m.context_window / 1000;
        let meta = format!("{} · {} · {}k ctx", m.provider, m.model_id, ctx_k);
        let item = PickerItem::new(m.name.clone(), label).with_meta(meta);
        if is_current {
            items.insert(0, item);
        } else {
            items.push(item);
        }
    }
    items
}

#[cfg(test)]
#[path = "types_test.rs"]
mod tests;
