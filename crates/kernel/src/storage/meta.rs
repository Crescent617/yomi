use crate::types::{KernelError, Result, SessionId};
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
    pub working_dir: Option<String>,
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
    ///
    /// Note: Schema is now managed by the migrations system in `crate::storage::migrations`.
    /// This method is kept for backwards compatibility and testing purposes.
    /// In production, migrations are run automatically when creating `FsStorage`.
    pub async fn init(&self) -> Result<()> {
        // Schema is managed by migrations system
        // This method exists for tests that need to ensure tables exist
        // without going through the full migration system
        Ok(())
    }

    /// Create a new session record with optional working directory
    pub async fn create(&self, id: &SessionId, working_dir: Option<&str>) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id, working_dir) VALUES (?, ?)")
            .bind(&id.0)
            .bind(working_dir)
            .execute(&self.pool)
            .await
            .map_err(|e| KernelError::storage(format!("Failed to create session record: {e}")))?;
        Ok(())
    }

    /// Create a forked session record, inheriting `working_dir` from parent
    pub async fn fork(&self, new_id: &SessionId, parent_id: &SessionId) -> Result<()> {
        // Get parent's working_dir and verify parent exists
        let parent_working_dir: Option<String> =
            sqlx::query_scalar("SELECT working_dir FROM sessions WHERE id = ?")
                .bind(&parent_id.0)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| {
                    KernelError::storage(format!("Failed to get parent session working_dir: {e}"))
                })?;

        // Check if parent exists
        if parent_working_dir.is_none() {
            return Err(KernelError::session(format!(
                "Cannot fork session: parent session '{}' does not exist",
                parent_id.0
            )));
        }

        sqlx::query("INSERT INTO sessions (id, parent_id, working_dir) VALUES (?, ?, ?)")
            .bind(&new_id.0)
            .bind(&parent_id.0)
            .bind(parent_working_dir)
            .execute(&self.pool)
            .await
            .map_err(|e| {
                KernelError::storage(format!("Failed to create forked session record: {e}"))
            })?;
        Ok(())
    }

    /// Get session metadata by ID
    pub async fn get(&self, id: &SessionId) -> Result<Option<SessionMeta>> {
        let row = sqlx::query_as::<_, SessionRow>(
            r"
            SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir
            FROM sessions
            WHERE id = ?
            ",
        )
        .bind(&id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to get session record: {e}")))?;

        Ok(row.map(|r| r.into()))
    }

    /// Delete session record
    pub async fn delete(&self, id: &SessionId) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| KernelError::storage(format!("Failed to delete session record: {e}")))?;
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
        .map_err(|e| KernelError::storage(format!("Failed to update message count: {e}")))?;
        Ok(())
    }

    /// Update session title (also used for last user message preview)
    pub async fn update_title(&self, id: &SessionId, title: impl Into<String>) -> Result<()> {
        let title = title.into();
        sqlx::query("UPDATE sessions SET title = ? WHERE id = ?")
            .bind(&title)
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| KernelError::storage(format!("Failed to update session title: {e}")))?;
        Ok(())
    }

    /// List all sessions ordered by `updated_at` DESC
    pub async fn list(&self) -> Result<Vec<SessionMeta>> {
        let rows = sqlx::query_as::<_, SessionRow>(
            r"
            SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir
            FROM sessions
            ORDER BY updated_at DESC
            ",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to list sessions: {e}")))?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    /// List sessions by working directory ordered by `updated_at` DESC
    pub async fn list_by_working_dir(&self, working_dir: &str) -> Result<Vec<SessionMeta>> {
        let rows = sqlx::query_as::<_, SessionRow>(
            r"
            SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir
            FROM sessions
            WHERE working_dir = ?
            ORDER BY updated_at DESC
            ",
        )
        .bind(working_dir)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| {
            KernelError::storage(format!("Failed to list sessions by working directory: {e}"))
        })?;

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
    working_dir: Option<String>,
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
            working_dir: row.working_dir,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::run_migrations;

    async fn create_test_storage() -> MetaStorage {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        MetaStorage::new(pool)
    }

    #[tokio::test]
    async fn test_create_and_get() {
        let storage = create_test_storage().await;

        let id = SessionId::new();
        storage.create(&id, None).await.unwrap();

        let meta = storage.get(&id).await.unwrap().unwrap();
        assert_eq!(meta.id.0, id.0);
        assert_eq!(meta.message_count, 0);
        assert_eq!(meta.working_dir, None);
    }

    #[tokio::test]
    async fn test_create_with_working_dir() {
        let storage = create_test_storage().await;

        let id = SessionId::new();
        storage
            .create(&id, Some("/test/working/dir"))
            .await
            .unwrap();

        let meta = storage.get(&id).await.unwrap().unwrap();
        assert_eq!(meta.working_dir, Some("/test/working/dir".to_string()));
    }

    #[tokio::test]
    async fn test_fork() {
        let storage = create_test_storage().await;

        let parent = SessionId::new();
        storage.create(&parent, Some("/parent/dir")).await.unwrap();

        let child = SessionId::new();
        storage.fork(&child, &parent).await.unwrap();

        let meta = storage.get(&child).await.unwrap().unwrap();
        assert_eq!(meta.parent_id.as_ref().unwrap().0, parent.0);
        // Check working_dir was inherited
        assert_eq!(meta.working_dir, Some("/parent/dir".to_string()));
    }

    #[tokio::test]
    async fn test_update_message_count() {
        let storage = create_test_storage().await;

        let id = SessionId::new();
        storage.create(&id, None).await.unwrap();
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
        let storage = create_test_storage().await;

        let id1 = SessionId::new();
        let id2 = SessionId::new();
        storage.create(&id1, None).await.unwrap();
        storage.create(&id2, None).await.unwrap();
        // Update id1 to make it more recent
        storage.update_message_count(&id1, 1).await.unwrap();

        let list = storage.list().await.unwrap();
        assert_eq!(list.len(), 2);
        // id1 should be first (more recent updated_at)
        assert_eq!(list[0].id.0, id1.0);
        assert_eq!(list[1].id.0, id2.0);
    }

    #[tokio::test]
    async fn test_list_by_working_dir() {
        let storage = create_test_storage().await;

        let id1 = SessionId::new();
        let id2 = SessionId::new();
        let id3 = SessionId::new();
        storage.create(&id1, Some("/dir/a")).await.unwrap();
        storage.create(&id2, Some("/dir/b")).await.unwrap();
        storage.create(&id3, None).await.unwrap();

        let list_a = storage.list_by_working_dir("/dir/a").await.unwrap();
        assert_eq!(list_a.len(), 1);
        assert_eq!(list_a[0].id.0, id1.0);

        let list_b = storage.list_by_working_dir("/dir/b").await.unwrap();
        assert_eq!(list_b.len(), 1);
        assert_eq!(list_b[0].id.0, id2.0);

        let list_all = storage.list().await.unwrap();
        assert_eq!(list_all.len(), 3);
    }

    #[tokio::test]
    async fn test_delete() {
        let storage = create_test_storage().await;

        let id = SessionId::new();
        storage.create(&id, None).await.unwrap();
        assert!(storage.get(&id).await.unwrap().is_some());

        storage.delete(&id).await.unwrap();
        assert!(storage.get(&id).await.unwrap().is_none());
    }
}
