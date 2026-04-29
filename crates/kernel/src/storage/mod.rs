pub mod fs;
pub mod meta;
pub mod migrations;
pub mod todo;

pub use fs::FsStorage;
pub use meta::{MetaStorage, SessionMeta};
pub use migrations::{run_migrations, CURRENT_SCHEMA_VERSION};
pub use todo::TodoStorage;

use crate::types::{Message, SessionId, SessionRecord};
use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Session information for listing
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub parent_id: Option<String>,
    pub title: Option<String>,
    pub message_count: i64,
    pub working_dir: Option<String>,
}

impl SessionInfo {
    /// Format the age of the session (time since last update) as a human-readable string
    pub fn format_age(&self) -> String {
        let age = Utc::now() - self.updated_at;
        if age.num_days() > 0 {
            format!("{}d ago", age.num_days())
        } else if age.num_hours() > 0 {
            format!("{}h ago", age.num_hours())
        } else if age.num_minutes() > 0 {
            format!("{}m ago", age.num_minutes())
        } else {
            "just now".to_string()
        }
    }
}

#[async_trait]
pub trait Storage: Send + Sync {
    /// Create a new session with optional working directory
    async fn create_session(&self, working_dir: Option<&str>) -> Result<SessionId>;
    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId>;
    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>>;
    async fn delete_session(&self, id: &SessionId) -> Result<()>;
    async fn append_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()>;
    async fn get_messages(&self, session_id: &SessionId) -> Result<Vec<Message>>;
    /// Replace all messages for a session (used after compaction)
    async fn set_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()>;
    /// List all sessions with basic info
    async fn list_sessions(&self) -> Result<Vec<SessionInfo>>;
    /// List sessions filtered by working directory
    async fn list_sessions_by_working_dir(&self, working_dir: &str) -> Result<Vec<SessionInfo>>;
}
