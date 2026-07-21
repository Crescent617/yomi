//! Favorite answer storage - snapshots of assistant text answers.
//!
//! Favorites snapshot the answer content so they survive session deletion
//! and compaction. `session_id` / `message_id` are kept only as metadata
//! for navigating back to the source when it still exists.

use crate::types::{MessageId, Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// A favorited assistant answer (content snapshot)
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FavoriteAnswer {
    pub id: String,
    pub session_id: SessionId,
    pub message_id: MessageId,
    /// Session title captured at favorite time (survives session deletion)
    pub session_title: Option<String>,
    /// Markdown snapshot of the answer text
    pub content: String,
    /// Optional user note
    pub note: Option<String>,
    pub favorited_at: DateTime<Utc>,
    /// Original answer timestamp, if known
    pub message_created_at: Option<DateTime<Utc>>,
}

/// Input for adding a favorite
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AddFavoriteInput {
    pub session_id: SessionId,
    pub message_id: MessageId,
    pub session_title: Option<String>,
    pub content: String,
    pub note: Option<String>,
    pub message_created_at: Option<DateTime<Utc>>,
}

/// Storage for favorited assistant answers
#[async_trait]
pub trait FavoriteStore: Send + Sync {
    /// Add a favorite. Re-favoriting the same message refreshes the snapshot.
    async fn add(&self, input: AddFavoriteInput) -> Result<FavoriteAnswer>;

    /// Remove a favorite by its id
    async fn remove(&self, id: &str) -> Result<()>;

    /// Remove a favorite by its source message
    async fn remove_by_message(&self, session_id: &SessionId, message_id: &MessageId)
        -> Result<()>;

    /// Get the favorite for a source message, if any
    async fn get_by_message(
        &self,
        session_id: &SessionId,
        message_id: &MessageId,
    ) -> Result<Option<FavoriteAnswer>>;

    /// List favorites, most recent first. `query` filters content/note/title.
    async fn list(
        &self,
        query: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<FavoriteAnswer>>;

    /// Update the note for a favorite
    async fn update_note(&self, id: &str, note: Option<&str>) -> Result<()>;
}

pub(crate) use crate::storage::storage_err;

pub mod sqlite;
pub use sqlite::SqliteFavoriteStore;
