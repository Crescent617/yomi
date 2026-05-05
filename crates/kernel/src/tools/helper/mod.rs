//! Helper utilities for tools
//!
//! This module provides shared low-level functionality used by multiple tools:
//! - File operations (mtime, locking, state tracking)
//! - Text truncation

pub mod file_lock;
pub mod file_state;
pub mod file_utils;
pub mod truncate;

// Re-export commonly used items
pub use file_lock::{
    lock_exclusive, lock_exclusive_timeout, lock_shared, lock_shared_timeout, FileLockError,
    FileLockGuard, DEFAULT_LOCK_TIMEOUT,
};
pub use file_state::FileStateStore;
pub use file_utils::{get_mtime, get_mtimes_concurrent, MAX_FILE_SIZE};
pub use truncate::{
    maybe_truncate_output, truncate_output, truncate_with_message, TRUNCATION_MESSAGE,
};

// Constants used across tools
pub const MAX_TOOL_OUTPUT_LENGTH: usize = 20_000;
