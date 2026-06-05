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

    /// Hash a working directory path to a filename using std hasher
    fn hash_path(working_dir: &Path) -> String {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let path_str = working_dir.to_string_lossy();
        let mut hasher = DefaultHasher::new();
        path_str.hash(&mut hasher);
        format!("{:x}", hasher.finish())
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

    /// Merge new entries into existing history, deduplicate, and trim.
    ///
    /// - Appends all `new_entries` to existing history
    /// - Removes duplicates, keeping the latest occurrence of each
    /// - Trims to [`tui::INPUT_HISTORY_LIMIT`] entries with hysteresis (keeps last 50%)
    ///
    /// Call once on session exit with all `new_history_entries`.
    pub async fn save_input_history(
        &self,
        working_dir: &Path,
        new_entries: &[String],
    ) -> Result<()> {
        if new_entries.is_empty() {
            return Ok(());
        }

        let path = self.input_hist_path(working_dir);
        let mut entries = self.load_input_history(working_dir).await?;
        entries.extend(new_entries.iter().cloned());

        // Dedup: process from end to keep latest occurrence, then reverse back
        let mut seen = std::collections::HashSet::new();
        let mut deduped: Vec<String> = entries
            .into_iter()
            .rev()
            .filter(|e| seen.insert(e.clone()))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        // Trim with hysteresis: if over limit, keep the last half
        let limit = tui::INPUT_HISTORY_LIMIT;
        let entries = if deduped.len() > limit {
            deduped.split_off(deduped.len() - limit / 2)
        } else {
            deduped
        };

        self.write_history(&path, &entries).await
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
            .save_input_history(&working_dir, &["hello".to_string(), "world".to_string()])
            .await
            .unwrap();

        let history = storage.load_input_history(&working_dir).await.unwrap();
        assert_eq!(history, vec!["hello", "world"]);
    }

    #[tokio::test]
    async fn test_input_history_dedup() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path()).unwrap();

        let working_dir = PathBuf::from("/path/to/project");

        storage
            .save_input_history(&working_dir, &["a".to_string(), "b".to_string()])
            .await
            .unwrap();

        // Re-add "a" — should keep the latest occurrence
        storage
            .save_input_history(&working_dir, &["a".to_string()])
            .await
            .unwrap();

        let history = storage.load_input_history(&working_dir).await.unwrap();
        assert_eq!(history, vec!["b", "a"]);
    }

    #[tokio::test]
    async fn test_input_history_empty_noop() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path()).unwrap();

        let working_dir = PathBuf::from("/path/to/project");

        // Empty entries should not create a file
        storage.save_input_history(&working_dir, &[]).await.unwrap();

        assert!(!storage.input_hist_path(&working_dir).exists());
    }

    #[tokio::test]
    async fn test_input_history_trim() {
        let temp_dir = TempDir::new().unwrap();
        let storage = AppStorage::new(temp_dir.path()).unwrap();

        let working_dir = PathBuf::from("/path/to/project");
        let limit = tui::INPUT_HISTORY_LIMIT;

        // Seed with limit + 1 entries → triggers trim to limit / 2
        let existing: Vec<String> = (0..=limit).map(|i| format!("old_{i}")).collect();
        storage
            .save_input_history(&working_dir, &existing)
            .await
            .unwrap();

        let history = storage.load_input_history(&working_dir).await.unwrap();
        assert_eq!(history.len(), limit / 2);
        assert_eq!(history.last().unwrap(), "old_2000");
    }
}
