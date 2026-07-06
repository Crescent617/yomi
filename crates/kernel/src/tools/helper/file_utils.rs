//! File utility functions shared across tools
//!
//! Provides common utilities for path resolution and file metadata operations.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Maximum concurrent filesystem operations for mtime retrieval
const DEFAULT_MAX_CONCURRENT_MTIME_OPS: usize = 100;

/// Maximum file size (10 MB)
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Get file modification time in milliseconds since epoch.
///
/// Returns `None` if the file metadata cannot be read.
pub async fn get_mtime(path: &Path) -> Option<u64> {
    match tokio::fs::metadata(path).await {
        Ok(metadata) => metadata
            .modified()
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64),
        Err(_) => None,
    }
}

/// Get modification times for multiple files concurrently with limited concurrency.
///
/// This prevents file descriptor exhaustion when processing directories with many files.
/// Uses a semaphore to limit concurrent filesystem operations to `max_concurrent`
/// (default: 100 if None).
///
/// Returns a vector of (path, mtime) pairs. Paths that fail to get mtime are skipped.
pub async fn get_mtimes_concurrent(
    paths: Vec<PathBuf>,
    max_concurrent: Option<usize>,
) -> Vec<(PathBuf, u64)> {
    let limit = max_concurrent.unwrap_or(DEFAULT_MAX_CONCURRENT_MTIME_OPS);
    let semaphore = Arc::new(Semaphore::new(limit));

    let futures: Vec<_> = paths
        .into_iter()
        .map(|path| {
            let sem = Arc::clone(&semaphore);
            async move {
                let _permit = sem.acquire().await.ok()?;
                get_mtime(&path).await.map(|mtime| (path, mtime))
            }
        })
        .collect();

    futures::future::join_all(futures)
        .await
        .into_iter()
        .flatten()
        .collect()
}

#[cfg(test)]
#[path = "file_utils_test.rs"]
mod tests;
