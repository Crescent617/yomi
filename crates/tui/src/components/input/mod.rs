//! Input component module

mod completion;
mod component;
mod editor;
mod handlers;
mod history;
mod paste;

pub use component::InputComponent;
pub use editor::{InputEditor, InputSelection, MouseEventResult};

use std::time::{SystemTime, UNIX_EPOCH};

/// Available slash commands with descriptions
pub const SLASH_COMMANDS: &[(&str, &str)] = &[
    ("/new", "Create new session"),
    (
        "/goal",
        "<description> Start goal mode with optional description",
    ),
    ("/goal:stop", "Stop goal mode"),
    ("/todos", "Toggle todo list visibility"),
    ("/yolo", "Toggle YOLO mode (auto-approve all tools)"),
    ("/browse", "Toggle browse mode"),
    ("/sessions", "Switch to another session"),
    ("/rewind", "Restore conversation/file checkpoint"),
    ("/undo", "Undo last turn"),
    ("/compact", "Force message compaction"),
    ("/reload", "Reload skills and hooks from disk"),
    ("/help", "Show keyboard shortcuts help"),
];

/// Random tips to show in the input placeholder
const INPUT_TIPS: &[&str] = &[
    "Shift+Enter newline · Enter send",
    "Ctrl+O browse mode · /new session",
    "Ctrl+C clear · double-click select",
    "Ctrl+V paste image · @ mention file",
    "Ctrl+P/N history · Ctrl+R search",
    "Ctrl+W delete word · Ctrl+U kill line",
    "Alt+B/F word jump · mouse drag select",
    "Ctrl+Z suspend · fg to resume",
];

/// Get a random tip based on current time
fn random_tip() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let idx = (now as usize) % INPUT_TIPS.len();
    INPUT_TIPS[idx].to_string()
}
