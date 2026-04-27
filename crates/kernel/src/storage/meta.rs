use crate::types::SessionId;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;

/// Session metadata stored in `SQLite`
#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub parent_id: Option<SessionId>,
    pub title: Option<String>,
    pub message_count: i64,
}

/// SQLite-based metadata storage
#[derive(Debug, Clone)]
pub struct MetaStorage {
    pool: SqlitePool,
}

impl MetaStorage {
    /// Create new `MetaStorage` with `SQLite` pool
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Initialize database schema
    pub async fn init(&self) -> Result<()> {
        sqlx::query(
            r"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                parent_id TEXT,
                title TEXT,
                message_count INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (parent_id) REFERENCES sessions(id) ON DELETE SET NULL
            );
            ",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create sessions table")?;

        sqlx::query(
            r"
            CREATE INDEX IF NOT EXISTS idx_sessions_updated_at 
            ON sessions(updated_at DESC);
            ",
        )
        .execute(&self.pool)
        .await
        .context("Failed to create updated_at index")?;

        Ok(())
    }

    /// Create a new session record
    pub async fn create(&self, id: &SessionId) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id) VALUES (?)")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .context("Failed to create session record")?;
        Ok(())
    }

    /// Create a forked session record
    pub async fn fork(&self, new_id: &SessionId, parent_id: &SessionId) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id, parent_id) VALUES (?, ?)")
            .bind(&new_id.0)
            .bind(&parent_id.0)
            .execute(&self.pool)
            .await
            .context("Failed to create forked session record")?;
        Ok(())
    }

    /// Get session metadata by ID
    pub async fn get(&self, id: &SessionId) -> Result<Option<SessionMeta>> {
        let row = sqlx::query_as::<_, SessionRow>(
            r"
            SELECT id, created_at, updated_at, parent_id, title, message_count 
            FROM sessions 
            WHERE id = ?
            ",
        )
        .bind(&id.0)
        .fetch_optional(&self.pool)
        .await
        .context("Failed to get session record")?;

        Ok(row.map(|r| r.into()))
    }

    /// Delete session record
    pub async fn delete(&self, id: &SessionId) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .context("Failed to delete session record")?;
        Ok(())
    }

    /// Update message count and `updated_at` timestamp
    pub async fn update_message_count(&self, id: &SessionId, message_count: i64) -> Result<()> {
        sqlx::query(
            r"
            UPDATE sessions 
            SET message_count = ?, updated_at = CURRENT_TIMESTAMP 
            WHERE id = ?
            ",
        )
        .bind(message_count)
        .bind(&id.0)
        .execute(&self.pool)
        .await
        .context("Failed to update message count")?;
        Ok(())
    }

    /// Update session title
    pub async fn update_title(&self, id: &SessionId, title: impl Into<String>) -> Result<()> {
        let title = title.into();
        sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
            .bind(&title)
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .context("Failed to update session title")?;
        Ok(())
    }

    /// List all sessions ordered by `updated_at` DESC
    pub async fn list(&self) -> Result<Vec<SessionMeta>> {
        let rows = sqlx::query_as::<_, SessionRow>(
            r"
            SELECT id, created_at, updated_at, parent_id, title, message_count 
            FROM sessions 
            ORDER BY updated_at DESC
            ",
        )
        .fetch_all(&self.pool)
        .await
        .context("Failed to list sessions")?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }
}

/// Internal row type for sqlx mapping
#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    parent_id: Option<String>,
    title: Option<String>,
    message_count: i64,
}

impl From<SessionRow> for SessionMeta {
    fn from(row: SessionRow) -> Self {
        Self {
            id: SessionId(row.id),
            created_at: row.created_at,
            updated_at: row.updated_at,
            parent_id: row.parent_id.map(SessionId),
            title: row.title,
            message_count: row.message_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn create_test_pool() -> SqlitePool {
        sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let pool = create_test_pool().await;
        let storage = MetaStorage::new(pool);
        storage.init().await.unwrap();

        let id = SessionId::new();
        storage.create(&id).await.unwrap();

        let meta = storage.get(&id).await.unwrap().unwrap();
        assert_eq!(meta.id.0, id.0);
        assert_eq!(meta.message_count, 0);
    }

    #[tokio::test]
    async fn test_fork() {
        let pool = create_test_pool().await;
        let storage = MetaStorage::new(pool);
        storage.init().await.unwrap();

        let parent = SessionId::new();
        storage.create(&parent).await.unwrap();

        let child = SessionId::new();
        storage.fork(&child, &parent).await.unwrap();

        let meta = storage.get(&child).await.unwrap().unwrap();
        assert_eq!(meta.parent_id.as_ref().unwrap().0, parent.0);
    }

    #[tokio::test]
    async fn test_update_message_count() {
        let pool = create_test_pool().await;
        let storage = MetaStorage::new(pool);
        storage.init().await.unwrap();

        let id = SessionId::new();
        storage.create(&id).await.unwrap();
        let created_at = storage.get(&id).await.unwrap().unwrap().created_at;

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        storage.update_message_count(&id, 5).await.unwrap();

        let meta = storage.get(&id).await.unwrap().unwrap();
        assert_eq!(meta.message_count, 5);
        assert!(
            meta.updated_at >= created_at,
            "updated_at should be >= created_at"
        );
    }

    #[tokio::test]
    async fn test_list_ordering() {
        let pool = create_test_pool().await;
        let storage = MetaStorage::new(pool);
        storage.init().await.unwrap();

        let id1 = SessionId::new();
        let id2 = SessionId::new();
        storage.create(&id1).await.unwrap();
        storage.create(&id2).await.unwrap();
        // Update id1 to make it more recent
        storage.update_message_count(&id1, 1).await.unwrap();

        let list = storage.list().await.unwrap();
        assert_eq!(list.len(), 2);
        // id1 should be first (more recent updated_at)
        assert_eq!(list[0].id.0, id1.0);
        assert_eq!(list[1].id.0, id2.0);
    }

    #[tokio::test]
    async fn test_delete() {
        let pool = create_test_pool().await;
        let storage = MetaStorage::new(pool);
        storage.init().await.unwrap();

        let id = SessionId::new();
        storage.create(&id).await.unwrap();
        assert!(storage.get(&id).await.unwrap().is_some());

        storage.delete(&id).await.unwrap();
        assert!(storage.get(&id).await.unwrap().is_none());
    }
}
