//! Session management - session lifecycle and metadata storage

use crate::types::{Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionListScope {
    All,
    Assigned,
}

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
        format_age(self.updated_at)
    }
}

/// Format a timestamp as a relative age ("2d ago", "3h ago", "5m ago",
/// "just now").
pub fn format_age(ts: DateTime<Utc>) -> String {
    let age = Utc::now() - ts;
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
        scope: SessionListScope,
        before: Option<chrono::DateTime<chrono::Utc>>,
        limit: usize,
    ) -> Result<(Vec<SessionInfo>, Option<String>)>;

    /// List direct subagent children of a parent session, newest first.
    async fn list_subagents(&self, parent_id: &SessionId) -> Result<Vec<SessionInfo>>;

    /// Update message count for a session
    async fn update_message_count(&self, id: &SessionId, count: i64) -> Result<()>;

    /// Update session title
    async fn update_title(&self, id: &SessionId, title: &str) -> Result<()>;

    /// Update session `auto_approve_level`
    async fn update_auto_approve_level(&self, id: &SessionId, level: &str) -> Result<u64>;

    /// List expired session IDs: `updated_at` older than `cutoff`.
    ///
    /// The returned set includes:
    /// - regular (non-subagent) expired sessions
    /// - child subagent sessions of those expired parents (regardless of own age)
    /// - orphaned subagent sessions (`parent_id IS NULL`) that are themselves expired
    ///
    /// Subagent sessions whose parent is still alive are never returned.
    /// When `keep_pinned` is true, pinned sessions (and their children) are excluded.
    async fn list_expired(
        &self,
        cutoff: DateTime<Utc>,
        keep_pinned: bool,
    ) -> Result<Vec<SessionId>>;

    /// Delete sessions by ID in batches. Returns the number of rows deleted.
    async fn delete_batch(&self, ids: &[SessionId]) -> Result<u64>;

    /// List all session IDs belonging to a project, including subagent
    /// children of those sessions (used by project cascade deletion).
    async fn list_ids_by_project(
        &self,
        project_id: &crate::types::ProjectId,
    ) -> Result<Vec<SessionId>>;
}

pub(crate) use crate::storage::storage_err;

pub mod sqlite;
pub use sqlite::SqliteSessionStore;
