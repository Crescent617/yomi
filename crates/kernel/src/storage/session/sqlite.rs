//! `SQLite` implementation of `SessionStore`

use super::{storage_err, SessionInfo, SessionStore};
use crate::types::{Result, SessionId};
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
    async fn create(
        &self,
        id: &SessionId,
        project_id: Option<&crate::types::ProjectId>,
        working_dir: Option<&str>,
        auto_approve_level: Option<&str>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id, project_id, working_dir, auto_approve_level) VALUES (?, ?, ?, ?)")
            .bind(&id.0)
            .bind(project_id.map(|p| &p.0))
            .bind(working_dir)
            .bind(auto_approve_level)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to create session: {e}")))?;
        Ok(())
    }

    async fn fork(&self, parent_id: &SessionId) -> Result<SessionId> {
        let new_id = SessionId::new();
        sqlx::query(
            "INSERT INTO sessions (id, parent_id, project_id, working_dir, auto_approve_level)
             SELECT ?, ?, project_id, working_dir, auto_approve_level FROM sessions WHERE id = ?",
        )
        .bind(&new_id.0)
        .bind(&parent_id.0)
        .bind(&parent_id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to fork session: {e}")))?;

        Ok(new_id)
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionInfo>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir, project_id, auto_approve_level
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

    async fn list(
        &self,
        project_id: Option<&crate::types::ProjectId>,
        before: Option<chrono::DateTime<chrono::Utc>>,
        limit: usize,
    ) -> Result<(Vec<SessionInfo>, bool)> {
        let mut conditions = vec!["1=1"];
        let mut binds: Vec<String> = Vec::new();

        if let Some(pid) = project_id {
            conditions.push("project_id = ?");
            binds.push(pid.0.clone());
        }
        if let Some(before) = before {
            conditions.push("updated_at < ?");
            binds.push(before.to_rfc3339());
        }

        let query = format!(
            "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir, project_id, auto_approve_level
             FROM sessions WHERE {} ORDER BY updated_at DESC LIMIT {}",
            conditions.join(" AND "),
            limit + 1,
        );

        let mut sql_query = sqlx::query_as::<_, SessionRow>(&query);
        for bind in binds {
            sql_query = sql_query.bind(bind);
        }

        let rows = sql_query
            .fetch_all(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to list sessions: {e}")))?;

        let has_more = rows.len() > limit;
        let sessions: Vec<SessionInfo> = rows.into_iter().take(limit).map(Into::into).collect();
        Ok((sessions, has_more))
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
        let result = sqlx::query(
            "UPDATE sessions SET title = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(title)
        .bind(&id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to update session title: {e}")))?;

        tracing::info!(
            "update_title: id={}, title={}, rows_affected={}",
            id.0,
            title,
            result.rows_affected()
        );
        Ok(())
    }

    async fn update_auto_approve_level(&self, id: &SessionId, level: &str) -> Result<()> {
        let result = sqlx::query(
            "UPDATE sessions SET auto_approve_level = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(level)
        .bind(&id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to update session level: {e}")))?;

        tracing::info!(
            "update_auto_approve_level: id={}, level={}, rows_affected={}",
            id.0,
            level,
            result.rows_affected()
        );
        Ok(())
    }

    async fn cleanup(&self, days: i64) -> Result<Vec<SessionId>> {
        const CHUNK_SIZE: usize = 100;

        let cutoff = chrono::Utc::now() - chrono::Duration::days(days);

        // Use list() to get old sessions
        let (sessions, _) = self.list(None, Some(cutoff), 10000).await?;

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
    project_id: Option<String>,
    auto_approve_level: Option<String>,
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
            project_id: row.project_id.map(crate::types::ProjectId),
            auto_approve_level: row.auto_approve_level,
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
        store.create(&id, None, None, None).await.unwrap();
        let info = store.get(&id).await.unwrap().unwrap();

        assert_eq!(info.id.0, id.0);
        assert_eq!(info.message_count, 0);
    }

    #[tokio::test]
    async fn test_create_with_working_dir() {
        let store = create_test_store().await;

        let id = SessionId::new();
        store.create(&id, None, Some("/test/dir"), None).await.unwrap();
        let info = store.get(&id).await.unwrap().unwrap();

        assert_eq!(info.working_dir, Some("/test/dir".to_string()));
    }

    #[tokio::test]
    async fn test_fork() {
        let store = create_test_store().await;

        let parent = SessionId::new();
        store.create(&parent, None, Some("/parent/dir"), None).await.unwrap();
        let child = store.fork(&parent).await.unwrap();

        let child_info = store.get(&child).await.unwrap().unwrap();
        assert_eq!(child_info.parent_id.unwrap().0, parent.0);
        assert_eq!(child_info.working_dir, Some("/parent/dir".to_string()));
    }

    #[tokio::test]
    async fn test_list_ordering() {
        let store = create_test_store().await;

        let id1 = SessionId::new();
        store.create(&id1, None, None, None).await.unwrap();
        let id2 = SessionId::new();
        store.create(&id2, None, None, None).await.unwrap();

        // Update id1 to make it more recent
        store.update_message_count(&id1, 1).await.unwrap();

        let (list, _) = store.list(None, None, 100).await.unwrap();
        assert_eq!(list[0].id.0, id1.0);
        assert_eq!(list[1].id.0, id2.0);
    }

    #[tokio::test]
    async fn test_list_filter_by_project_id() {
        let store = create_test_store().await;

        let pid = crate::types::ProjectId::new();
        let id1 = SessionId::new();
        store.create(&id1, Some(&pid), Some("/foo/bar"), None).await.unwrap();
        let id2 = SessionId::new();
        store.create(&id2, None, Some("/baz/qux"), None).await.unwrap();
        let id3 = SessionId::new();
        store.create(&id3, Some(&pid), Some("/foo/bar"), None).await.unwrap();

        let (list, _) = store.list(Some(&pid), None, 100).await.unwrap();
        assert_eq!(list.len(), 2);
        let ids: Vec<_> = list.iter().map(|s| &s.id.0).collect();
        assert!(ids.contains(&&id1.0));
        assert!(ids.contains(&&id3.0));
    }

    #[tokio::test]
    async fn test_list_limit_and_has_more() {
        let store = create_test_store().await;

        // Create 5 sessions with explicit delays to ensure different timestamps
        let mut ids = Vec::new();
        for i in 0..5 {
            let id = SessionId::new();
            store.create(&id, None, None, None).await.unwrap();
            ids.push(id);
            // Update message count to change updated_at, with increasing delays
            tokio::time::sleep(tokio::time::Duration::from_millis(10 + i as u64 * 5)).await;
            store
                .update_message_count(&ids[i], i as i64 + 1)
                .await
                .unwrap();
        }

        // Test limit
        let (list, has_more) = store.list(None, None, 2).await.unwrap();
        assert_eq!(list.len(), 2);
        assert!(has_more);

        // Get next page using cursor
        let before = list.last().unwrap().updated_at;
        let (next_list, next_has_more) = store.list(None, Some(before), 2).await.unwrap();
        assert_eq!(next_list.len(), 2);
        assert!(next_has_more);

        // Full list for comparison
        let (full_list, full_has_more) = store.list(None, None, 100).await.unwrap();
        assert_eq!(full_list.len(), 5);
        assert!(!full_has_more);

        // Next page results should be different from first page
        assert_ne!(next_list[0].id.0, list[0].id.0);
    }

    #[tokio::test]
    async fn test_cleanup_deletes_old_sessions() {
        let store = create_test_store().await;

        // Create a session and manually set its updated_at to 10 days ago
        let old_id = SessionId::new();
        store.create(&old_id, None, Some("/test"), None).await.unwrap();
        sqlx::query("UPDATE sessions SET updated_at = datetime('now', '-10 days') WHERE id = ?")
            .bind(&old_id.0)
            .execute(&store.pool)
            .await
            .unwrap();

        // Create a recent session
        let recent_id = SessionId::new();
        store.create(&recent_id, None, Some("/test"), None).await.unwrap();

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
        store.create(&id1, None, None, None).await.unwrap();
        let id2 = SessionId::new();
        store.create(&id2, None, None, None).await.unwrap();

        // Cleanup sessions older than 30 days
        let deleted = store.cleanup(30).await.unwrap();
        assert!(deleted.is_empty());

        // Verify all sessions still exist
        let (all, _) = store.list(None, None, 100).await.unwrap();
        assert_eq!(all.len(), 2);
    }
}
