//! Messages for TUI application

use kernel::event::Event as AppEvent;

/// User event type for tuirealm
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserEvent {
    AppEvent(AppEvent),
    Tick,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Msg {
    // App events from kernel
    AppEvent(AppEvent),

    // Stream events
    StreamText(String),
    StreamThinking(String),
    StreamComplete,
    StreamError(String),

    // Tool events
    ToolStarted(String),
    ToolOutput(String),
    ToolError(String),

    // User input
    InputSubmit(String),
    InputChanged(String),

    // Scrolling
    ScrollUp,
    ScrollDown,
    ToggleThinking,
    ToggleExpandAll,

    // UI
    Tick,
    Quit,
    Redraw,

    // Request control
    CancelRequest,

    // Status bar message with duration in milliseconds
    ShowStatusMessage(String, u64),

    // Browse mode (readonly like less)
    ToggleBrowseMode,
    PageUp,
    PageDown,
    GoToTop,    // 'g' - go to first line
    GoToBottom, // 'G' - go to last line

    // Toggle YOLO mode (Dangerous permission level)
    ToggleYoloMode,

    // Dialog results
    DialogSelected(usize), // Selected option index
    DialogCancelled,       // Dialog was cancelled

    // Slash commands
    CommandNew,             // /new - create new session
    CommandClear,           // /clear - clear history
    CommandYolo,            // /yolo - toggle yolo mode
    CommandBrowse,          // /browse - toggle browse mode
    CommandUnknown(String), // Unknown command
}

impl From<AppEvent> for Msg {
    fn from(event: AppEvent) -> Self {
        Self::AppEvent(event)
    }
}
