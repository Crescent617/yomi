//! Session management - session lifecycle and metadata storage

use crate::types::{KernelError, Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Session metadata for listing and display
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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

/// Default limit for listing sessions (safety cap)
const DEFAULT_LIST_LIMIT: usize = 1000;

/// Arguments for listing sessions with various filters
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[deprecated(since = "0.2.0", note = "Use cursor-based list(project_id, before, limit) instead")]
pub struct ListArgs {
    /// Filter: sessions with `updated_at` < before
    pub before: Option<chrono::DateTime<chrono::Utc>>,
    /// Filter: sessions with `updated_at` > after
    pub after: Option<chrono::DateTime<chrono::Utc>>,
    /// Filter: exact match on working directory
    pub working_dir: Option<String>,
    /// Limit number of results (None = unlimited)
    pub limit: Option<usize>,
    /// Offset for pagination
    pub offset: Option<usize>,
    /// Order by `updated_at` (default: descending)
    pub order_asc: bool,
}

#[allow(deprecated)]
impl Default for ListArgs {
    fn default() -> Self {
        Self {
            before: None,
            after: None,
            working_dir: None,
            limit: Some(DEFAULT_LIST_LIMIT),
            offset: None,
            order_asc: false,
        }
    }
}

/// Storage for session lifecycle and metadata
#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Create a new session with the given ID, optional `project_id`, optional working directory,
    /// and optional `auto_approve_level`
    async fn create(
        &self,
        id: &SessionId,
        project_id: Option<&crate::types::ProjectId>,
        working_dir: Option<&str>,
        auto_approve_level: Option<&str>,
    ) -> Result<()>;

    /// Fork a session, copying its metadata (including auto_approve_level)
    async fn fork(&self, parent_id: &SessionId) -> Result<SessionId>;

    /// Get session metadata by ID
    async fn get(&self, id: &SessionId) -> Result<Option<SessionInfo>>;

    /// Delete a session
    async fn delete(&self, id: &SessionId) -> Result<()>;

    /// List sessions with cursor-based pagination.
    /// `project_id` = None returns all sessions (including independent ones).
    /// Returns `(sessions, has_more)`.
    async fn list(
        &self,
        project_id: Option<&crate::types::ProjectId>,
        before: Option<chrono::DateTime<chrono::Utc>>,
        limit: usize,
    ) -> Result<(Vec<SessionInfo>, bool)>;

    /// Update message count for a session
    async fn update_message_count(&self, id: &SessionId, count: i64) -> Result<()>;

    /// Update session title
    async fn update_title(&self, id: &SessionId, title: &str) -> Result<()>;

    /// Update session auto_approve_level
    async fn update_auto_approve_level(&self, id: &SessionId, level: &str) -> Result<()>;

    /// Delete sessions older than the given number of days
    ///
    /// Returns the IDs of deleted sessions
    async fn cleanup(&self, days: i64) -> Result<Vec<SessionId>>;
}

/// Helper for storage errors
fn storage_err(msg: impl Into<String>) -> KernelError {
    KernelError::Storage(msg.into())
}

pub mod sqlite;
pub use sqlite::SqliteSessionStore;
