//! Base tool functionality shared across file-based tools
//!
//! Provides common utilities for path resolution and file metadata operations.

use std::path::{Path, PathBuf};

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
}
