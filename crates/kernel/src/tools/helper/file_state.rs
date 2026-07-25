use super::file_utils::get_mtime;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Simple in-memory file mtime tracking for detecting stale reads
#[derive(Clone)]
pub struct FileStateStore {
    /// Map of file path to last known modification time
    mtimes: Arc<RwLock<HashMap<PathBuf, u64>>>,
    /// Optional persistent storage backend - set once at creation
    persistent: Option<Arc<dyn crate::storage::FileStateStore>>,
}

impl Default for FileStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for FileStateStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FileStateStore")
            .field("mtimes_count", &self.mtimes.read().map_or(0, |m| m.len()))
            .field("has_persistent", &self.persistent.is_some())
            .finish()
    }
}

impl FileStateStore {
    /// Create a new empty store (no persistence)
    pub fn new() -> Self {
        Self {
            mtimes: Arc::new(RwLock::new(HashMap::new())),
            persistent: None,
        }
    }

    /// Create with persistent storage backend (empty memory)
    #[must_use]
    pub fn with_persistent(mut self, persistent: Arc<dyn crate::storage::FileStateStore>) -> Self {
        self.persistent = Some(persistent);
        self
    }

    /// Create with memory states only (no persistence)
    #[must_use]
    pub fn with_states(mut self, states: impl Iterator<Item = (PathBuf, u64)>) -> Self {
        let mtimes: HashMap<PathBuf, u64> = states
            .map(|(p, m)| (p.canonicalize().unwrap_or(p), m))
            .collect();
        self.mtimes = Arc::new(RwLock::new(mtimes));
        self
    }

    /// Record a file's modification time
    ///
    /// Updates in-memory state synchronously, then persists if a persistent store is configured.
    /// Persistence errors are logged but not returned (best-effort persistence).
    pub async fn record(&self, path: PathBuf, mtime: u64) {
        let key = match tokio::fs::canonicalize(&path).await {
            Ok(p) => p,
            Err(_) => path,
        };
        self.mtimes.write().unwrap().insert(key.clone(), mtime);

        if let Some(ref store) = self.persistent {
            if let Err(e) = store.record(key, mtime).await {
                tracing::warn!("Failed to persist file state: {}", e);
            }
        }
    }

    /// Stat the file and record its current mtime.
    ///
    /// No-op if the file's metadata cannot be read.
    pub async fn refresh(&self, path: &Path) {
        if let Some(mtime) = get_mtime(path).await {
            self.record(path.to_path_buf(), mtime).await;
        }
    }

    /// Like [`refresh`](Self::refresh), but only for files already known to the
    /// store. Mutations that don't reveal content (edit, append) must use this
    /// so they can't mark a never-read file as known and unlock write-overwrite.
    pub async fn refresh_if_known(&self, path: &Path) {
        if self.has_recorded(path) {
            self.refresh(path).await;
        }
    }

    /// Get the recorded mtime for a file
    pub fn get_mtime(&self, path: &Path) -> Option<u64> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.mtimes.read().unwrap().get(&key).copied()
    }

    /// Check if a file has been recorded
    pub fn has_recorded(&self, path: &Path) -> bool {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.mtimes.read().unwrap().contains_key(&key)
    }

    /// Remove a file entry
    pub fn remove(&self, path: &Path) -> Option<u64> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.mtimes.write().unwrap().remove(&key)
    }

    /// Clear all entries (called when compactor runs)
    pub async fn clear(&self) {
        self.mtimes.write().unwrap().clear();

        if let Some(ref store) = self.persistent {
            if let Err(e) = store.truncate().await {
                tracing::warn!("Failed to clear persisted file states: {}", e);
            }
        }
    }

    /// Check if file has been modified since last read.
    /// Returns `false` if the file was never recorded.
    pub fn is_stale(&self, path: &Path, current_mtime: u64) -> bool {
        self.get_mtime(path)
            .is_some_and(|recorded| recorded != current_mtime)
    }

    /// Check staleness and return an error message if stale
    pub fn check_staleness(
        &self,
        path: &Path,
        current_mtime: u64,
    ) -> std::result::Result<(), String> {
        if self.is_stale(path, current_mtime) {
            Err(
                "File has been modified since it was read. Read the file again before modifying."
                    .to_string(),
            )
        } else {
            Ok(())
        }
    }

    /// Check if the store is empty
    pub fn is_empty(&self) -> bool {
        self.mtimes.read().map_or(true, |m| m.is_empty())
    }

    /// Record multiple file states efficiently
    ///
    /// Updates in-memory state synchronously, then persists if a persistent store is configured.
    /// Persistence errors are logged but not returned (best-effort persistence).
    pub async fn record_batch(&self, states: Vec<(PathBuf, u64)>) {
        if states.is_empty() {
            return;
        }

        // Canonicalize paths first to ensure consistency between memory and persistence
        let mut canonicalized: Vec<(PathBuf, u64)> = Vec::with_capacity(states.len());
        for (path, mtime) in states {
            let key = match tokio::fs::canonicalize(&path).await {
                Ok(p) => p,
                Err(_) => path,
            };
            canonicalized.push((key, mtime));
        }

        // Update memory first
        {
            let mut mtimes = self.mtimes.write().unwrap();
            for (path, mtime) in &canonicalized {
                mtimes.insert(path.clone(), *mtime);
            }
        }

        // Persist if store is available
        if let Some(ref store) = self.persistent {
            let file_states: Vec<crate::storage::FileState> = canonicalized
                .into_iter()
                .map(|(path, mtime)| crate::storage::FileState::new(path, mtime))
                .collect();

            if let Err(e) = store.record_batch(file_states).await {
                tracing::warn!("Failed to persist file states batch: {}", e);
            }
        }
    }
}

#[cfg(test)]
#[path = "file_state_test.rs"]
mod tests;
