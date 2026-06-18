//! Pinned session storage - keeps pinned state and emoji in a separate table
//! so the main `sessions` table and its public interfaces stay unchanged.

use crate::types::{Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Metadata for a pinned session entry
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PinnedSessionInfo {
    pub session_id: SessionId,
    pub icon_emoji: Option<String>,
    pub pinned_at: DateTime<Utc>,
}

/// Full pinned session row, joined with `sessions` for UI rendering
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PinnedSessionDetail {
    pub session_id: SessionId,
    pub title: Option<String>,
    pub project_id: Option<crate::types::ProjectId>,
    pub updated_at: DateTime<Utc>,
    pub icon_emoji: Option<String>,
    pub pinned_at: DateTime<Utc>,
}

/// Storage for pinned session metadata
#[async_trait]
pub trait PinnedSessionStore: Send + Sync {
    /// Pin a session, optionally with an emoji. Re-pinning updates `pinned_at`.
    async fn pin(&self, session_id: &SessionId, emoji: Option<&str>) -> Result<()>;

    /// Unpin a session
    async fn unpin(&self, session_id: &SessionId) -> Result<()>;

    /// Update the emoji for an already-pinned session
    async fn update_emoji(&self, session_id: &SessionId, emoji: Option<&str>) -> Result<()>;

    /// Get a single pinned session entry
    async fn get(&self, session_id: &SessionId) -> Result<Option<PinnedSessionInfo>>;

    /// List all pinned sessions, ordered by most recently pinned first
    async fn list(&self) -> Result<Vec<PinnedSessionInfo>>;

    /// List pinned sessions joined with session metadata for the UI
    async fn list_with_details(&self) -> Result<Vec<PinnedSessionDetail>>;
}

pub(crate) use crate::storage::storage_err;

pub mod sqlite;
pub use sqlite::SqlitePinnedSessionStore;
