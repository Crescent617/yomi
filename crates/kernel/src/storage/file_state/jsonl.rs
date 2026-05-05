//! JSON Lines implementation of `FileStateStore` using generic `JsonlStore`

use super::{FileState, FileStateStore};
use crate::storage::jsonl_store::JsonlStore;
use crate::types::Result;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// Vacuum interval in seconds: compact files older than 1 hour
const VACUUM_INTERVAL_SECS: u64 = 3600;

/// Append-only JSONL file store for file states
/// Auto-vacuum internally managed by `JsonlStore`
#[derive(Debug)]
pub struct JsonlFileStateStore {
    inner: JsonlStore<FileState, PathBuf>,
}

impl JsonlFileStateStore {
    /// Create or open a state file for the given session
    /// File states are stored in `sessions/file_states/`
    pub async fn new(session_id: &str, data_dir: &Path) -> Result<Self> {
        let file_states_dir = data_dir.join("sessions").join("file_states");
        let file_path = file_states_dir.join(format!("{session_id}.jsonl"));

        let exists = file_path.exists();
        let inner: JsonlStore<FileState, PathBuf> =
            JsonlStore::open(&file_path, |fs: &FileState| fs.path.clone()).await?;

        // Check if vacuum needed based on last vacuum time
        if exists {
            let meta = inner.meta().await?;
            if crate::utils::now_secs().saturating_sub(meta.vacuumed_at) > VACUUM_INTERVAL_SECS {
                // Force vacuum for old files
                let _ = inner.vacuum().await;
            }
        }

        Ok(Self { inner })
    }
}

#[async_trait]
impl FileStateStore for JsonlFileStateStore {
    async fn record(&self, path: PathBuf, mtime: u64) -> Result<()> {
        let entry = FileState::new(path, mtime);
        self.inner.append(&entry).await?;
        Ok(())
    }

    async fn record_batch(&self, states: Vec<FileState>) -> Result<()> {
        // Append all states with single flush - vacuum will be triggered naturally if threshold reached
        self.inner.append_batch(&states).await
    }

    async fn read_all(&self) -> Result<Vec<FileState>> {
        // read_all() returns deduplicated entries by default (last wins)
        self.inner.read_all().await
    }

    async fn truncate(&self) -> Result<()> {
        self.inner.truncate().await?;
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

        let states = store.read_all().await.unwrap();
        assert_eq!(states.len(), 2);
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

        let states = store.read_all().await.unwrap();
        assert_eq!(states.len(), 1);
        assert_eq!(states[0].path, PathBuf::from("/tmp/test.rs"));
        assert_eq!(states[0].mtime, 200);
    }

    #[tokio::test]
    async fn test_clear() {
        let (store, _temp) = create_test_store().await;

        store
            .record(PathBuf::from("/tmp/test.rs"), 100)
            .await
            .unwrap();
        store.truncate().await.unwrap();

        let states = store.read_all().await.unwrap();
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
            let states = store.read_all().await.unwrap();
            assert_eq!(states.len(), 1);
            assert_eq!(states[0].path, PathBuf::from("/tmp/test.rs"));
            assert_eq!(states[0].mtime, 123);
        }
    }

    #[tokio::test]
    async fn test_auto_vacuum() {
        let temp = TempDir::new().unwrap();

        // Create store with low threshold
        let store = JsonlFileStateStore::new("vacuum-test", temp.path())
            .await
            .unwrap();

        // Write 1000+ records to trigger auto-vacuum
        for i in 0..1005 {
            store
                .record(PathBuf::from("/tmp/same.rs"), 100 + i as u64)
                .await
                .unwrap();
        }

        // After vacuum, only 1 unique path remains
        let states = store.read_all().await.unwrap();
        assert_eq!(states.len(), 1);
        // Latest mtime is 100 + 1004 = 1104
        assert_eq!(states[0].path, PathBuf::from("/tmp/same.rs"));
        assert_eq!(states[0].mtime, 1104);
    }
}
