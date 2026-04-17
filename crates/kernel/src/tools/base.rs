//! Base tool functionality shared across file-based tools
//!
//! Provides common utilities for path resolution and file metadata operations.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Semaphore;

/// Maximum concurrent filesystem operations for mtime retrieval
const DEFAULT_MAX_CONCURRENT_MTIME_OPS: usize = 100;

/// Maximum file size (10 MB)
pub const MAX_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Trait for tools that operate on files within a base directory
#[allow(async_fn_in_trait)]
pub trait FileTool: Send + Sync {
    /// Get the base directory for path resolution
    fn base_dir(&self) -> &Path;

    /// Resolve a relative path against the base directory
    ///
    /// Returns the canonicalized path if possible, otherwise the joined path.
    fn resolve_path(&self, relative: &str) -> PathBuf {
        let path = self.base_dir().join(relative);
        path.canonicalize().unwrap_or(path)
    }

    /// Get file modification time in milliseconds since epoch
    ///
    /// Returns 0 if the file metadata cannot be read.
    async fn get_mtime(&self, path: &Path) -> u64 {
        match tokio::fs::metadata(path).await {
            Ok(metadata) => metadata
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64),
            Err(_) => 0,
        }
    }

    /// Get modification times for multiple files concurrently with limited concurrency
    ///
    /// This prevents file descriptor exhaustion when processing directories with many files.
    /// Uses a semaphore to limit concurrent filesystem operations to `max_concurrent`
    /// (default: 100 if None).
    ///
    /// Returns a vector of (path, mtime) pairs. Paths that fail to get mtime are skipped.
    async fn get_mtimes_concurrent(
        &self,
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
                    let mtime = self.get_mtime(&path).await;
                    Some((path, mtime))
                }
            })
            .collect();

        futures::future::join_all(futures)
            .await
            .into_iter()
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    struct TestTool {
        base_dir: PathBuf,
    }

    impl FileTool for TestTool {
        fn base_dir(&self) -> &Path {
            &self.base_dir
        }
    }

    #[test]
    fn test_resolve_path_relative() {
        let temp = TempDir::new().unwrap();
        let tool = TestTool {
            base_dir: temp.path().to_path_buf(),
        };

        let resolved = tool.resolve_path("test.txt");
        assert!(resolved.ends_with("test.txt"));
    }

    #[tokio::test]
    async fn test_get_mtime_existing_file() {
        let temp = TempDir::new().unwrap();
        let file_path = temp.path().join("test.txt");

        // Create file
        let mut file = std::fs::File::create(&file_path).unwrap();
        file.write_all(b"test").unwrap();
        drop(file);

        let tool = TestTool {
            base_dir: temp.path().to_path_buf(),
        };

        let mtime = tool.get_mtime(&file_path).await;
        assert!(
            mtime > 0,
            "mtime should be greater than 0 for existing file"
        );
    }

    #[tokio::test]
    async fn test_get_mtime_nonexistent_file() {
        let temp = TempDir::new().unwrap();
        let tool = TestTool {
            base_dir: temp.path().to_path_buf(),
        };

        let mtime = tool.get_mtime(Path::new("/nonexistent/file.txt")).await;
        assert_eq!(mtime, 0, "mtime should be 0 for nonexistent file");
    }

    #[tokio::test]
    async fn test_get_mtimes_concurrent() {
        let temp = TempDir::new().unwrap();
        let base_path = temp.path().to_path_buf();

        // Create multiple test files
        let file1 = base_path.join("file1.txt");
        let file2 = base_path.join("file2.txt");
        let file3 = base_path.join("file3.txt");

        std::fs::write(&file1, "content1").unwrap();
        std::fs::write(&file2, "content2").unwrap();
        // file3 doesn't exist

        let tool = TestTool {
            base_dir: base_path,
        };
        let paths = vec![file1.clone(), file2.clone(), file3.clone()];

        let results = tool.get_mtimes_concurrent(paths, None).await;

        // Should have 3 results (including non-existent file with mtime=0)
        assert_eq!(results.len(), 3);
        assert!(results[0].1 > 0); // file1 exists
        assert!(results[1].1 > 0); // file2 exists
        assert_eq!(results[2].1, 0); // file3 doesn't exist, mtime=0
    }

    #[tokio::test]
    async fn test_get_mtimes_concurrent_with_limit() {
        let temp = TempDir::new().unwrap();
        let base_path = temp.path().to_path_buf();

        // Create test files
        for i in 0..10 {
            let file = base_path.join(format!("file{i}.txt"));
            std::fs::write(&file, format!("content{i}")).unwrap();
        }

        let tool = TestTool {
            base_dir: base_path.clone(),
        };
        let paths: Vec<PathBuf> = (0..10)
            .map(|i| base_path.join(format!("file{i}.txt")))
            .collect();

        // Use a low concurrency limit
        let results = tool.get_mtimes_concurrent(paths, Some(2)).await;

        // Should have 10 results (all files exist)
        assert_eq!(results.len(), 10);
    }
}
