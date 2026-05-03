use super::entry::{StateEntry, STATE_VERSION};
use crate::types::SessionId;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Manages session state in an append-only JSONL file.
///
/// File format:
/// ```jsonl
/// {"t":"meta","v":1,"created":"2026-05-03T10:00:00Z"}
/// {"t":"file","p":"/home/user/src/main.rs","m":1714723200}
/// ```
pub struct SessionStateManager {
    file_path: PathBuf,
    file: Option<File>,
}

impl std::fmt::Debug for SessionStateManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionStateManager")
            .field("file_path", &self.file_path)
            .field("has_open_file", &self.file.is_some())
            .finish()
    }
}

impl SessionStateManager {
    /// Create or open a session state file.
    ///
    /// If the file doesn't exist, creates it with a metadata header.
    pub async fn new(session_id: &SessionId, data_dir: &Path) -> Result<Self> {
        let sessions_dir = data_dir.join("sessions");
        fs::create_dir_all(&sessions_dir)
            .await
            .context("Failed to create sessions directory")?;

        let file_path = sessions_dir.join(format!("{}.state.jsonl", session_id.0));

        if file_path.exists() {
            // Open existing file in append mode
            let file = fs::OpenOptions::new()
                .append(true)
                .open(&file_path)
                .await
                .context("Failed to open existing state file")?;

            Ok(Self {
                file_path,
                file: Some(file),
            })
        } else {
            // Create new file with metadata header (append mode for consistency)
            let mut file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)
                .await
                .context("Failed to create state file")?;

            let meta = StateEntry::Metadata {
                v: STATE_VERSION,
                created: chrono::Utc::now().to_rfc3339(),
            };
            let line = serde_json::to_string(&meta)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;

            Ok(Self {
                file_path,
                file: Some(file),
            })
        }
    }

    /// Record a file state entry.
    ///
    /// Appends a new line to the file. If you need deduplication,
    /// use `get_file_states()` to get the merged view.
    pub async fn record_file(&mut self, path: PathBuf, mtime: u64) -> Result<()> {
        let entry = StateEntry::FileState { p: path, m: mtime };
        self.append_entry(&entry).await
    }

    /// Append a raw entry to the file.
    async fn append_entry(&mut self, entry: &StateEntry) -> Result<()> {
        let file = self.file.as_mut().context("State file not open")?;

        let line = serde_json::to_string(entry)?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;

        Ok(())
    }

    /// Read all file states from disk.
    ///
    /// Returns a map from path to mtime. If the same path appears multiple times,
    /// the latest (last) entry wins.
    pub async fn get_file_states(&self) -> Result<HashMap<PathBuf, u64>> {
        let mut states = HashMap::new();

        if !self.file_path.exists() {
            return Ok(states);
        }

        let file = fs::File::open(&self.file_path).await?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<StateEntry>(&line) {
                Ok(StateEntry::FileState { p, m }) => {
                    states.insert(p, m);
                }
                Ok(StateEntry::Metadata { .. }) => {
                    // Skip metadata header
                }
                Err(e) => {
                    tracing::warn!("Failed to parse state entry: {}", e);
                }
            }
        }

        Ok(states)
    }

    /// Clear all file states.
    ///
    /// This is called when the compactor runs. It truncates the file
    /// and rewrites only the metadata header.
    pub async fn clear_file_states(&mut self) -> Result<()> {
        // Drop the old file handle
        self.file = None;

        // Truncate and rewrite with just metadata
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.file_path)
            .await
            .context("Failed to truncate state file")?;

        let meta = StateEntry::Metadata {
            v: STATE_VERSION,
            created: chrono::Utc::now().to_rfc3339(),
        };
        let line = serde_json::to_string(&meta)?;
        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;

        self.file = Some(file);
        Ok(())
    }

    /// Get the file path.
    pub fn file_path(&self) -> &Path {
        &self.file_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_manager() -> (SessionStateManager, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let session_id = SessionId::new();
        let manager = SessionStateManager::new(&session_id, temp_dir.path())
            .await
            .unwrap();
        (manager, temp_dir)
    }

    #[tokio::test]
    async fn test_create_new_file() {
        let (manager, _temp) = create_test_manager().await;
        assert!(manager.file_path.exists());
    }

    #[tokio::test]
    async fn test_record_and_get_file_states() {
        let (mut manager, _temp) = create_test_manager().await;

        manager
            .record_file(PathBuf::from("/tmp/a.rs"), 100)
            .await
            .unwrap();
        manager
            .record_file(PathBuf::from("/tmp/b.rs"), 200)
            .await
            .unwrap();

        let states = manager.get_file_states().await.unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states.get(&PathBuf::from("/tmp/a.rs")), Some(&100));
        assert_eq!(states.get(&PathBuf::from("/tmp/b.rs")), Some(&200));
    }

    #[tokio::test]
    async fn test_duplicate_paths_keep_latest() {
        let (mut manager, _temp) = create_test_manager().await;

        manager
            .record_file(PathBuf::from("/tmp/test.rs"), 100)
            .await
            .unwrap();
        manager
            .record_file(PathBuf::from("/tmp/test.rs"), 200)
            .await
            .unwrap();

        let states = manager.get_file_states().await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states.get(&PathBuf::from("/tmp/test.rs")), Some(&200));
    }

    #[tokio::test]
    async fn test_clear_file_states() {
        let (mut manager, _temp) = create_test_manager().await;

        manager
            .record_file(PathBuf::from("/tmp/test.rs"), 100)
            .await
            .unwrap();

        manager.clear_file_states().await.unwrap();

        let states = manager.get_file_states().await.unwrap();
        assert!(states.is_empty());
    }

    #[tokio::test]
    async fn test_persist_across_reopen() {
        let temp_dir = TempDir::new().unwrap();
        let session_id = SessionId::new();

        // First manager instance
        {
            let mut manager = SessionStateManager::new(&session_id, temp_dir.path())
                .await
                .unwrap();
            manager
                .record_file(PathBuf::from("/tmp/persist.rs"), 123)
                .await
                .unwrap();
        }

        // Second manager instance (same session_id)
        {
            let manager = SessionStateManager::new(&session_id, temp_dir.path())
                .await
                .unwrap();
            let states = manager.get_file_states().await.unwrap();
            assert_eq!(states.len(), 1);
            assert_eq!(states.get(&PathBuf::from("/tmp/persist.rs")), Some(&123));
        }
    }
}
