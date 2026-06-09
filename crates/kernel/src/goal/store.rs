use super::state::GoalState;
use crate::types::{KernelError, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use tokio::sync::Mutex;

/// Storage for goal state persistence
///
/// Goals are stored per-session so that a session can be resumed with its
/// active goal restored.
#[async_trait]
pub trait GoalStore: Send + Sync {
    /// Save the current goal state for a session
    async fn save(&self, session_id: &str, state: &GoalState) -> Result<()>;

    /// Load the goal state for a session, if any
    async fn load(&self, session_id: &str) -> Result<Option<GoalState>>;

    /// Delete the stored goal state for a session
    async fn delete(&self, session_id: &str) -> Result<()>;
}

/// JSON file-based goal store
///
/// Stores goals as `{data_dir}/goals/{session_id}.json`.
/// This mirrors the pattern used by `JsonTodoStore`.
#[derive(Debug)]
pub struct JsonGoalStore {
    data_dir: PathBuf,
    lock: Mutex<()>,
}

impl JsonGoalStore {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            lock: Mutex::new(()),
        }
    }

    fn path(&self, session_id: &str) -> PathBuf {
        self.data_dir
            .join("sessions")
            .join("goals")
            .join(format!("{session_id}.json"))
    }

    async fn ensure_dir(&self) -> Result<()> {
        let dir = self.data_dir.join("sessions").join("goals");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| KernelError::storage(format!("failed to create goals dir: {e}")))?;
        Ok(())
    }
}

#[async_trait]
impl GoalStore for JsonGoalStore {
    async fn save(&self, session_id: &str, state: &GoalState) -> Result<()> {
        let _guard = self.lock.lock().await;
        self.ensure_dir().await?;
        let path = self.path(session_id);
        let json =
            serde_json::to_string_pretty(state).map_err(|e| KernelError::storage(e.to_string()))?;
        tokio::fs::write(&path, json)
            .await
            .map_err(|e| KernelError::storage(format!("failed to write goal file: {e}")))?;
        Ok(())
    }

    async fn load(&self, session_id: &str) -> Result<Option<GoalState>> {
        let _guard = self.lock.lock().await;
        let path = self.path(session_id);
        if !path.exists() {
            return Ok(None);
        }
        let json = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| KernelError::storage(format!("failed to read goal file: {e}")))?;
        let state: GoalState =
            serde_json::from_str(&json).map_err(|e| KernelError::storage(e.to_string()))?;
        Ok(Some(state))
    }

    async fn delete(&self, session_id: &str) -> Result<()> {
        let _guard = self.lock.lock().await;
        let path = self.path(session_id);
        if path.exists() {
            tokio::fs::remove_file(&path)
                .await
                .map_err(|e| KernelError::storage(format!("failed to delete goal file: {e}")))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = JsonGoalStore::new(tmp.path());

        let state = GoalState::new("do something");
        store.save("sess-1", &state).await.unwrap();

        let loaded = store.load("sess-1").await.unwrap().unwrap();
        assert_eq!(loaded.description, "do something");
    }

    #[tokio::test]
    async fn test_load_missing() {
        let tmp = TempDir::new().unwrap();
        let store = JsonGoalStore::new(tmp.path());
        assert!(store.load("nonexistent").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete() {
        let tmp = TempDir::new().unwrap();
        let store = JsonGoalStore::new(tmp.path());

        let state = GoalState::new("x");
        store.save("s", &state).await.unwrap();
        assert!(store.load("s").await.unwrap().is_some());

        store.delete("s").await.unwrap();
        assert!(store.load("s").await.unwrap().is_none());
    }
}
