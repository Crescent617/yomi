//! JSON Lines implementation of `FileStateStore`

use super::{storage_err, FileStateStore, StateEntry, STATE_VERSION};
use crate::types::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// Append-only JSONL file store for file states
#[derive(Debug)]
pub struct JsonlFileStateStore {
    file_path: PathBuf,
    file: Mutex<Option<File>>,
}

impl JsonlFileStateStore {
    /// Create or open a state file for the given session
    /// File states are stored in `sessions/file_states/`
    pub async fn new(session_id: &str, data_dir: &Path) -> Result<Self> {
        let file_states_dir = data_dir.join("sessions").join("file_states");
        fs::create_dir_all(&file_states_dir)
            .await
            .map_err(|e| storage_err(e.to_string()))?;

        let file_path = file_states_dir.join(format!("{session_id}.jsonl"));

        let file = if file_path.exists() {
            fs::OpenOptions::new()
                .append(true)
                .open(&file_path)
                .await
                .map_err(|e| storage_err(e.to_string()))?
        } else {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file_path)
                .await
                .map_err(|e| storage_err(e.to_string()))?;

            let meta = StateEntry::Metadata {
                v: STATE_VERSION,
                created: chrono::Utc::now().to_rfc3339(),
            };
            let line = serde_json::to_string(&meta).map_err(|e| storage_err(e.to_string()))?;
            f.write_all(line.as_bytes())
                .await
                .map_err(|e| storage_err(e.to_string()))?;
            f.write_all(b"\n")
                .await
                .map_err(|e| storage_err(e.to_string()))?;
            f
        };

        Ok(Self {
            file_path,
            file: Mutex::new(Some(file)),
        })
    }
}

#[async_trait]
impl FileStateStore for JsonlFileStateStore {
    async fn record(&self, path: PathBuf, mtime: u64) -> Result<()> {
        let entry = StateEntry::FileState { p: path, m: mtime };
        let line = serde_json::to_string(&entry).map_err(|e| storage_err(e.to_string()))?;

        let mut guard = self.file.lock().await;
        let file = guard.as_mut().ok_or_else(|| storage_err("file not open"))?;

        file.write_all(line.as_bytes())
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        file.write_all(b"\n")
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        file.flush().await.map_err(|e| storage_err(e.to_string()))?;

        Ok(())
    }

    async fn get_all(&self) -> Result<HashMap<PathBuf, u64>> {
        let mut states = HashMap::new();

        if !self.file_path.exists() {
            return Ok(states);
        }

        let file = File::open(&self.file_path)
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| storage_err(e.to_string()))?
        {
            if line.trim().is_empty() {
                continue;
            }

            match serde_json::from_str::<StateEntry>(&line) {
                Ok(StateEntry::FileState { p, m }) => {
                    states.insert(p, m);
                }
                Ok(StateEntry::Metadata { .. }) => {}
                Err(e) => tracing::warn!("failed to parse state entry: {e}"),
            }
        }

        Ok(states)
    }

    async fn clear(&self) -> Result<()> {
        let mut guard = self.file.lock().await;
        *guard = None;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.file_path)
            .await
            .map_err(|e| storage_err(e.to_string()))?;

        let meta = StateEntry::Metadata {
            v: STATE_VERSION,
            created: chrono::Utc::now().to_rfc3339(),
        };
        let line = serde_json::to_string(&meta).map_err(|e| storage_err(e.to_string()))?;
        file.write_all(line.as_bytes())
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        file.write_all(b"\n")
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        file.flush().await.map_err(|e| storage_err(e.to_string()))?;

        *guard = Some(file);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_store() -> (JsonlFileStateStore, TempDir) {
        let temp = TempDir::new().unwrap();
        let store = JsonlFileStateStore::new("test-session", temp.path())
            .await
            .unwrap();
        (store, temp)
    }

    #[tokio::test]
    async fn test_record_and_get() {
        let (store, _temp) = create_test_store().await;

        store.record(PathBuf::from("/tmp/a.rs"), 100).await.unwrap();
        store.record(PathBuf::from("/tmp/b.rs"), 200).await.unwrap();

        let states = store.get_all().await.unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states.get(&PathBuf::from("/tmp/a.rs")), Some(&100));
        assert_eq!(states.get(&PathBuf::from("/tmp/b.rs")), Some(&200));
    }

    #[tokio::test]
    async fn test_duplicate_paths_keep_latest() {
        let (store, _temp) = create_test_store().await;

        store
            .record(PathBuf::from("/tmp/test.rs"), 100)
            .await
            .unwrap();
        store
            .record(PathBuf::from("/tmp/test.rs"), 200)
            .await
            .unwrap();

        let states = store.get_all().await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states.get(&PathBuf::from("/tmp/test.rs")), Some(&200));
    }

    #[tokio::test]
    async fn test_clear() {
        let (store, _temp) = create_test_store().await;

        store
            .record(PathBuf::from("/tmp/test.rs"), 100)
            .await
            .unwrap();
        store.clear().await.unwrap();

        let states = store.get_all().await.unwrap();
        assert!(states.is_empty());
    }

    #[tokio::test]
    async fn test_persist_across_reopen() {
        let temp = TempDir::new().unwrap();
        let session_id = "persist-session";

        {
            let store = JsonlFileStateStore::new(session_id, temp.path())
                .await
                .unwrap();
            store
                .record(PathBuf::from("/tmp/test.rs"), 123)
                .await
                .unwrap();
        }

        {
            let store = JsonlFileStateStore::new(session_id, temp.path())
                .await
                .unwrap();
            let states = store.get_all().await.unwrap();
            assert_eq!(states.len(), 1);
            assert_eq!(states.get(&PathBuf::from("/tmp/test.rs")), Some(&123));
        }
    }
}
