//! CLI-specific storage for session index and input history
//!
//! This storage is separate from the kernel's Storage trait and manages:
//! - Session index: Maps working directories to their last session ID
//! - Input history: Per-directory input history for TUI navigation
//!
//! Data is stored in `~/.yomi/appdata/` with per-directory hashed filenames
//! to avoid concurrent access issues.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

const APP_DATA_DIR: &str = "app_data";
const PROJ_INDEX_DIR: &str = "projects";
const DEFAULT_MAX_HISTORY: usize = 1000;

/// Session metadata for a working directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub session_id: String,
    pub last_accessed: chrono::DateTime<chrono::Utc>,
    pub working_dir: String,
}

/// CLI-specific storage for session index and input history
#[derive(Debug, Clone)]
pub struct AppStorage {
    base_dir: PathBuf,
}

impl AppStorage {
    /// Create new `AppStorage` at the given base directory
    ///
    /// The base directory is typically `~/.yomi/`, data will be stored in `~/.yomi/appdata/`
    pub fn new(base_dir: impl AsRef<std::path::Path>) -> Result<Self> {
        let app_data_dir = base_dir.as_ref().join(APP_DATA_DIR);

        // Create subdirectories
        std::fs::create_dir_all(&app_data_dir).with_context(|| {
            format!(
                "Failed to create appdata directory: {}",
                app_data_dir.display()
            )
        })?;
        std::fs::create_dir_all(app_data_dir.join(PROJ_INDEX_DIR)).with_context(|| {
            format!(
                "Failed to create sessions directory: {}",
                app_data_dir.join(PROJ_INDEX_DIR).display()
            )
        })?;

        Ok(Self {
            base_dir: app_data_dir,
        })
    }

    /// Hash a working directory path to a filename using MD5
    fn hash_path(working_dir: &Path) -> String {
        let path_str = working_dir.to_string_lossy();
        let hash = md5::compute(path_str.as_bytes());
        format!("{hash:x}")
    }

    fn proj_meta_path(&self, working_dir: &Path) -> PathBuf {
        let hash = Self::hash_path(working_dir);
        self.base_dir
            .join(PROJ_INDEX_DIR)
            .join(format!("{hash}.json"))
    }

    fn input_hist_path(&self, working_dir: &Path) -> PathBuf {
        let hash = Self::hash_path(working_dir);
        self.base_dir
            .join(PROJ_INDEX_DIR)
            .join(format!("{hash}.input_hist.jsonl"))
    }

    /// Record a session for a working directory
    ///
    /// Each working directory gets its own file to avoid concurrent access issues.
    /// File: `~/.yomi/appdata/sessions/{hash}.json`
    pub async fn record_session(&self, working_dir: &Path, session_id: &str) -> Result<()> {
        let path = self.proj_meta_path(working_dir);
        let entry = SessionEntry {
            session_id: session_id.to_string(),
            last_accessed: chrono::Utc::now(),
            working_dir: working_dir.to_string_lossy().to_string(),
        };

        // Atomic write
        let temp_path = path.with_extension("tmp");
        let content = serde_json::to_string_pretty(&entry)?;
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(content.as_bytes()).await?;
        file.flush().await?;
        drop(file);
        fs::rename(&temp_path, &path).await?;

        Ok(())
    }

