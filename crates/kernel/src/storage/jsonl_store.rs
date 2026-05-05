//! Generic append-only JSONL store with internal metadata and auto-vacuum

use crate::types::KernelError;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::HashSet;
use std::hash::Hash;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::fs::{self, File};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Mutex;

/// Store metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Metadata {
    /// Unix timestamp (seconds since epoch)
    pub created_at: u64,
    /// Last vacuum timestamp
    #[serde(default)]
    pub vacuumed_at: u64,
    #[serde(default)]
    pub truncate_count: u32,
    #[serde(default)]
    pub vacuum_count: u32,
}

impl Metadata {
    fn new() -> Self {
        let now = crate::utils::now_secs();
        Self {
            created_at: now,
            vacuumed_at: now,
            truncate_count: 0,
            vacuum_count: 0,
        }
    }
}

/// Generic append-only JSONL store with auto-vacuum
///
/// - Internal metadata tracks timestamps, vacuum/truncate counts
/// - Auto-vacuum every N appends to control file size
/// - Vacuum deduplicates by key function bound at creation
pub struct JsonlStore<R, K> {
    path: PathBuf,
    file: Mutex<Option<File>>,
    vacuum_threshold: usize,
    append_count: AtomicUsize,
    key_fn: Arc<dyn Fn(&R) -> K + Send + Sync>,
    _phantom: PhantomData<(R, K)>,
}

impl<R, K> std::fmt::Debug for JsonlStore<R, K> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JsonlStore")
            .field("path", &self.path)
            .field("vacuum_threshold", &self.vacuum_threshold)
            .finish_non_exhaustive()
    }
}

