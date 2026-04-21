//! Utility functions for the TUI crate

pub mod clipboard;
pub mod text;

// Re-export from kernel for consistency
pub use kernel::utils::{strs, tokens};
