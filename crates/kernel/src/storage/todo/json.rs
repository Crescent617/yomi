//! JSON file implementation of `TodoStore`

use super::{storage_err, TodoStore};
use crate::types::Result;
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::fs;

/// File-based todo storage using JSON files
#[derive(Debug, Clone)]
pub struct JsonTodoStore {
    base_dir: PathBuf,
}

impl JsonTodoStore {
    /// Create new store with the given data directory
    /// Todo files are stored in `sessions/todos/`
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: data_dir.into().join("sessions").join("todos"),
        }
    }

    fn file_path(&self, session_id: &str) -> PathBuf {
        // Sanitize session_id to prevent path traversal
        let safe_id = session_id.replace(['/', '\\'], "_");
        self.base_dir.join(format!("{safe_id}.json"))
    }
}

#[async_trait]
impl TodoStore for JsonTodoStore {
    async fn save(&self, session_id: &str, json: &str) -> Result<()> {
        let path = self.file_path(session_id);

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| storage_err(e.to_string()))?;
        }

        fs::write(&path, json)
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        Ok(())
    }

    async fn load(&self, session_id: &str) -> Result<Option<String>> {
        let path = self.file_path(session_id);

        if !path.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&path)
            .await
            .map_err(|e| storage_err(e.to_string()))?;
        Ok(Some(content))
    }

    async fn clear(&self, session_id: &str) -> Result<()> {
        let path = self.file_path(session_id);

        if path.exists() {
            fs::remove_file(&path)
                .await
                .map_err(|e| storage_err(e.to_string()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_store() -> (JsonTodoStore, TempDir) {
        let temp = TempDir::new().unwrap();
        let store = JsonTodoStore::new(temp.path());
        (store, temp)
    }

    #[tokio::test]
    async fn test_save_and_load() {
        let (store, _temp) = create_test_store().await;

        store.save("s1", r#"{"todos":[]}"#).await.unwrap();
        let loaded = store.load("s1").await.unwrap().unwrap();

        assert_eq!(loaded, r#"{"todos":[]}"#);
    }

    #[tokio::test]
    async fn test_load_nonexistent() {
        let (store, _temp) = create_test_store().await;

        assert!(store.load("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_clear() {
        let (store, _temp) = create_test_store().await;

        store.save("s1", "{}").await.unwrap();
        store.clear("s1").await.unwrap();

        assert!(store.load("s1").await.unwrap().is_none());
    }
}
