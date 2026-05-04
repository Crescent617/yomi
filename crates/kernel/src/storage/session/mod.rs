//! Session management - session lifecycle and metadata storage

use crate::types::{KernelError, Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Session metadata for listing and display
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub parent_id: Option<SessionId>,
    pub title: Option<String>,
    pub message_count: i64,
    pub working_dir: Option<String>,
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
    /// Create a new session with optional working directory
    async fn create(&self, working_dir: Option<&str>) -> Result<SessionId>;

    /// Fork a session, copying its metadata
    async fn fork(&self, parent_id: &SessionId) -> Result<SessionId>;

    /// Get session metadata by ID
    async fn get(&self, id: &SessionId) -> Result<Option<SessionInfo>>;

    /// Delete a session
    async fn delete(&self, id: &SessionId) -> Result<()>;

    /// List all sessions, ordered by `updated_at` descending
    async fn list(&self) -> Result<Vec<SessionInfo>>;

    /// List sessions by working directory
    async fn list_by_working_dir(&self, working_dir: &str) -> Result<Vec<SessionInfo>>;

    /// Update message count for a session
    async fn update_message_count(&self, id: &SessionId, count: i64) -> Result<()>;

    /// Update session title
    async fn update_title(&self, id: &SessionId, title: &str) -> Result<()>;
}

/// Helper for storage errors
fn storage_err(msg: impl Into<String>) -> KernelError {
    KernelError::Storage(msg.into())
}

pub mod sqlite;
pub use sqlite::SqliteSessionStore;
