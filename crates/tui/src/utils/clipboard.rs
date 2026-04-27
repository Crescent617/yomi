//! Clipboard utilities using OSC 52 escape sequences
//!
//! Works over SSH by sending escape sequences to the terminal,
//! which then sets the system clipboard.

use base64::Engine;
use std::io::{self, Write};

/// Copy text to clipboard using OSC 52 escape sequence
///
/// Works in SSH sessions as long as the local terminal supports OSC 52.
pub fn copy_text(text: &str) -> io::Result<()> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    let osc_seq = format!("\x1b]52;c;{encoded}\x07");

    let mut stdout = io::stdout();
    stdout.write_all(osc_seq.as_bytes())?;
    stdout.flush()
}
