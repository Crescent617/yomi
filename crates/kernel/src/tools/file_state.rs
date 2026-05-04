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
    pub fn record(&self, path: PathBuf, mtime: u64) {
        let key = path.canonicalize().unwrap_or(path);
        self.mtimes.write().unwrap().insert(key.clone(), mtime);

        if let Some(ref store) = self.persistent {
            let store = Arc::clone(store);
            tokio::spawn(async move {
                if let Err(e) = store.record(key, mtime).await {
                    tracing::warn!("Failed to persist file state: {}", e);
                }
            });
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
    pub fn clear(&self) {
        self.mtimes.write().unwrap().clear();

        if let Some(ref store) = self.persistent {
            let store = Arc::clone(store);
            tokio::spawn(async move {
                if let Err(e) = store.truncate().await {
                    tracing::warn!("Failed to clear persisted file states: {}", e);
                }
            });
        }
    }

    /// Check if file has been modified since last read
    pub fn is_stale(&self, path: &Path, current_mtime: u64) -> bool {
        self.get_mtime(path) != Some(current_mtime)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_state_store() {
        let store = FileStateStore::new();
        let path = PathBuf::from("/tmp/test.txt");

        assert!(!store.has_recorded(&path));
        assert!(store.get_mtime(&path).is_none());

        store.record(path.clone(), 12345);

        assert!(store.has_recorded(&path));
        assert_eq!(store.get_mtime(&path), Some(12345));
        assert!(!store.is_stale(&path, 12345));
        assert!(store.is_stale(&path, 12346));

        store.remove(&path);
        assert!(!store.has_recorded(&path));
        assert!(store.is_stale(&path, 12345));
    }
}
