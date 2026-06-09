//! Desktop notification utilities using notify-rust
//!
//! Cross-platform notifications via OS-native APIs:
//! - macOS: `NSUserNotificationCenter`
//! - Linux: D-Bus notification daemon
//! - Windows: `WinRT` toast notifications
//!
//! Falls back to OSC escape sequences if notify-rust fails.

use std::io::{self, Write};
use std::sync::OnceLock;

static IN_SSH: OnceLock<bool> = OnceLock::new();

/// Check if running inside an SSH session (cached at first call).
fn in_ssh() -> bool {
    *IN_SSH.get_or_init(|| {
        std::env::var("SSH_CONNECTION").is_ok()
            || std::env::var("SSH_CLIENT").is_ok()
            || std::env::var("SSH_TTY").is_ok()
    })
}

/// Check if running inside tmux.
fn in_tmux() -> bool {
    std::env::var("TMUX").is_ok()
}

/// Check if running inside neovim (terminal, job, or child process).
fn in_nvim() -> bool {
    std::env::var("NVIM").is_ok() || std::env::var("NVIM_LISTEN_ADDRESS").is_ok()
}

/// Wrap a sequence for tmux DCS passthrough.
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
fn notify_osc9_raw(message: &str) -> io::Result<()> {
    if in_nvim() {
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
fn notify_osc777_raw(title: &str, message: &str) -> io::Result<()> {
    if in_nvim() {
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

/// Fallback OSC notification
fn notify_osc(title: &str, message: &str) -> io::Result<()> {
    if notify_osc777_raw(title, message).is_ok() {
        return Ok(());
    }
    let full_message = format!("{title}: {message}");
    notify_osc9_raw(&full_message)
}

/// Send desktop notification via OS-native APIs.
///
/// Uses notify-rust for cross-platform support (macOS `NSUserNotificationCenter`,
/// Linux D-Bus, Windows `WinRT`). Falls back to OSC escape sequences if the
/// native notification system is unavailable.
pub fn send_desktop_notification(title: &str, message: &str) {
    // Only send if desktop notifications are enabled
    if !crate::feature_gates().desktop_notify {
        return;
    }

    // In SSH sessions there is no desktop notification bus, so skip
    // notify-rust entirely to avoid noisy failures on the TUI stdout.
    if in_ssh() {
        let _ = notify_osc(title, message);
        return;
    }

    // Try native notification via notify-rust
    if notify_rust::Notification::new()
        .summary(title)
        .body(message)
        .show()
        .is_ok()
    {
        return;
    }

    // Fallback to OSC for terminals that support it
    let _ = notify_osc(title, message);
}
