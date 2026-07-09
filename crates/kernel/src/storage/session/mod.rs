//! Session management - session lifecycle and metadata storage

use crate::types::{Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Session metadata for listing and display
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionInfo {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub parent_id: Option<SessionId>,
    pub title: Option<String>,
    pub message_count: i64,
    pub working_dir: Option<String>,
    pub project_id: Option<crate::types::ProjectId>,
    pub auto_approve_level: Option<String>,
    pub model_key: Option<String>,
}

impl SessionInfo {
    /// Format the age of the session as a human-readable string
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

/// Storage for session lifecycle and metadata
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Create a new session with the given ID, optional `project_id`, optional working directory,
    /// optional `auto_approve_level`, optional `parent_id`, and optional `model_key`
    async fn create(
        &self,
        id: &SessionId,
        project_id: Option<&crate::types::ProjectId>,
        working_dir: Option<&str>,
        auto_approve_level: Option<&str>,
        parent_id: Option<&SessionId>,
        model_key: Option<&str>,
    ) -> Result<()>;

    /// Fork a session, copying its metadata (including `auto_approve_level` and `model_key`)
    async fn fork(&self, parent_id: &SessionId) -> Result<SessionId>;

    /// Update session `model_key`
    async fn update_model_key(&self, id: &SessionId, key: &str) -> Result<u64>;

    /// Get session metadata by ID
    async fn get(&self, id: &SessionId) -> Result<Option<SessionInfo>>;

    /// Delete a session
    async fn delete(&self, id: &SessionId) -> Result<()>;

    /// List sessions with cursor-based pagination.
    /// `project_id` = None returns all sessions (including independent ones).
    /// Returns `(sessions, next_cursor)` where `next_cursor` is the `updated_at` of the last
    /// session if there are more pages, or None if this is the last page.
    async fn list(
        &self,
        project_id: Option<&crate::types::ProjectId>,
        before: Option<chrono::DateTime<chrono::Utc>>,
        limit: usize,
    ) -> Result<(Vec<SessionInfo>, Option<String>)>;

    /// Update message count for a session
    async fn update_message_count(&self, id: &SessionId, count: i64) -> Result<()>;

    /// Update session title
    async fn update_title(&self, id: &SessionId, title: &str) -> Result<()>;

    /// Update session `auto_approve_level`
    async fn update_auto_approve_level(&self, id: &SessionId, level: &str) -> Result<u64>;

    /// Delete sessions older than the given number of days
    ///
    /// Returns the IDs of deleted sessions
    async fn cleanup(&self, days: i64) -> Result<Vec<SessionId>>;
}

pub(crate) use crate::storage::storage_err;

pub mod sqlite;
pub use sqlite::SqliteSessionStore;
