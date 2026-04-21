use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Simple file mtime tracking for detecting stale reads
#[derive(Debug, Clone, Default)]
pub struct FileStateStore {
    /// Map of file path to last known modification time
    mtimes: Arc<RwLock<HashMap<PathBuf, u64>>>,
}

impl FileStateStore {
    /// Create a new empty store
    pub fn new() -> Self {
        Self {
            mtimes: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Record a file's modification time
    /// Path is canonicalized if possible for consistent lookup
    pub fn record(&self, path: PathBuf, mtime: u64) {
        // Use canonicalized path as key for consistent lookup
        let key = path.canonicalize().unwrap_or(path);
        self.mtimes.write().unwrap().insert(key, mtime);
    }

    /// Get the recorded mtime for a file
    /// Path is canonicalized if possible for consistent lookup
    pub fn get_mtime(&self, path: &Path) -> Option<u64> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.mtimes.read().unwrap().get(&key).copied()
    }

    /// Check if a file has been recorded
    /// Path is canonicalized if possible for consistent lookup
    pub fn has_recorded(&self, path: &Path) -> bool {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.mtimes.read().unwrap().contains_key(&key)
    }

    /// Remove a file entry
    /// Path is canonicalized if possible for consistent lookup
    pub fn remove(&self, path: &Path) -> Option<u64> {
        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.mtimes.write().unwrap().remove(&key)
    }

    /// Clear all entries
    pub fn clear(&self) {
        self.mtimes.write().unwrap().clear();
    }

    /// Check if file has been modified since last read
    /// Returns true if file was not recorded or mtime differs
    pub fn is_stale(&self, path: &Path, current_mtime: u64) -> bool {
        self.get_mtime(path) != Some(current_mtime)
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
        assert!(store.is_stale(&path, 12345)); // Not recorded = stale
    }
}
