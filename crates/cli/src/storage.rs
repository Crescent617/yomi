//! CLI-specific storage for session index and input history
//!
//! This storage is separate from the kernel's storage and manages:
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
const DEFAULT_MAX_HISTORY: usize = 2000;

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
    pub fn new(base_dir: impl AsRef<Path>) -> Result<Self> {
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

    /// Update only the `session_id` for a working directory
    pub async fn update_last_session(&self, working_dir: &Path, session_id: &str) -> Result<()> {
        let entry = SessionEntry {
            session_id: session_id.to_string(),
            last_accessed: chrono::Utc::now(),
            working_dir: working_dir.to_string_lossy().to_string(),
        };
        self.write_entry(working_dir, &entry).await
    }

    /// Save session metadata for a working directory
    pub async fn save_session(&self, working_dir: &Path, session_id: &str) -> Result<()> {
        let entry = SessionEntry {
            session_id: session_id.to_string(),
            last_accessed: chrono::Utc::now(),
            working_dir: working_dir.to_string_lossy().to_string(),
        };
        self.write_entry(working_dir, &entry).await
    }

    async fn write_entry(&self, working_dir: &Path, entry: &SessionEntry) -> Result<()> {
        let path = self.proj_meta_path(working_dir);
        let temp_path = path.with_extension("tmp");
        let content = serde_json::to_string_pretty(entry)?;
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

    /// Add an entry to input history (append-only for performance)
    ///
    /// Empty inputs are ignored. Call `dedup_input_history` on exit to remove duplicates.
    /// History is trimmed to `DEFAULT_MAX_HISTORY` entries with hysteresis.
    pub async fn add_input_entry(&self, working_dir: &Path, input: &str) -> Result<()> {
        if input.trim().is_empty() {
            return Ok(());
        }

        let path = self.input_hist_path(working_dir);
        let needs_trim = Self::count_entries(&path).await? >= DEFAULT_MAX_HISTORY;

        if needs_trim {
            let mut entries = self.load_input_history(working_dir).await?;
            let keep_count = DEFAULT_MAX_HISTORY / 2;
            if entries.len() > keep_count {
                entries = entries.split_off(entries.len() - keep_count);
            }
            entries.push(input.to_string());
            self.write_history(&path, &entries).await?;
        } else {
            Self::append_entry(&path, input).await?;
        }

        Ok(())
    }

    /// Remove duplicate entries, keeping only the latest occurrence of each
    pub async fn dedup_input_history(&self, working_dir: &Path) -> Result<()> {
        let path = self.input_hist_path(working_dir);
        if !path.exists() {
            return Ok(());
        }

        let entries = self.load_input_history(working_dir).await?;
        if entries.len() < 2 {
            return Ok(());
        }

        // Dedup: process from end to keep latest occurrence, then reverse back
        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<String> = entries
            .into_iter()
            .rev()
            .filter(|e| seen.insert(e.clone()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        self.write_history(&path, &deduped).await?;
        Ok(())
    }

    async fn count_entries(path: &Path) -> Result<usize> {
        if !path.exists() {
            return Ok(0);
        }
        let content = fs::read_to_string(path).await?;
        Ok(content.lines().filter(|l| !l.trim().is_empty()).count())
    }

    async fn append_entry(path: &Path, input: &str) -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .await?;
        let line = serde_json::to_string(input)?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;
        Ok(())
    }

    async fn write_history(&self, path: &Path, entries: &[String]) -> Result<()> {
        let temp_path = path.with_extension("tmp");
        let mut file = fs::File::create(&temp_path).await?;
        for entry in entries {
            let line = serde_json::to_string(entry)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }
        file.flush().await?;
        drop(file);
        fs::rename(&temp_path, path).await?;
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

        assert!(storage.load_session(&working_dir).await.unwrap().is_none());
        storage
            .save_session(&working_dir, "session-123")
            .await
            .unwrap();

        let entry = storage.load_session(&working_dir).await.unwrap().unwrap();
        assert_eq!(entry.session_id, "session-123");
    }

    #[tokio::test]
    async fn test_input_history() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path()).unwrap();

        let working_dir = PathBuf::from("/path/to/project");

        let history = storage.load_input_history(&working_dir).await.unwrap();
        assert!(history.is_empty());

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
    }
}
