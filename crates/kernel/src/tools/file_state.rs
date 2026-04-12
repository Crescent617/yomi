use std::collections::HashMap;
use std::path::PathBuf;
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
    pub fn record(&self, path: PathBuf, mtime: u64) {
        self.mtimes.write().unwrap().insert(path, mtime);
    }

    /// Get the recorded mtime for a file
    pub fn get_mtime(&self, path: &PathBuf) -> Option<u64> {
        self.mtimes.read().unwrap().get(path).copied()
    }

    /// Check if a file has been recorded
    pub fn has_recorded(&self, path: &PathBuf) -> bool {
        self.mtimes.read().unwrap().contains_key(path)
    }

    /// Remove a file entry
    pub fn remove(&self, path: &PathBuf) -> Option<u64> {
        self.mtimes.write().unwrap().remove(path)
    }

    /// Clear all entries
    pub fn clear(&self) {
        self.mtimes.write().unwrap().clear();
    }

    /// Check if file has been modified since last read
    /// Returns true if file was not recorded or mtime differs
    pub fn is_stale(&self, path: &PathBuf, current_mtime: u64) -> bool {
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