    /// Get the last session ID for a working directory
    ///
    /// Returns `None` if no session has been recorded for this directory
    pub async fn get_last_session(&self, working_dir: &Path) -> Result<Option<String>> {
        let path = self.proj_meta_path(working_dir);
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path).await?;
        let entry: SessionEntry = serde_json::from_str(&content)?;
        Ok(Some(entry.session_id))
    }

    /// Load input history for a working directory
    ///
    /// Returns a vector of input strings, oldest first
    /// File: `~/.yomi/appdata/history/{hash}.jsonl`
    pub async fn load_input_history(&self, working_dir: &Path) -> Result<Vec<String>> {
        let path = self.input_hist_path(working_dir);
        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&path).await?;
        let mut entries = Vec::new();
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let entry: String = serde_json::from_str(line)?;
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Add an entry to input history
    ///
    /// Empty inputs are ignored. Duplicate consecutive entries are not added.
    /// History is trimmed to `DEFAULT_MAX_HISTORY` entries.
    pub async fn add_input_entry(&self, working_dir: &Path, input: &str) -> Result<()> {
        if input.trim().is_empty() {
            return Ok(());
        }

        let path = self.input_hist_path(working_dir);
        let mut entries = self.load_input_history(working_dir).await?;

        // Avoid duplicates at the end
        if entries.last() == Some(&input.to_string()) {
            return Ok(());
        }

        entries.push(input.to_string());

        // Trim to max size
        if entries.len() > DEFAULT_MAX_HISTORY {
            entries = entries.split_off(entries.len() - DEFAULT_MAX_HISTORY);
        }

        // Atomic write
        let temp_path = path.with_extension("tmp");
        let mut file = fs::File::create(&temp_path).await?;
        for entry in &entries {
            let line = serde_json::to_string(entry)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
        drop(file);
        fs::rename(&temp_path, &path).await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_session_index() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path().to_path_buf()).unwrap();

        let working_dir = PathBuf::from("/path/to/project");

        // Initially no session
        assert!(storage
            .get_last_session(&working_dir)
            .await
            .unwrap()
            .is_none());

        // Record a session
        storage
            .record_session(&working_dir, "session-123")
            .await
            .unwrap();

        // Should be able to retrieve it
        let session_id = storage.get_last_session(&working_dir).await.unwrap();
        assert_eq!(session_id, Some("session-123".to_string()));

        // Update with new session
        storage
            .record_session(&working_dir, "session-456")
            .await
            .unwrap();

        let session_id = storage.get_last_session(&working_dir).await.unwrap();
        assert_eq!(session_id, Some("session-456".to_string()));
    }

    #[tokio::test]
    async fn test_input_history() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path().to_path_buf()).unwrap();

        let working_dir = PathBuf::from("/path/to/project");

        // Initially empty
        let history = storage.load_input_history(&working_dir).await.unwrap();
        assert!(history.is_empty());

        // Add some entries
        storage
            .add_input_entry(&working_dir, "hello")
            .await
            .unwrap();
        storage
            .add_input_entry(&working_dir, "world")
            .await
            .unwrap();

        let history = storage.load_input_history(&working_dir).await.unwrap();
        assert_eq!(history, vec!["hello", "world"]);

        // Duplicate should not be added
        storage
            .add_input_entry(&working_dir, "world")
            .await
            .unwrap();
        let history = storage.load_input_history(&working_dir).await.unwrap();
        assert_eq!(history, vec!["hello", "world"]);

        // Empty should be ignored
        storage.add_input_entry(&working_dir, "").await.unwrap();
        storage.add_input_entry(&working_dir, "   ").await.unwrap();
        let history = storage.load_input_history(&working_dir).await.unwrap();
        assert_eq!(history, vec!["hello", "world"]);
    }

    #[tokio::test]
    async fn test_different_working_dirs() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path().to_path_buf()).unwrap();

        let dir1 = PathBuf::from("/path/to/project1");
        let dir2 = PathBuf::from("/path/to/project2");

        storage.record_session(&dir1, "session-1").await.unwrap();
        storage.record_session(&dir2, "session-2").await.unwrap();

        storage
            .add_input_entry(&dir1, "input for project 1")
            .await
            .unwrap();
        storage
            .add_input_entry(&dir2, "input for project 2")
            .await
            .unwrap();

        assert_eq!(
            storage.get_last_session(&dir1).await.unwrap(),
            Some("session-1".to_string())
        );
        assert_eq!(
            storage.get_last_session(&dir2).await.unwrap(),
            Some("session-2".to_string())
        );

        let history1 = storage.load_input_history(&dir1).await.unwrap();
        let history2 = storage.load_input_history(&dir2).await.unwrap();

        assert_eq!(history1, vec!["input for project 1"]);
        assert_eq!(history2, vec!["input for project 2"]);
    }
}
