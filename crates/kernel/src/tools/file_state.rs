use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Information about a file that has been read
#[derive(Debug, Clone)]
pub struct FileState {
    /// The content that was read
    pub content: String,
    /// Timestamp when the file was last modified (from fs metadata)
    pub timestamp: u64,
    /// Line offset (1-based) if partial read, None for full read
    pub offset: Option<usize>,
    /// Line limit if partial read, None for full read
    pub limit: Option<usize>,
    /// Whether this was a partial view (read with offset/limit)
    pub is_partial_view: bool,
}

impl FileState {
    /// Create a new file state for a full read
    pub const fn full_read(content: String, timestamp: u64) -> Self {
        Self {
            content,
            timestamp,
            offset: None,
            limit: None,
            is_partial_view: false,
        }
    }

    /// Create a new file state for a partial read
    pub const fn partial_read(content: String, timestamp: u64, offset: usize, limit: Option<usize>) -> Self {
        Self {
            content,
            timestamp,
            offset: Some(offset),
            limit,
            is_partial_view: true,
        }
    }

    /// Check if this read covers the given range
    pub fn covers_range(&self, offset: usize, limit: Option<usize>) -> bool {
        if self.is_partial_view {
            // If we have a partial view, check if the requested range matches exactly
            self.offset == Some(offset) && self.limit == limit
        } else {
            // Full read covers any range
            true
        }
    }
}

/// Thread-safe store for tracking file read state
#[derive(Clone)]
pub struct FileStateStore {
    states: Arc<RwLock<HashMap<PathBuf, FileState>>>,
}

impl Default for FileStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl FileStateStore {
    /// Create a new empty file state store
    pub fn new() -> Self {
        Self {
            states: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get the file state for a given path
    pub fn get(&self, path: &PathBuf) -> Option<FileState> {
        self.states.read().unwrap().get(path).cloned()
    }

    /// Set the file state for a given path
    pub fn set(&self, path: PathBuf, state: FileState) {
        self.states.write().unwrap().insert(path, state);
    }

    /// Remove a file state entry
    pub fn remove(&self, path: &PathBuf) -> Option<FileState> {
        self.states.write().unwrap().remove(path)
    }

    /// Check if a file has been read
    pub fn has_been_read(&self, path: &PathBuf) -> bool {
        self.states.read().unwrap().contains_key(path)
    }

    /// Check if a file was read as a full read (not partial)
    pub fn is_full_read(&self, path: &PathBuf) -> bool {
        self.states
            .read()
            .unwrap()
            .get(path)
            .is_some_and(|s| !s.is_partial_view)
    }

    /// Clear all stored states
    pub fn clear(&self) {
        self.states.write().unwrap().clear();
    }

    /// Get the number of tracked files
    pub fn len(&self) -> usize {
        self.states.read().unwrap().len()
    }

    /// Check if no files are being tracked
    pub fn is_empty(&self) -> bool {
        self.states.read().unwrap().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_file_state_full_read() {
        let state = FileState::full_read("content".to_string(), 12345);
        assert!(!state.is_partial_view);
        assert_eq!(state.offset, None);
        assert_eq!(state.limit, None);
        assert!(state.covers_range(1, None));
        assert!(state.covers_range(100, Some(50)));
    }

    #[test]
    fn test_file_state_partial_read() {
        let state = FileState::partial_read("content".to_string(), 12345, 10, Some(20));
        assert!(state.is_partial_view);
        assert_eq!(state.offset, Some(10));
        assert_eq!(state.limit, Some(20));
        assert!(state.covers_range(10, Some(20)));
        assert!(!state.covers_range(1, None));
        assert!(!state.covers_range(10, Some(30)));
    }

    #[test]
    fn test_file_state_store() {
        let store = FileStateStore::new();
        let path = PathBuf::from("/tmp/test.txt");

        assert!(!store.has_been_read(&path));
        assert!(store.get(&path).is_none());

        let state = FileState::full_read("hello".to_string(), 12345);
        store.set(path.clone(), state);

        assert!(store.has_been_read(&path));
        assert!(store.is_full_read(&path));

        let retrieved = store.get(&path).unwrap();
        assert_eq!(retrieved.content, "hello");
        assert_eq!(retrieved.timestamp, 12345);

        store.remove(&path);
        assert!(!store.has_been_read(&path));
    }
}
