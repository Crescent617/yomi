use crate::config::DEFAULT_DATA_DIR;
use crate::types::{Message, SessionId, SessionRecord};
use anyhow::Result;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[async_trait]
pub trait Storage: Send + Sync {
    async fn create_session(&self, project_path: &Path) -> Result<SessionId>;
    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId>;
    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>>;
    async fn list_sessions(&self, project_path: &Path) -> Result<Vec<SessionRecord>>;
    async fn delete_session(&self, id: &SessionId) -> Result<()>;
    async fn append_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()>;
    async fn get_messages(&self, session_id: &SessionId) -> Result<Vec<Message>>;
    async fn update_summary(&self, session_id: &SessionId, summary: &str) -> Result<()>;
    async fn get_summary(&self, session_id: &SessionId) -> Result<Option<String>>;
}

pub mod fs;
pub mod sqlite;

pub use fs::FsStorage;
pub use sqlite::SqliteStorage;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub url: String,
    pub compaction_threshold: usize,
}

impl StorageConfig {
    /// Create config with expanded data directory path
    pub fn with_data_dir(data_dir: &std::path::Path) -> Self {
        Self {
            url: data_dir.join("sessions.db").to_string_lossy().to_string(),
            compaction_threshold: 100,
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        // Use expanded path for default
        let data_dir = crate::config::expand_tilde(DEFAULT_DATA_DIR);
        Self::with_data_dir(&data_dir)
    }
}
