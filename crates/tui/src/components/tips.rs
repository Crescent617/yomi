//! Tips shown on startup

use kernel::{const_concat, env_names};

/// Tips shown on startup
pub const TIPS: &[&str] = &[
    "Use Ctrl+O to enter browse mode",
    "Use Ctrl+C twice to exit",
    "Use Ctrl+P/Ctrl+N/↑/↓ to navigate history",
    "Use ESC to interrupt current response",
    "Use Ctrl+V to paste image in clipboard",
    "Type /new to start a new session",
    "Type /sessions to switch between sessions",
    "Type /yolo to toggle YOLO mode",
    const_concat!(
        "Use env var ",
        env_names::CONTEXT_WINDOW,
        " to set llm context window"
    ),
];

/// Get a random tip
pub fn get_random_tip() -> &'static str {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    TIPS[(now as usize) % TIPS.len()]
}
