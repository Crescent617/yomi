use super::{storage_err, PinnedSessionDetail, PinnedSessionInfo, PinnedSessionStore};
use crate::types::{KernelError, ProjectId, Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::SqlitePool;

/// SQLite-backed pinned session store
#[derive(Debug, Clone)]
pub struct SqlitePinnedSessionStore {
    pool: SqlitePool,
}

impl SqlitePinnedSessionStore {
    /// Create a new store
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[derive(sqlx::FromRow)]
struct PinnedSessionRow {
    session_id: String,
    icon_emoji: Option<String>,
    pinned_at: DateTime<Utc>,
}

impl From<PinnedSessionRow> for PinnedSessionInfo {
    fn from(row: PinnedSessionRow) -> Self {
        Self {
            session_id: SessionId::from(row.session_id),
            icon_emoji: row.icon_emoji,
            pinned_at: row.pinned_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct PinnedSessionDetailRow {
    session_id: String,
    title: Option<String>,
    project_id: Option<String>,
    updated_at: DateTime<Utc>,
    icon_emoji: Option<String>,
    pinned_at: DateTime<Utc>,
}

impl From<PinnedSessionDetailRow> for PinnedSessionDetail {
    fn from(row: PinnedSessionDetailRow) -> Self {
        Self {
            session_id: SessionId::from(row.session_id),
            title: row.title,
            project_id: row.project_id.map(ProjectId::from),
            updated_at: row.updated_at,
            icon_emoji: row.icon_emoji,
            pinned_at: row.pinned_at,
        }
    }
}

#[async_trait]
impl PinnedSessionStore for SqlitePinnedSessionStore {
    async fn pin(&self, session_id: &SessionId, emoji: Option<&str>) -> Result<()> {
        sqlx::query(
            "INSERT INTO pinned_sessions (session_id, icon_emoji, pinned_at)
             VALUES (?, ?, CURRENT_TIMESTAMP)
             ON CONFLICT(session_id) DO UPDATE SET
                 icon_emoji = excluded.icon_emoji,
                 pinned_at = excluded.pinned_at",
        )
        .bind(&*session_id.0)
        .bind(emoji)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to pin session: {e}")))?;
        Ok(())
    }

    async fn unpin(&self, session_id: &SessionId) -> Result<()> {
        sqlx::query("DELETE FROM pinned_sessions WHERE session_id = ?")
            .bind(&*session_id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to unpin session: {e}")))?;
        Ok(())
    }

    async fn update_emoji(&self, session_id: &SessionId, emoji: Option<&str>) -> Result<()> {
        let result = sqlx::query("UPDATE pinned_sessions SET icon_emoji = ? WHERE session_id = ?")
            .bind(emoji)
            .bind(&*session_id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to update pinned emoji: {e}")))?;

        if result.rows_affected() == 0 {
            return Err(KernelError::Storage(
                "cannot set emoji on an unpinned session".to_string(),
            ));
        }
        Ok(())
    }

    async fn get(&self, session_id: &SessionId) -> Result<Option<PinnedSessionInfo>> {
        let row = sqlx::query_as::<_, PinnedSessionRow>(
            "SELECT session_id, icon_emoji, pinned_at FROM pinned_sessions WHERE session_id = ?",
        )
        .bind(&*session_id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to get pinned session: {e}")))?;
        Ok(row.map(Into::into))
    }

    async fn list(&self) -> Result<Vec<PinnedSessionInfo>> {
        let rows = sqlx::query_as::<_, PinnedSessionRow>(
            "SELECT session_id, icon_emoji, pinned_at
             FROM pinned_sessions
             ORDER BY pinned_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to list pinned sessions: {e}")))?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn list_with_details(&self) -> Result<Vec<PinnedSessionDetail>> {
        let rows = sqlx::query_as::<_, PinnedSessionDetailRow>(
            "SELECT
                 p.session_id,
                 s.title,
                 s.project_id,
                 s.updated_at,
                 p.icon_emoji,
                 p.pinned_at
             FROM pinned_sessions p
             JOIN sessions s ON s.id = p.session_id
             WHERE s.id NOT LIKE 'sub_%'
             ORDER BY p.pinned_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to list pinned session details: {e}")))?;
        Ok(rows.into_iter().map(Into::into).collect())
    }
}
