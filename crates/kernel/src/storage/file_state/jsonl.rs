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
    /// Lazy initialization: file is created on first write
    pub fn new(session_id: &str, data_dir: &Path) -> Self {
        let file_states_dir = data_dir.join("sessions").join("file_states");
        let safe_id = session_id.replace(['/', '\\'], "_");
        let file_path = file_states_dir.join(format!("{safe_id}.jsonl"));

        let inner: JsonlStore<FileState, PathBuf> =
            JsonlStore::new(&file_path, |fs: &FileState| fs.path.clone());

        Self { inner }
    }

    /// Force vacuum if the file hasn't been vacuumed recently
    pub async fn maybe_vacuum(&self) -> Result<()> {
        let meta = self.inner.meta().await?;
        if crate::utils::now_secs().saturating_sub(meta.vacuumed_at) > VACUUM_INTERVAL_SECS {
            let _ = self.inner.vacuum().await;
        }
        Ok(())
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
#[path = "jsonl_test.rs"]
mod tests;
