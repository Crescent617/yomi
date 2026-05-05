//! Process-level file locking utilities
//!
//! Provides file locking for edit/write tools to prevent concurrent modifications
//! within the same process. Uses `tokio::sync::Mutex` for async-friendly locking.
//!
//! This approach is chosen over filesystem locking for Windows compatibility:
//! - Windows mandatory file locks prevent re-opening the same file within the same process
//! - Linux advisory locks work fine but have different semantics
//! - Process-level mutex provides consistent behavior across platforms

use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Default timeout for file lock acquisition
pub const DEFAULT_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Process-level file locks to serialize concurrent tool calls targeting the same file.
/// This uses a Mutex instead of filesystem locking for Windows compatibility.
static PROCESS_FILE_LOCKS: std::sync::LazyLock<DashMap<PathBuf, Arc<Mutex<()>>>> =
    std::sync::LazyLock::new(DashMap::new);

/// A guard that holds a process-level lock on a file path.
/// The lock is released when this guard is dropped.
pub struct FileLockGuard {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Error type for file lock operations
#[derive(Debug)]
pub enum FileLockError {
    /// Lock acquisition timeout
    Timeout,
}

impl std::fmt::Display for FileLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileLockError::Timeout => {
                write!(
                    f,
                    "Timeout waiting for file lock (another tool call may be holding it)"
                )
            }
        }
    }
}

impl std::error::Error for FileLockError {}

/// Acquire an exclusive lock on a file path.
///
/// This ensures that concurrent tool calls targeting the same file are serialized.
/// Unlike filesystem locks, this works reliably on Windows because it doesn't
/// prevent the same process from re-opening the file.
///
/// The lock is automatically released when the returned guard is dropped.
pub async fn lock_file(path: &Path) -> FileLockGuard {
    // Get or create the mutex for this path
    let mutex = PROCESS_FILE_LOCKS
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();

    // Acquire the lock
    let guard = mutex.lock_owned().await;

    FileLockGuard { _guard: guard }
}

/// Acquire a file lock with timeout.
///
/// Returns an error if the lock cannot be acquired within the specified duration.
pub async fn lock_file_timeout(
    path: &Path,
    timeout: std::time::Duration,
) -> Result<FileLockGuard, FileLockError> {
    let mutex = PROCESS_FILE_LOCKS
        .entry(path.to_path_buf())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();

    match tokio::time::timeout(timeout, mutex.lock_owned()).await {
        Ok(guard) => Ok(FileLockGuard { _guard: guard }),
        Err(_) => Err(FileLockError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_lock_file_basic() {
        let temp_path = std::env::temp_dir().join("test_lock_basic.txt");

        let guard = lock_file(&temp_path).await;
        drop(guard);

        // Should be able to acquire again after drop
        let _guard2 = lock_file(&temp_path).await;
    }

    #[tokio::test]
    async fn test_lock_file_timeout() {
        let temp_path = std::env::temp_dir().join("test_lock_timeout.txt");

        let _guard = lock_file(&temp_path).await;

        // Try to acquire with a very short timeout - should timeout
        let result = lock_file_timeout(&temp_path, Duration::from_millis(1)).await;
        assert!(matches!(result, Err(FileLockError::Timeout)));
    }

    #[tokio::test]
    async fn test_concurrent_locks_different_paths() {
        let path1 = std::env::temp_dir().join("test_lock_1.txt");
        let path2 = std::env::temp_dir().join("test_lock_2.txt");

        // Different paths should not block each other
        let guard1 = lock_file(&path1).await;
        let guard2 = lock_file(&path2).await;

        drop(guard1);
        drop(guard2);
    }
}