impl<R, K> JsonlStore<R, K>
where
    R: Serialize + DeserializeOwned + Clone,
    K: Eq + Hash + Send + Sync,
{
    /// Open existing file or create new with default metadata
    pub async fn open(
        path: impl AsRef<Path>,
        key_fn: impl Fn(&R) -> K + Send + Sync + 'static,
    ) -> crate::types::Result<Self> {
        Self::open_with_threshold(path, key_fn, 1000).await
    }

    /// Open with custom vacuum threshold
    pub async fn open_with_threshold(
        path: impl AsRef<Path>,
        key_fn: impl Fn(&R) -> K + Send + Sync + 'static,
        vacuum_threshold: usize,
    ) -> crate::types::Result<Self> {
        let path = path.as_ref().to_path_buf();

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| KernelError::Io(e.to_string()))?;
        }

        let exists = path.exists();

        let file = if exists {
            fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .await
                .map_err(|e| KernelError::Io(e.to_string()))?
        } else {
            let mut f = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .await
                .map_err(|e| KernelError::Io(e.to_string()))?;

            // Write metadata as first line
            let meta = Metadata::new();
            Self::write_line(&mut f, &meta).await?;
            f
        };

        Ok(Self {
            path,
            file: Mutex::new(Some(file)),
            vacuum_threshold,
            append_count: AtomicUsize::new(0),
            key_fn: Arc::new(key_fn),
            _phantom: PhantomData,
        })
    }

    /// Append a record. Auto-vacuum if threshold reached.
    pub async fn append(&self, record: &R) -> crate::types::Result<()> {
        self.append_batch(std::slice::from_ref(record)).await
    }

    /// Append multiple records with a single flush.
    /// More efficient than calling `append()` in a loop.
    pub async fn append_batch(&self, records: &[R]) -> crate::types::Result<()> {
        if records.is_empty() {
            return Ok(());
        }

        // Write all records with single flush
        {
            let mut guard = self.file.lock().await;
            let file = guard
                .as_mut()
                .ok_or_else(|| KernelError::Storage("store closed".to_string()))?;
            for record in records {
                Self::write_line(file, record).await?;
            }
            file.flush()
                .await
                .map_err(|e| KernelError::Io(e.to_string()))?;
        }

        // Update append count and check vacuum using CAS
        let added = records.len();
        let count = self.append_count.fetch_add(added, Ordering::Relaxed) + added;
        if count >= self.vacuum_threshold {
            // Try to reset counter to 0 only if it's still at count (CAS).
            // Only the thread that succeeds CAS will vacuum.
            let cas_result =
                self.append_count
                    .compare_exchange(count, 0, Ordering::Relaxed, Ordering::Relaxed);

            if cas_result.is_ok() {
                // This thread won the race, do the vacuum
                if let Err(e) = self.vacuum().await {
                    tracing::warn!("Auto-vacuum failed: {}, will retry on next append", e);
                    // Restore counter to trigger retry on next append
                    self.append_count
                        .store(self.vacuum_threshold, Ordering::Relaxed);
                }
            }
            // If CAS failed, another thread is vacuuming, just continue
        }

        Ok(())
    }

    /// Read all records deduplicated by key (keeps last occurrence)
    pub async fn read_all(&self) -> crate::types::Result<Vec<R>> {
        let records = self.read_raw().await?;
        Ok(self.dedup(records))
    }

    /// Clear all records, keeping metadata.
    pub async fn truncate(&self) -> crate::types::Result<()> {
        let meta = Self::read_meta(&self.path).await.unwrap_or_default();

        self.rewrite_file(meta, &[], |meta| {
            meta.truncate_count += 1;
        })
        .await?;

        // Reset append counter
        self.append_count.store(0, Ordering::Relaxed);

        tracing::debug!("Truncated JSONL store");
        Ok(())
    }

    /// Force vacuum now (deduplicate by key)
    pub async fn vacuum(&self) -> crate::types::Result<()> {
        let records = self.read_raw().await?;
        let deduped = self.dedup(records);

        let meta = Self::read_meta(&self.path).await.unwrap_or_default();
        self.rewrite_file(meta, &deduped, |meta| {
            meta.vacuum_count += 1;
            meta.vacuumed_at = crate::utils::now_secs();
        })
        .await?;

        tracing::debug!("Vacuumed JSONL store: {} records", deduped.len());
        Ok(())
    }

    /// Get metadata
    pub async fn meta(&self) -> crate::types::Result<Metadata> {
        Ok(Self::read_meta(&self.path).await.unwrap_or_default())
    }

    /// Close the store
    pub async fn close(&self) {
        let mut guard = self.file.lock().await;
        *guard = None;
    }

    // Private helpers

    /// Read raw records without deduplication
    async fn read_raw(&self) -> crate::types::Result<Vec<R>> {
        let mut records = Vec::new();

        if !self.path.exists() {
            return Ok(records);
        }

        let file = File::open(&self.path)
            .await
            .map_err(|e| KernelError::Io(e.to_string()))?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        // Skip metadata line
        let _ = lines.next_line().await;

        while let Some(line) = lines
            .next_line()
            .await
            .map_err(|e| KernelError::Io(e.to_string()))?
        {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<R>(&line) {
                Ok(record) => records.push(record),
                Err(e) => tracing::warn!("Failed to parse record: {}", e),
            }
        }

        Ok(records)
    }

    /// Deduplicate records by key, keeping last occurrence but preserving original order.
    fn dedup(&self, records: Vec<R>) -> Vec<R> {
        let mut seen = HashSet::new();
        let mut result = Vec::with_capacity(records.len());
        for r in records.into_iter().rev() {
            if seen.insert((self.key_fn)(&r)) {
                result.push(r);
            }
        }
        result.reverse();
        result
    }

    /// Rewrite file with metadata and records. Closes and reopens file handle.
    async fn rewrite_file(
        &self,
        mut meta: Metadata,
        records: &[R],
        update_meta_fn: impl FnOnce(&mut Metadata),
    ) -> crate::types::Result<()> {
        update_meta_fn(&mut meta);

        let mut guard = self.file.lock().await;
        *guard = None;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.path)
            .await
            .map_err(|e| KernelError::Io(e.to_string()))?;

        Self::write_line(&mut file, &meta).await?;
        for r in records {
            Self::write_line(&mut file, r).await?;
        }
        file.flush()
            .await
            .map_err(|e| KernelError::Io(e.to_string()))?;

        *guard = Some(file);
        Ok(())
    }

    async fn write_line<T: Serialize>(file: &mut File, item: &T) -> crate::types::Result<()> {
        let line = serde_json::to_string(item).map_err(|e| KernelError::Serde(e.to_string()))?;
        file.write_all(line.as_bytes())
            .await
            .map_err(|e| KernelError::Io(e.to_string()))?;
        file.write_all(b"\n")
            .await
            .map_err(|e| KernelError::Io(e.to_string()))?;
        Ok(())
    }

    async fn read_meta(path: &Path) -> Option<Metadata> {
        if !path.exists() {
            return None;
        }

        let file = File::open(path).await.ok()?;
        let reader = BufReader::new(file);
        let mut lines = reader.lines();

        let first = lines.next_line().await.ok()??;
        serde_json::from_str(&first).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestRecord {
        id: u32,
        data: String,
    }

    async fn create_test_store() -> (JsonlStore<TestRecord, u32>, TempDir) {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");
        let store: JsonlStore<TestRecord, u32> = JsonlStore::open(&path, |r: &TestRecord| r.id)
            .await
            .unwrap();
        (store, temp)
    }

    #[tokio::test]
    async fn test_create_and_meta() {
        let (store, _temp) = create_test_store().await;

        let meta = store.meta().await.unwrap();
        assert_eq!(meta.vacuum_count, 0);
    }

    #[tokio::test]
    async fn test_append_and_read() {
        let (store, _temp) = create_test_store().await;

        store
            .append(&TestRecord {
                id: 1,
                data: "hello".to_string(),
            })
            .await
            .unwrap();
        store
            .append(&TestRecord {
                id: 2,
                data: "world".to_string(),
            })
            .await
            .unwrap();

        let records = store.read_all().await.unwrap();
        assert_eq!(records.len(), 2);
    }

    #[tokio::test]
    async fn test_read_all_deduped() {
        let (store, _temp) = create_test_store().await;

        store
            .append(&TestRecord {
                id: 1,
                data: "first".to_string(),
            })
            .await
            .unwrap();
        store
            .append(&TestRecord {
                id: 1,
                data: "second".to_string(),
            })
            .await
            .unwrap();
        store
            .append(&TestRecord {
                id: 2,
                data: "other".to_string(),
            })
            .await
            .unwrap();

        // read_all() returns deduplicated records by default
        let records = store.read_all().await.unwrap();
        assert_eq!(records.len(), 2);
        // Last occurrence wins
        let r1 = records.iter().find(|r| r.id == 1).unwrap();
        assert_eq!(r1.data, "second");
    }

    #[tokio::test]
    async fn test_auto_vacuum_dedup() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("test.jsonl");
        // Set low threshold for testing
        let store =
            JsonlStore::<TestRecord, u32>::open_with_threshold(&path, |r: &TestRecord| r.id, 5)
                .await
                .unwrap();

        // Append 5 records with duplicate ids
        for i in 0..5 {
            store
                .append(&TestRecord {
                    id: i % 3, // 0, 1, 2, 0, 1 - duplicates
                    data: format!("v{i}"),
                })
                .await
                .unwrap();
        }

        // At 5 records, vacuum should trigger, leaving 3 unique
        let records = store.read_all().await.unwrap();
        assert_eq!(records.len(), 3);

        // Check vacuum was recorded
        let meta = store.meta().await.unwrap();
        assert_eq!(meta.vacuum_count, 1);
    }

    #[tokio::test]
    async fn test_manual_vacuum() {
        let (store, _temp) = create_test_store().await;

        for i in 0..3 {
            store
                .append(&TestRecord {
                    id: 0, // same key
                    data: format!("v{i}"),
                })
                .await
                .unwrap();
        }

        // Force vacuum
        store.vacuum().await.unwrap();

        let records = store.read_all().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].data, "v2"); // last one wins
    }

    #[tokio::test]
    async fn test_clear() {
        let (store, _temp) = create_test_store().await;

        store
            .append(&TestRecord {
                id: 1,
                data: "x".to_string(),
            })
            .await
            .unwrap();

        store.truncate().await.unwrap();

        let records = store.read_all().await.unwrap();
        assert!(records.is_empty());

        let meta = store.meta().await.unwrap();
        assert_eq!(meta.truncate_count, 1);
    }

    #[tokio::test]
    async fn test_persist_across_reopen() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("persist.jsonl");

        {
            let store: JsonlStore<TestRecord, u32> = JsonlStore::open(&path, |r: &TestRecord| r.id)
                .await
                .unwrap();
            store
                .append(&TestRecord {
                    id: 42,
                    data: "test".to_string(),
                })
                .await
                .unwrap();
        }

        {
            let store: JsonlStore<TestRecord, u32> = JsonlStore::open(&path, |r: &TestRecord| r.id)
                .await
                .unwrap();
            let records = store.read_all().await.unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(records[0].id, 42);
        }
    }

    #[tokio::test]
    async fn test_append_batch() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("batch.jsonl");
        let store: JsonlStore<TestRecord, u32> = JsonlStore::open(&path, |r: &TestRecord| r.id)
            .await
            .unwrap();

        // Append batch of 5 records with single flush
        let records: Vec<TestRecord> = (0..5)
            .map(|i| TestRecord {
                id: i,
                data: format!("batch_{i}"),
            })
            .collect();

        store.append_batch(&records).await.unwrap();

        // Verify all records persisted
        let read = store.read_all().await.unwrap();
        assert_eq!(read.len(), 5);

        // Verify data integrity (sort by id since read_all returns unordered)
        let mut sorted: Vec<_> = read.into_iter().collect();
        sorted.sort_by_key(|r| r.id);
        for (i, r) in sorted.iter().enumerate() {
            assert_eq!(r.id as usize, i);
            assert_eq!(r.data, format!("batch_{i}"));
        }
    }

    #[tokio::test]
    async fn test_append_batch_empty() {
        let (store, _temp) = create_test_store().await;

        // Empty batch should be no-op
        store.append_batch(&[]).await.unwrap();

        let records = store.read_all().await.unwrap();
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn test_append_batch_triggers_vacuum() {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join("batch_vacuum.jsonl");
        // Set threshold to 5
        let store =
            JsonlStore::<TestRecord, u32>::open_with_threshold(&path, |r: &TestRecord| r.id, 5)
                .await
                .unwrap();

        // Append batch of 5 (exactly at threshold)
        let records: Vec<TestRecord> = (0..5)
            .map(|i| TestRecord {
                id: i % 3, // duplicates: 0, 1, 2, 0, 1
                data: format!("v{i}"),
            })
            .collect();

        store.append_batch(&records).await.unwrap();

        // Vacuum should have triggered, leaving 3 unique
        let read = store.read_all().await.unwrap();
        assert_eq!(read.len(), 3);

        // Check vacuum was recorded
        let meta = store.meta().await.unwrap();
        assert_eq!(meta.vacuum_count, 1);
    }
}
