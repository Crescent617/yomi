//! Clipboard utilities for cross-platform copy/paste
//!
//! Uses arboard with wayland-data-control feature for native Wayland support.

use arboard::Clipboard;

/// Copy text to clipboard
pub fn copy_text(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut clipboard = Clipboard::new()?;
    clipboard.set_text(text)?;
    Ok(())
}
