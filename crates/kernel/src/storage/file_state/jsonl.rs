//! JSON Lines implementation of `FileStateStore`

use super::{storage_err, FileStateStore, StateEntry};
use crate::types::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// Vacuum interval in seconds: compact files older than 1 hour
const VACUUM_INTERVAL_SECS: u64 = 3600;

/// Record operations threshold: vacuum every N records
const RECORD_THRESHOLD: usize = 100;

/// Append-only JSONL file store for file states
/// Auto-compacts every `COMPACT_THRESHOLD` entries to deduplicate paths
#[derive(Debug)]
pub struct JsonlFileStateStore {
    file_path: PathBuf,
    file: Mutex<Option<File>>,
    /// Counter for record operations since last vacuum
    counter: AtomicUsize,
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

        let exists = file_path.exists();

        let file = if exists {
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

            let meta = StateEntry::default_meta();
            let line = serde_json::to_string(&meta).map_err(|e| storage_err(e.to_string()))?;
            f.write_all(line.as_bytes())
                .await
                .map_err(|e| storage_err(e.to_string()))?;
            f.write_all(b"\n")
                .await
                .map_err(|e| storage_err(e.to_string()))?;
            f
        };

        // Check if vacuum needed based on file age
        let meta = if exists {
            Self::read_meta(&file_path).await
        } else {
            None
        };

        let needs_vacuum = meta.as_ref().is_some_and(|m| {
            if let StateEntry::Metadata { created_at, .. } = m {
                crate::utils::now_secs().saturating_sub(*created_at) > VACUUM_INTERVAL_SECS
            } else {
                false
            }
        });

        let store = Self {
            file_path,
            file: Mutex::new(Some(file)),
            counter: AtomicUsize::new(0),
        };

        // Vacuum old files on open
        if exists && needs_vacuum {
            let _ = store.vacuum().await;
        }

        Ok(store)
    }

    /// Read metadata from the first line of the file
    async fn read_meta(file_path: &Path) -> Option<StateEntry> {
        if !file_path.exists() {
            return None;
        }

        let file = File::open(file_path).await.ok()?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        let first_line = lines.next_line().await.ok()??;
        serde_json::from_str(&first_line).ok()
    }

    /// Vacuum the file by reading all states, deduplicating, and rewriting
    async fn vacuum(&self) -> Result<()> {
        // Read all states (already deduplicated by HashMap behavior)
        let states = self.get_all().await?;

        let mut guard = self.file.lock().await;
        *guard = None;

        // Build metadata: reuse existing or create default, then update vacuum count and time
        let mut meta = Self::read_meta(&self.file_path)
            .await
            .unwrap_or_else(StateEntry::default_meta);
        if let StateEntry::Metadata {
            vacuum_count: ref mut count,
            ref mut created_at,
            ..
        } = &mut meta
        {
            *count += 1;
            *created_at = crate::utils::now_secs();
        }

        // Rewrite file with compacted data
        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.file_path)
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        let line = serde_json::to_string(&meta).map_err(|e| storage_err(e.to_string()))?;
        file.write_all(line.as_bytes())
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        file.write_all(b"\n")
            .await
            .map_err(|e| storage_err(e.to_string()))?;

        // Write compacted states
        for (path, mtime) in &states {
            let entry = StateEntry::FileState {
                p: path.clone(),
                m: *mtime,
            };
            let line = serde_json::to_string(&entry).map_err(|e| storage_err(e.to_string()))?;
            file.write_all(line.as_bytes())
                .await
                .map_err(|e| storage_err(e.to_string()))?;
            file.write_all(b"\n")
                .await
                .map_err(|e| storage_err(e.to_string()))?;
        }

        file.flush().await.map_err(|e| storage_err(e.to_string()))?;
        *guard = Some(file);

        tracing::debug!("Vacuumed file state store: {} unique paths", states.len());

        Ok(())
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

        // Drop guard before potential vacuum
        drop(guard);

        // Check if vacuum needed based on record count
        let count = self.counter.fetch_add(1, Ordering::Relaxed) + 1;
        if count >= RECORD_THRESHOLD {
            self.vacuum().await?;
            self.counter.store(0, Ordering::Relaxed);
        }

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

    async fn truncate(&self) -> Result<()> {
        let mut guard = self.file.lock().await;
        *guard = None;

        // Build metadata: reuse existing or create default, then update cleared count
        let mut meta = Self::read_meta(&self.file_path)
            .await
            .unwrap_or_else(StateEntry::default_meta);
        if let StateEntry::Metadata {
            ref mut truncate_count,
            ..
        } = &mut meta
        {
            *truncate_count += 1;
        }

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.file_path)
            .await
            .map_err(|e| storage_err(e.to_string()))?;
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
        store.truncate().await.unwrap();

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

    #[tokio::test]
    async fn test_auto_compact_dedup() {
        let temp = TempDir::new().unwrap();
        let store = JsonlFileStateStore::new("compact-test", temp.path())
            .await
            .unwrap();

        // Record 50 entries for the same file (should trigger compaction at 100)
        for i in 0..50 {
            store
                .record(PathBuf::from("/tmp/same.rs"), 100 + i as u64)
                .await
                .unwrap();
        }

        // Record 50 more for different files
        for i in 0..50 {
            store
                .record(PathBuf::from(format!("/tmp/file{i}.rs")), 1000)
                .await
                .unwrap();
        }

        // At 100 entries, compaction should have triggered
        // Result: 1 unique path from first 50 + 50 unique paths = 51 unique
        let states = store.get_all().await.unwrap();
        assert_eq!(states.len(), 51);
        assert_eq!(states.get(&PathBuf::from("/tmp/same.rs")), Some(&149)); // latest mtime
    }
}
