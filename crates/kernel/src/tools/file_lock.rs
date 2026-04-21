//! Cross-process file locking utilities
//!
//! Provides file locking for edit/write tools to prevent concurrent modifications.
//! Uses stable Rust `std::fs::File` lock methods (available since Rust 1.89).

use std::fs::File;
use std::path::Path;

/// Default timeout for file lock acquisition
pub const DEFAULT_LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Error type for file lock operations
#[derive(Debug)]
pub enum FileLockError {
    /// Failed to open the file
    OpenError(std::io::Error),
    /// Failed to acquire lock
    LockError(std::io::Error),
    /// Lock acquisition timeout
    Timeout,
}

impl std::fmt::Display for FileLockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FileLockError::OpenError(e) => write!(f, "Failed to open file: {e}"),
            FileLockError::LockError(e) => write!(f, "Failed to acquire file lock: {e}"),
            FileLockError::Timeout => {
                write!(
                    f,
                    "Timeout waiting for file lock (another process may be holding it)"
                )
            }
        }
    }
}

impl std::error::Error for FileLockError {}

/// A file lock guard that releases the lock when dropped
pub struct FileLockGuard {
    _file: File,
}

impl Drop for FileLockGuard {
    fn drop(&mut self) {
        // Lock is automatically released when file is closed
        let _ = self._file.unlock();
    }
}

/// Acquire an exclusive (write) lock on a file
///
/// This blocks until the lock is acquired or an error occurs.
/// The lock is automatically released when the returned guard is dropped.
pub async fn lock_exclusive(path: &Path) -> Result<FileLockGuard, FileLockError> {
    let path = path.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let file = File::options()
            .read(true)
            .write(true)
            .create(false)
            .open(&path)
            .map_err(FileLockError::OpenError)?;

        file.lock().map_err(FileLockError::LockError)?;

        Ok(FileLockGuard { _file: file })
    })
    .await
    .map_err(|e| FileLockError::LockError(std::io::Error::other(format!("Task join error: {e}"))))?
}

/// Acquire a shared (read) lock on a file
///
/// This blocks until the lock is acquired or an error occurs.
/// The lock is automatically released when the returned guard is dropped.
pub async fn lock_shared(path: &Path) -> Result<FileLockGuard, FileLockError> {
    let path = path.to_path_buf();

    tokio::task::spawn_blocking(move || {
        let file = File::options()
            .read(true)
            .write(false)
            .open(&path)
            .map_err(FileLockError::OpenError)?;

        file.lock_shared().map_err(FileLockError::LockError)?;

        Ok(FileLockGuard { _file: file })
    })
    .await
    .map_err(|e| FileLockError::LockError(std::io::Error::other(format!("Task join error: {e}"))))?
}

/// Acquire an exclusive (write) lock on a file with timeout
///
/// This will wait up to the specified duration for the lock to become available.
/// Returns `FileLockError::Timeout` if the lock cannot be acquired within the timeout.
pub async fn lock_exclusive_timeout(
    path: &Path,
    timeout: std::time::Duration,
) -> Result<FileLockGuard, FileLockError> {
    tokio::time::timeout(timeout, lock_exclusive(path))
        .await
        .map_err(|_| FileLockError::Timeout)?
}

/// Acquire a shared (read) lock on a file with timeout
///
/// This will wait up to the specified duration for the lock to become available.
/// Returns `FileLockError::Timeout` if the lock cannot be acquired within the timeout.
pub async fn lock_shared_timeout(
    path: &Path,
    timeout: std::time::Duration,
) -> Result<FileLockGuard, FileLockError> {
    tokio::time::timeout(timeout, lock_shared(path))
        .await
        .map_err(|_| FileLockError::Timeout)?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[tokio::test]
    async fn test_exclusive_lock() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "test content").unwrap();

        let _guard = lock_exclusive(temp_file.path()).await.unwrap();
    }

    #[tokio::test]
    async fn test_shared_lock() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "test content").unwrap();

        let _guard = lock_shared(temp_file.path()).await.unwrap();
    }

    #[tokio::test]
    async fn test_lock_guard_releases_on_drop() {
        let mut temp_file = NamedTempFile::new().unwrap();
        writeln!(temp_file, "test content").unwrap();
        let path = temp_file.path().to_path_buf();

        {
            let _guard = lock_exclusive(&path).await.unwrap();
        }

        let _guard2 = lock_exclusive(&path).await.unwrap();
    }
}
