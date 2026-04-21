//! Clipboard utilities for cross-platform copy/paste
//!
//! On Linux and Windows, uses arboard crate.
//! On macOS, uses the native pbcopy command to avoid TCC/permissions issues.

/// Copy text to clipboard
///
/// On macOS, this uses the `pbcopy` command to avoid AppKit/TCC issues.
/// On other platforms, uses the arboard crate.
pub fn copy_text(text: &str) -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        use std::process::{Command, Stdio};

        let mut child = Command::new("pbcopy")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }

        let status = child.wait()?;
        if status.success() {
            Ok(())
        } else {
            Err("pbcopy failed".into())
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        use arboard::Clipboard;

        let mut clipboard = Clipboard::new()?;
        clipboard.set_text(text)?;
        Ok(())
    }
}
