//! Messages for TUI application

use crate::components::fuzzy_picker::PickerItem;
use crate::components::info_bar::Notification;
use kernel::checkpoint::Checkpoint;
use kernel::event::Event as AppEvent;
use kernel::types::ContentBlock;

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

    // User input (supports multi-modal content blocks)
    InputSubmit(Vec<ContentBlock>),
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
    ClearQueuedMessage, // Clear queued message (ESC when queue is not empty)

    // Notification message with level and duration (shown in InfoBar)
    Notification(Notification),

    // Browse mode (readonly like less)
    ToggleBrowseMode,
    PageHalfUp,
    PageHalfDown,
    GoToTop,    // 'g' - go to first line
    GoToBottom, // 'G' - go to last line

    // Toggle YOLO mode (Dangerous permission level)
    ToggleYoloMode,

    // Dialog results
    DialogSelected(usize),     // Selected option index
    DialogCustomInput(String), // User entered a custom free-text answer
    DialogCancelled,           // Dialog was cancelled

    // Slash commands (String = raw user input, e.g. "/goal do stuff")
    CommandNew,                      // /new - create new session
    CommandGoal(String),             // /goal <description> - start autonomous goal mode
    CommandGoalStop,                 // /goal:stop - stop autonomous goal mode
    CommandYolo,                     // /yolo - toggle yolo mode
    CommandBrowse,                   // /browse - toggle browse mode
    CommandCompact,                  // /compact - force message compaction
    CommandReload,                   // /reload - reload skills and hooks in daemon
    CommandSteer(Vec<ContentBlock>), // /steer <content> - inject steer message before next streaming
    CommandHelp,                     // /help - show help dialog
    CommandSessions,                 // /sessions - switch session
    CommandTodos,                    // /todos - toggle todo list visibility

    // Session picker
    SessionSelected(String), // User selected a session to switch to
    CloseSessionPicker,      // Close session picker without selection

    // Suspend process to background (Ctrl-Z)
    Suspend,

    // Async clipboard read
    ReadClipboard,
    ClipboardText(String),

    // History: raw submitted text + the actual message to process.
    // Emitted whenever the user presses Enter; the raw text goes into input history.
    InputEntry(String, Box<Msg>),

    // History picker (C-r)
    ShowHistoryPicker,       // Show fuzzy history search
    HistorySelected(String), // User selected a history item
    CloseHistoryPicker,      // Close history picker without selection

    // Help dialog
    CloseHelpDialog, // Close the help dialog

    // Checkpoint commands
    CommandRewind,                            // /rewind - show checkpoint picker
    CommandUndo, // /undo - undo last turn (rewind to latest checkpoint)
    CheckpointSelected(String, RewindTarget), // message_id, target (Conversation/Files/Both)
    CloseCheckpointPicker, // Close checkpoint picker without selection

    // Async command results
    SessionList(Vec<PickerItem>),
    CheckpointList(Vec<Checkpoint>),
}

/// Target for rewinding (mirrors `kernel::checkpoint::RewindTarget`)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RewindTarget {
    Conversation,
    Files,
    Both,
}

impl From<AppEvent> for Msg {
    fn from(event: AppEvent) -> Self {
        Self::AppEvent(event)
    }
}
