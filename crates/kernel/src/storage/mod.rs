pub mod fs;
pub mod meta;

pub use fs::FsStorage;
pub use meta::{MetaStorage, SessionMeta};

use crate::types::{Message, SessionId, SessionRecord};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Session information for listing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub parent_id: Option<String>,
    pub title: Option<String>,
    pub message_count: i64,
}

#[async_trait]
pub trait Storage: Send + Sync {
    async fn create_session(&self) -> Result<SessionId>;
    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId>;
    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>>;
    async fn delete_session(&self, id: &SessionId) -> Result<()>;
    async fn append_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()>;
    async fn get_messages(&self, session_id: &SessionId) -> Result<Vec<Message>>;
    /// Replace all messages for a session (used after compaction)
    async fn set_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()>;
    /// List all sessions with basic info
    async fn list_sessions(&self) -> Result<Vec<SessionInfo>>;
}
