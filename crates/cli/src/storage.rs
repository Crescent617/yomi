//! CLI-specific storage for session index and input history
//!
//! This storage is separate from the kernel's Storage trait and manages:
//! - Session index: Maps working directories to their last session ID
//! - Input history: Per-directory input history for TUI navigation
//! - File state: Tracks file modification times for stale read detection
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

/// Re-export file state types from kernel
pub use kernel::tools::file_state::FileStateSnapshot;

/// Session metadata for a working directory
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEntry {
    pub session_id: String,
    pub last_accessed: chrono::DateTime<chrono::Utc>,
    pub working_dir: String,
    /// File state snapshot for detecting stale reads
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub file_state: Option<FileStateSnapshot>,
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

    /// Save session metadata with optional file state for a working directory
    ///
    /// Each working directory gets its own file to avoid concurrent access issues.
    /// File: `~/.yomi/appdata/projects/{hash}.json`
    pub async fn save_session(
        &self,
        working_dir: &Path,
        session_id: &str,
        file_state: Option<&FileStateSnapshot>,
    ) -> Result<()> {
        let path = self.proj_meta_path(working_dir);
        let entry = SessionEntry {
            session_id: session_id.to_string(),
            last_accessed: chrono::Utc::now(),
            working_dir: working_dir.to_string_lossy().to_string(),
            file_state: file_state.cloned(),
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

    /// Load session entry for a working directory
    ///
    /// Returns `None` if no session has been recorded for this directory
    pub async fn load_session(&self, working_dir: &Path) -> Result<Option<SessionEntry>> {
        let path = self.proj_meta_path(working_dir);
        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path).await?;
        let entry: SessionEntry = serde_json::from_str(&content)?;
        Ok(Some(entry))
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
    async fn test_session_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path()).unwrap();

        let working_dir = PathBuf::from("/path/to/project");

        // Initially no session
        assert!(storage.load_session(&working_dir).await.unwrap().is_none());

        // Save a session without file state
        storage
            .save_session(&working_dir, "session-123", None)
            .await
            .unwrap();

        // Should be able to retrieve it
        let entry = storage.load_session(&working_dir).await.unwrap().unwrap();
        assert_eq!(entry.session_id, "session-123");
        assert!(entry.file_state.is_none());

        // Update with new session and file state
        let file_state = FileStateSnapshot { entries: vec![] };
        storage
            .save_session(&working_dir, "session-456", Some(&file_state))
            .await
            .unwrap();

        let entry = storage.load_session(&working_dir).await.unwrap().unwrap();
        assert_eq!(entry.session_id, "session-456");
        assert!(entry.file_state.is_some());
    }

    #[tokio::test]
    async fn test_input_history() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path()).unwrap();

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
        let storage = AppStorage::new(temp_dir.path()).unwrap();

        let dir1 = PathBuf::from("/path/to/project1");
        let dir2 = PathBuf::from("/path/to/project2");

        storage
            .save_session(&dir1, "session-1", None)
            .await
            .unwrap();
        storage
            .save_session(&dir2, "session-2", None)
            .await
            .unwrap();

        storage
            .add_input_entry(&dir1, "input for project 1")
            .await
            .unwrap();
        storage
            .add_input_entry(&dir2, "input for project 2")
            .await
            .unwrap();

        let entry1 = storage.load_session(&dir1).await.unwrap().unwrap();
        let entry2 = storage.load_session(&dir2).await.unwrap().unwrap();
        assert_eq!(entry1.session_id, "session-1");
        assert_eq!(entry2.session_id, "session-2");

        let history1 = storage.load_input_history(&dir1).await.unwrap();
        let history2 = storage.load_input_history(&dir2).await.unwrap();

        assert_eq!(history1, vec!["input for project 1"]);
        assert_eq!(history2, vec!["input for project 2"]);
    }
}
