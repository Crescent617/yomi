//! Todo list file storage
//!
//! Provides file-based persistence for todo lists.
//! Each session has its own todo file: `{data_dir}/todos/{session_id}.json`

use std::path::PathBuf;

/// Todo storage manager
#[derive(Debug, Clone)]
pub struct TodoStorage {
    base_dir: PathBuf,
}

impl TodoStorage {
    /// Create new todo storage with the given data directory
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        let base_dir = data_dir.into().join("todos");
        Self { base_dir }
    }

    /// Get the path for a session's todo file
    fn todo_file_path(&self, session_id: &str) -> PathBuf {
        // Sanitize session_id to prevent path traversal
        let safe_id = session_id.replace(['/', '\\'], "_");
        self.base_dir.join(format!("{safe_id}.json"))
    }

    /// Save todo JSON for a session
    pub fn save(&self, session_id: &str, todo_json: &str) -> std::io::Result<()> {
        let path = self.todo_file_path(session_id);

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, todo_json)
    }

    /// Load todo JSON for a session
    /// Returns None if file doesn't exist or is empty
    pub fn load(&self, session_id: &str) -> Option<String> {
        let path = self.todo_file_path(session_id);

        if !path.exists() {
            return None;
        }

        std::fs::read_to_string(&path).ok()
    }

    /// Clear todo for a session (delete the file)
    pub fn clear(&self, session_id: &str) -> std::io::Result<()> {
        let path = self.todo_file_path(session_id);

        if path.exists() {
            std::fs::remove_file(&path)?;
        }

        Ok(())
    }

    /// Check if todo exists for a session
    pub fn exists(&self, session_id: &str) -> bool {
        self.todo_file_path(session_id).exists()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_save_and_load() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TodoStorage::new(temp_dir.path());
        let session_id = "test-session";
        let todo_json = r#"{"todos":[{"id":"1","content":"Test","status":"pending"}]}"#;

        // Save
        storage.save(session_id, todo_json).unwrap();

        // Load
        let loaded = storage.load(session_id).unwrap();
        assert_eq!(loaded, todo_json);
    }

    #[test]
    fn test_load_nonexistent() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TodoStorage::new(temp_dir.path());

        assert!(storage.load("nonexistent").is_none());
    }

    #[test]
    fn test_clear() {
        let temp_dir = TempDir::new().unwrap();
        let storage = TodoStorage::new(temp_dir.path());
        let session_id = "test-session";

        storage.save(session_id, "{}").unwrap();
        assert!(storage.exists(session_id));

        storage.clear(session_id).unwrap();
        assert!(!storage.exists(session_id));
        assert!(storage.load(session_id).is_none());
    }
}
