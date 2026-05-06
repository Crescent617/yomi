//! Process-level key locking utilities
//!
//! Provides key-based locking for tools to prevent concurrent modifications
//! within the same process. Uses `tokio::sync::Mutex` for async-friendly locking.
//!
//! This approach is chosen over filesystem locking for Windows compatibility:
//! - Windows mandatory file locks prevent re-opening the same file within the same process
//! - Linux advisory locks work fine but have different semantics
//! - Process-level mutex provides consistent behavior across platforms

use dashmap::DashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Default timeout for key lock acquisition
pub const DEFAULT_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Process-level key locks to serialize concurrent tool calls targeting the same key.
/// This uses a Mutex instead of filesystem locking for Windows compatibility.
static G_LOCKS: std::sync::LazyLock<DashMap<String, Arc<Mutex<()>>>> =
    std::sync::LazyLock::new(DashMap::new);

/// A guard that holds a process-level lock on a key.
/// The lock is released when this guard is dropped.
pub struct GLockGuard {
    _guard: tokio::sync::OwnedMutexGuard<()>,
}

/// Error type for key lock operations
#[derive(Debug)]
pub enum GLockError {
    /// Lock acquisition timeout
    Timeout,
}

impl std::fmt::Display for GLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GLockError::Timeout => {
                write!(
                    f,
                    "Timeout waiting for key lock (another tool call may be holding it)"
                )
            }
        }
    }
}

impl std::error::Error for GLockError {}

/// Acquire an exclusive lock on a key.
///
/// This ensures that concurrent tool calls targeting the same key are serialized.
/// Unlike filesystem locks, this works reliably on Windows because it doesn't
/// prevent the same process from re-opening files.
///
/// The lock is automatically released when the returned guard is dropped.
pub async fn g_lock(key: impl Into<String>) -> GLockGuard {
    let key = key.into();
    // Get or create the mutex for this key
    let mutex = G_LOCKS
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();

    // Acquire the lock
    let guard = mutex.lock_owned().await;

    GLockGuard { _guard: guard }
}

/// Acquire a key lock with timeout.
///
/// Returns an error if the lock cannot be acquired within the specified duration.
pub async fn g_lock_timeout(
    key: impl Into<String>,
    timeout: std::time::Duration,
) -> Result<GLockGuard, GLockError> {
    let key = key.into();
    let mutex = G_LOCKS
        .entry(key)
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone();

    match tokio::time::timeout(timeout, mutex.lock_owned()).await {
        Ok(guard) => Ok(GLockGuard { _guard: guard }),
        Err(_) => Err(GLockError::Timeout),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn test_lock_key_basic() {
        let key = "test_lock_basic";

        let guard = g_lock(key).await;
        drop(guard);

        // Should be able to acquire again after drop
        let _guard2 = g_lock(key).await;
    }

    #[tokio::test]
    async fn test_lock_key_timeout() {
        let key = "test_lock_timeout";

        let _guard = g_lock(key).await;

        // Try to acquire with a very short timeout - should timeout
        let result = g_lock_timeout(key, Duration::from_millis(1)).await;
        assert!(matches!(result, Err(GLockError::Timeout)));
    }

    #[tokio::test]
    async fn test_concurrent_locks_different_keys() {
        let key1 = "test_key_1";
        let key2 = "test_key_2";

        // Different keys should not block each other
        let guard1 = g_lock(key1).await;
        let guard2 = g_lock(key2).await;

        drop(guard1);
        drop(guard2);
    }

    #[tokio::test]
    async fn test_lock_key_string_owned() {
        let key = "test_owned_string".to_string();

        let guard = g_lock(key.clone()).await;
        drop(guard);

        // Should be able to acquire again with the same key
        let _guard2 = g_lock(key).await;
    }
}
