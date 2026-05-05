//! Application state and modes

use std::time::Instant;

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

/// Tracks streaming content state
#[derive(Debug, Default)]
pub struct StreamingState {
    pub content: String,
    pub thinking: String,
    pub start_time: Option<Instant>,
}

impl StreamingState {
    pub fn clear(&mut self) {
        self.content.clear();
        self.thinking.clear();
        self.start_time = None;
    }

    pub fn append_content(&mut self, text: &str) {
        self.content.push_str(text);
    }

    pub fn append_thinking(&mut self, text: &str) {
        if self.start_time.is_none() {
            self.start_time = Some(Instant::now());
        }
        self.thinking.push_str(text);
    }

    pub fn elapsed_ms(&self) -> Option<u64> {
        self.start_time.map(|start| start.elapsed().as_millis() as u64)
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty() && self.thinking.is_empty()
    }

    /// Format content and thinking for storage/display
    /// Format: `content\x00thinking\x00elapsed_ms` (thinking and `elapsed_ms` optional)
    pub fn format_for_storage(&self) -> String {
        let elapsed_ms = self.elapsed_ms();

        if self.thinking.is_empty() {
            if let Some(ms) = elapsed_ms {
                format!("{}\x00\x00{}", self.content, ms)
            } else {
                self.content.clone()
            }
        } else {
            format!(
                "{}\x00{}\x00{}",
                self.content,
                self.thinking,
                elapsed_ms.unwrap_or(0)
            )
        }
    }
}
