use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

/// Simple file mtime tracking for detecting stale reads
#[derive(Debug, Clone, Default)]
pub struct FileStateStore {
    /// Map of file path to last known modification time
    mtimes: Arc<RwLock<HashMap<PathBuf, u64>>>,
}

/// Serializable representation of file state for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStateEntry {
    pub path: PathBuf,
    pub mtime: u64,
}

/// Serializable collection of file states
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileStateSnapshot {
    pub entries: Vec<FileStateEntry>,
}

impl FileStateSnapshot {
    /// Check if the snapshot has no entries
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
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

    /// Check staleness and return an error message if stale
    pub fn check_staleness(&self, path: &Path, current_mtime: u64) -> Result<(), String> {
        if self.is_stale(path, current_mtime) {
            Err(
                "File has been modified since it was read. Read the file again before modifying."
                    .to_string(),
            )
        } else {
            Ok(())
        }
    }

    /// Create a serializable snapshot of the current file states
    pub fn snapshot(&self) -> FileStateSnapshot {
        let entries = match self.mtimes.read() {
            Ok(mtimes) => mtimes
                .iter()
                .map(|(path, mtime)| FileStateEntry {
                    path: path.clone(),
                    mtime: *mtime,
                })
                .collect(),
            Err(e) => {
                tracing::warn!("Failed to read file state lock: {e}");
                Vec::new()
            }
        };
        FileStateSnapshot { entries }
    }

    /// Create a `FileStateStore` from a snapshot
    pub fn from_snapshot(snapshot: FileStateSnapshot) -> Self {
        let mut mtimes = HashMap::new();
        for entry in snapshot.entries {
            mtimes.insert(entry.path, entry.mtime);
        }
        Self {
            mtimes: Arc::new(RwLock::new(mtimes)),
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
        assert!(store.is_stale(&path, 12345)); // Not recorded = stale
    }
}
