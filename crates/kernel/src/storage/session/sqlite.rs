//! `SQLite` implementation of `SessionStore`

use super::{storage_err, SessionInfo, SessionStore};
use crate::types::{KernelError, Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;

/// SQLite-based session storage
#[derive(Debug, Clone)]
pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    /// Create new store with `SQLite` pool
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl SessionStore for SqliteSessionStore {
    async fn create(&self, working_dir: Option<&str>) -> Result<SessionId> {
        let id = SessionId::new();
        sqlx::query("INSERT INTO sessions (id, working_dir) VALUES (?, ?)")
            .bind(&id.0)
            .bind(working_dir)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to create session: {e}")))?;
        Ok(id)
    }

    async fn fork(&self, parent_id: &SessionId) -> Result<SessionId> {
        // Get parent's working_dir
        let parent_working_dir: Option<String> =
            sqlx::query_scalar("SELECT working_dir FROM sessions WHERE id = ?")
                .bind(&parent_id.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| storage_err(format!("failed to get parent session: {e}")))?;

        if parent_working_dir.is_none() {
            return Err(KernelError::Session(format!(
                "parent session '{}' does not exist",
                parent_id.0
            )));
        }

        let new_id = SessionId::new();
        sqlx::query("INSERT INTO sessions (id, parent_id, working_dir) VALUES (?, ?, ?)")
            .bind(&new_id.0)
            .bind(&parent_id.0)
            .bind(parent_working_dir)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to fork session: {e}")))?;

        Ok(new_id)
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionInfo>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir 
             FROM sessions WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to get session: {e}")))?;

        Ok(row.map(Into::into))
    }

    async fn delete(&self, id: &SessionId) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to delete session: {e}")))?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<SessionInfo>> {
        let rows = sqlx::query_as::<_, SessionRow>(
            "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir 
             FROM sessions ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to list sessions: {e}")))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn list_by_working_dir(&self, working_dir: &str) -> Result<Vec<SessionInfo>> {
        let rows = sqlx::query_as::<_, SessionRow>(
            "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir 
             FROM sessions WHERE working_dir = ? ORDER BY updated_at DESC",
        )
        .bind(working_dir)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to list sessions by working dir: {e}")))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn update_message_count(&self, id: &SessionId, count: i64) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET message_count = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(count)
        .bind(&id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to update message count: {e}")))?;
        Ok(())
    }

    async fn update_title(&self, id: &SessionId, title: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
            .bind(title)
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to update session title: {e}")))?;
        Ok(())
    }
}

/// Internal row type for SQL mapping
#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    parent_id: Option<String>,
    title: Option<String>,
    message_count: i64,
    working_dir: Option<String>,
}

impl From<SessionRow> for SessionInfo {
    fn from(row: SessionRow) -> Self {
        Self {
            id: SessionId(row.id),
            created_at: row.created_at,
            updated_at: row.updated_at,
            parent_id: row.parent_id.map(SessionId),
            title: row.title,
            message_count: row.message_count,
            working_dir: row.working_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::run_migrations;

    async fn create_test_store() -> SqliteSessionStore {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        SqliteSessionStore::new(pool)
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let store = create_test_store().await;

        let id = store.create(None).await.unwrap();
        let info = store.get(&id).await.unwrap().unwrap();

        assert_eq!(info.id.0, id.0);
        assert_eq!(info.message_count, 0);
    }

    #[tokio::test]
    async fn test_create_with_working_dir() {
        let store = create_test_store().await;

        let id = store.create(Some("/test/dir")).await.unwrap();
        let info = store.get(&id).await.unwrap().unwrap();

        assert_eq!(info.working_dir, Some("/test/dir".to_string()));
    }

    #[tokio::test]
    async fn test_fork() {
        let store = create_test_store().await;

        let parent = store.create(Some("/parent/dir")).await.unwrap();
        let child = store.fork(&parent).await.unwrap();

        let child_info = store.get(&child).await.unwrap().unwrap();
        assert_eq!(child_info.parent_id.unwrap().0, parent.0);
        assert_eq!(child_info.working_dir, Some("/parent/dir".to_string()));
    }

    #[tokio::test]
    async fn test_list_ordering() {
        let store = create_test_store().await;

        let id1 = store.create(None).await.unwrap();
        let id2 = store.create(None).await.unwrap();

        // Update id1 to make it more recent
        store.update_message_count(&id1, 1).await.unwrap();

        let list = store.list().await.unwrap();
        assert_eq!(list[0].id.0, id1.0);
        assert_eq!(list[1].id.0, id2.0);
    }
}
