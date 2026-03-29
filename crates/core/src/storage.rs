use crate::config::DEFAULT_DATA_DIR;
use crate::types::{Message, SessionRecord, SessionId};
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
    async fn append_messages(
        &self,
        session_id: &SessionId,
        messages: &[Message],
    ) -> Result<()>;
    async fn get_messages(&self,
        session_id: &SessionId,
    ) -> Result<Vec<Message>>;
    async fn update_summary(&self,
        session_id: &SessionId,
        summary: &str,
    ) -> Result<()>;
    async fn get_summary(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<String>>;
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

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            url: format!("{DEFAULT_DATA_DIR}/sessions.db"),
            compaction_threshold: 100,
        }
    }
}
