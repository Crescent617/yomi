//! Messages for TUI application

use kernel::event::Event as AppEvent;

/// User event type for tuirealm
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserEvent {
    AppEvent(AppEvent),
    Tick,
}

#[derive(Debug, Clone, PartialEq)]
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
}

impl From<AppEvent> for Msg {
    fn from(event: AppEvent) -> Self {
        Msg::AppEvent(event)
    }
}
