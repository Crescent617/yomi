//! Helper utilities for tools
//!
//! This module provides shared low-level functionality used by multiple tools:
//! - File operations (mtime, locking, state tracking)
//! - Text truncation

pub mod file_state;
pub mod file_utils;
pub mod truncate;

// Re-export commonly used items
pub use file_state::FileStateStore;
pub use file_utils::{get_mtime, get_mtimes_concurrent, MAX_FILE_SIZE};
pub use truncate::{
    maybe_truncate_output, truncate_output, truncate_with_message, DEFAULT_MAX_TOOL_OUTPUT_LENGTH,
    TRUNCATION_MESSAGE,
};
