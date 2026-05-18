//! Desktop notification utilities using OSC escape sequences
//!
//! Supports OSC 9 (iTerm2, `WezTerm`, Windows Terminal) and OSC 777 (kitty, foot).
//! Works inside tmux with passthrough sequences.
//!
//! # Limitations
//! - tmux >= 3.3 requires `allow-passthrough on`.
//! - nvim `:terminal` intercepts all OSC/DCS sequences; notifications silently
//!   fail there and fall back to TUI info-bar.

use std::io::{self, Write};

/// Check if running inside tmux.
fn in_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

/// Check if running inside neovim (terminal, job, or child process).
///
/// nvim's `:terminal` (libvterm) swallows OSC/DCS sequences and never forwards
/// them to the outer terminal, so OSC-based desktop notifications are impossible.
fn in_nvim() -> bool {
    std::env::var("NVIM").is_ok() || std::env::var("NVIM_LISTEN_ADDRESS").is_ok()
}

/// Wrap a sequence for tmux DCS passthrough.
///
/// tmux intercepts OSC sequences, so we need to wrap them in DCS passthrough:
/// `ESC P tmux ; <sequence> ESC \`
fn tmux_wrap(seq: &str) -> String {
    format!("\x1bPtmux;\x1b{seq}\x1b\\")
}

/// Send raw sequence to stdout.
fn send_raw(seq: &str) -> io::Result<()> {
    let mut stdout = io::stdout();
    stdout.write_all(seq.as_bytes())?;
    stdout.flush()
}

/// Send OSC 9 notification (iTerm2, `WezTerm`, Windows Terminal)
///
/// Format: `ESC ] 9 ; <message> BEL`
fn notify_osc9_raw(message: &str) -> io::Result<()> {
    if in_nvim() {
        // nvim :terminal swallows OSC sequences; rely on TUI info-bar instead.
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "OSC notifications blocked by nvim terminal",
        ));
    }

    let osc_seq = format!("\x1b]9;{message}\x07");

    if in_tmux() {
        send_raw(&tmux_wrap(&osc_seq))
    } else {
        send_raw(&osc_seq)
    }
}

/// Send OSC 777 notification (kitty, foot)
///
/// Format: `ESC ] 777 ; notify ; <title> ; <message> BEL`
fn notify_osc777_raw(title: &str, message: &str) -> io::Result<()> {
    if in_nvim() {
        // nvim :terminal swallows OSC sequences; rely on TUI info-bar instead.
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "OSC notifications blocked by nvim terminal",
        ));
    }

    let osc_seq = format!("\x1b]777;notify;{title};{message}\x07");

    if in_tmux() {
        send_raw(&tmux_wrap(&osc_seq))
    } else {
        send_raw(&osc_seq)
    }
}

/// Send desktop notification using best available OSC method
///
/// Tries OSC 777 first (kitty/foot), then OSC 9 (iTerm2/WezTerm/Windows Terminal)
pub fn notify_osc(title: &str, message: &str) -> io::Result<()> {
    // Try OSC 777 first (more features: title support)
    if notify_osc777_raw(title, message).is_ok() {
        return Ok(());
    }

    // Fallback to OSC 9 (combine title and message)
    let full_message = format!("{title}: {message}");
    notify_osc9_raw(&full_message)
}

/// Send OSC 9 notification (public API)
pub fn notify_osc9(message: &str) -> io::Result<()> {
    notify_osc9_raw(message)
}

/// Send OSC 777 notification (public API)
pub fn notify_osc777(title: &str, message: &str) -> io::Result<()> {
    notify_osc777_raw(title, message)
}

/// Send desktop notification via OSC
///
/// Works over SSH and inside tmux (with `allow-passthrough on`).
pub fn send_desktop_notification(title: &str, message: &str) {
    let _ = notify_osc(title, message);
}
