//! `SQLite` implementation of `SessionStore`

use super::{storage_err, ListArgs, SessionInfo, SessionStore};
use crate::types::{Result, SessionError, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;
use std::fmt::Write;

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
    async fn create(&self, id: &SessionId, working_dir: Option<&str>) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id, working_dir) VALUES (?, ?)")
            .bind(&id.0)
            .bind(working_dir)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to create session: {e}")))?;
        Ok(())
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
            return Err(SessionError::NotFound { session_id: parent_id.0.clone() }.into());
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

    async fn list(&self, args: ListArgs) -> Result<Vec<SessionInfo>> {
        // Build query dynamically based on filters
        let mut conditions = vec!["1=1"]; // Dummy condition for easier appending
        let mut binds = Vec::new();

        if let Some(before) = args.before {
            conditions.push("updated_at < ?");
            binds.push(before.to_rfc3339());
        }
        if let Some(after) = args.after {
            conditions.push("updated_at > ?");
            binds.push(after.to_rfc3339());
        }
        if let Some(ref wd) = args.working_dir {
            conditions.push("working_dir = ?");
            binds.push(wd.clone());
        }

        let order = if args.order_asc { "ASC" } else { "DESC" };

        let mut query = format!(
            "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir 
                 FROM sessions WHERE {} ORDER BY updated_at {order}",
            conditions.join(" AND ")
        );

        // Add LIMIT and OFFSET in SQL
        if let Some(limit) = args.limit {
            let _ = write!(query, " LIMIT {limit}");
        }
        if let Some(offset) = args.offset {
            let _ = write!(query, " OFFSET {offset}");
        }

        let mut sql_query = sqlx::query_as::<_, SessionRow>(&query);
        for bind in binds {
            sql_query = sql_query.bind(bind);
        }

        let rows = sql_query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to list sessions: {e}")))?;

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

    async fn cleanup(&self, days: i64) -> Result<Vec<SessionId>> {
        const CHUNK_SIZE: usize = 100;

        let cutoff = chrono::Utc::now() - chrono::Duration::days(days);

        // Use list() to get old sessions
        let args = ListArgs {
            before: Some(cutoff),
            limit: None,
            ..Default::default()
        };
        let sessions = self.list(args).await?;

        if sessions.is_empty() {
            return Ok(Vec::new());
        }

        let ids: Vec<String> = sessions.into_iter().map(|s| s.id.0).collect();

        // Delete in chunks to avoid too many parameters
        for chunk in ids.chunks(CHUNK_SIZE) {
            let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let query = format!("DELETE FROM sessions WHERE id IN ({placeholders})");

            let mut sql_query = sqlx::query(&query);
            for id in chunk {
                sql_query = sql_query.bind(id);
            }

            sql_query
                .execute(&self.pool)
                .await
                .map_err(|e| storage_err(format!("failed to delete old sessions: {e}")))?;
        }

        Ok(ids.into_iter().map(SessionId).collect())
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

        let id = SessionId::new();
        store.create(&id, None).await.unwrap();
        let info = store.get(&id).await.unwrap().unwrap();

        assert_eq!(info.id.0, id.0);
        assert_eq!(info.message_count, 0);
    }

    #[tokio::test]
    async fn test_create_with_working_dir() {
        let store = create_test_store().await;

        let id = SessionId::new();
        store.create(&id, Some("/test/dir")).await.unwrap();
        let info = store.get(&id).await.unwrap().unwrap();

        assert_eq!(info.working_dir, Some("/test/dir".to_string()));
    }

    #[tokio::test]
    async fn test_fork() {
        let store = create_test_store().await;

        let parent = SessionId::new();
        store.create(&parent, Some("/parent/dir")).await.unwrap();
        let child = store.fork(&parent).await.unwrap();

        let child_info = store.get(&child).await.unwrap().unwrap();
        assert_eq!(child_info.parent_id.unwrap().0, parent.0);
        assert_eq!(child_info.working_dir, Some("/parent/dir".to_string()));
    }

    #[tokio::test]
    async fn test_list_ordering() {
        let store = create_test_store().await;

        let id1 = SessionId::new();
        store.create(&id1, None).await.unwrap();
        let id2 = SessionId::new();
        store.create(&id2, None).await.unwrap();

        // Update id1 to make it more recent
        store.update_message_count(&id1, 1).await.unwrap();

        let list = store.list(Default::default()).await.unwrap();
        assert_eq!(list[0].id.0, id1.0);
        assert_eq!(list[1].id.0, id2.0);
    }

    #[tokio::test]
    async fn test_list_filter_by_working_dir() {
        let store = create_test_store().await;

        let id1 = SessionId::new();
        store.create(&id1, Some("/foo/bar")).await.unwrap();
        let id2 = SessionId::new();
        store.create(&id2, Some("/baz/qux")).await.unwrap();
        let id3 = SessionId::new();
        store.create(&id3, Some("/foo/bar")).await.unwrap();

        let args = ListArgs {
            working_dir: Some("/foo/bar".to_string()),
            ..Default::default()
        };
        let list = store.list(args).await.unwrap();
        assert_eq!(list.len(), 2);
        let ids: Vec<_> = list.iter().map(|s| &s.id.0).collect();
        assert!(ids.contains(&&id1.0));
        assert!(ids.contains(&&id3.0));
    }

    #[tokio::test]
    async fn test_list_limit_offset() {
        let store = create_test_store().await;

        // Create 5 sessions with explicit delays to ensure different timestamps
        let mut ids = Vec::new();
        for i in 0..5 {
            let id = SessionId::new();
            store.create(&id, None).await.unwrap();
            ids.push(id);
            // Update message count to change updated_at, with increasing delays
            tokio::time::sleep(tokio::time::Duration::from_millis(10 + i as u64 * 5)).await;
            store
                .update_message_count(&ids[i], i as i64 + 1)
                .await
                .unwrap();
        }

        // Test limit
        let args = ListArgs {
            limit: Some(2),
            ..Default::default()
        };
        let list = store.list(args).await.unwrap();
        assert_eq!(list.len(), 2);

        // Test offset - get middle 2 sessions
        let args = ListArgs {
            limit: Some(2),
            offset: Some(1),
            ..Default::default()
        };
        let offset_list = store.list(args).await.unwrap();
        assert_eq!(offset_list.len(), 2);

        // Full list for comparison
        let full_list = store.list(Default::default()).await.unwrap();
        assert_eq!(full_list.len(), 5);

        // Offset results should be different from first page
        assert_ne!(offset_list[0].id.0, full_list[0].id.0);
    }

    #[tokio::test]
    async fn test_cleanup_deletes_old_sessions() {
        let store = create_test_store().await;

        // Create a session and manually set its updated_at to 10 days ago
        let old_id = SessionId::new();
        store.create(&old_id, Some("/test")).await.unwrap();
        sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-10 days') WHERE id = ?")
            .bind(&old_id.0)
            .execute(&store.pool)
            .await
            .unwrap();

        // Create a recent session
        let recent_id = SessionId::new();
        store.create(&recent_id, Some("/test")).await.unwrap();

        // Cleanup sessions older than 7 days
        let deleted = store.cleanup(7).await.unwrap();
        assert_eq!(deleted.len(), 1);
        assert_eq!(deleted[0].0, old_id.0);

        // Verify old session is gone
        let old_session = store.get(&old_id).await.unwrap();
        assert!(old_session.is_none());

        // Verify recent session still exists
        let recent_session = store.get(&recent_id).await.unwrap();
        assert!(recent_session.is_some());
    }

    #[tokio::test]
    async fn test_cleanup_empty_when_no_old_sessions() {
        let store = create_test_store().await;

        // Create only recent sessions
        let id1 = SessionId::new();
        store.create(&id1, None).await.unwrap();
        let id2 = SessionId::new();
        store.create(&id2, None).await.unwrap();

        // Cleanup sessions older than 30 days
        let deleted = store.cleanup(30).await.unwrap();
        assert!(deleted.is_empty());

        // Verify all sessions still exist
        let all = store.list(Default::default()).await.unwrap();
        assert_eq!(all.len(), 2);
    }
}
