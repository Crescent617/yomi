//! Helper utilities for tools
//!
//! This module provides shared low-level functionality used by multiple tools:
//! - File operations (mtime, locking, state tracking)
//! - Text truncation

pub mod file_state;
pub mod file_utils;
pub mod g_lock;
pub mod truncate;

// Re-export commonly used items
pub use file_state::FileStateStore;
pub use file_utils::{get_mtime, get_mtimes_concurrent, MAX_FILE_SIZE};
pub use g_lock::{g_lock, g_lock_timeout, GLockError, GLockGuard, DEFAULT_LOCK_TIMEOUT};
pub use truncate::{
    maybe_truncate_output, truncate_output, truncate_with_message, MAX_TOOL_OUTPUT_LENGTH,
    TRUNCATION_MESSAGE,
};


