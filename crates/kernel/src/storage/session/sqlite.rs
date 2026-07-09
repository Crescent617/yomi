//! `SQLite` implementation of `SessionStore`

use super::{storage_err, SessionInfo, SessionStore};
use crate::types::{Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;
use sqlx::Row;

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
        parent_id: Option<&SessionId>,
        model_key: Option<&str>,
    ) -> Result<()> {
        sqlx::query("INSERT INTO sessions (id, project_id, working_dir, auto_approve_level, parent_id, model_key) VALUES (?, ?, ?, ?, ?, ?)")
            .bind(&*id.0)
            .bind(project_id.map(|p| &*p.0))
            .bind(working_dir)
            .bind(auto_approve_level)
            .bind(parent_id.map(|p| &*p.0))
            .bind(model_key)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to create session: {e}")))?;
        Ok(())
    }

    async fn fork(&self, parent_id: &SessionId) -> Result<SessionId> {
        let new_id = SessionId::new();
        sqlx::query(
            "INSERT INTO sessions (id, parent_id, project_id, working_dir, auto_approve_level, model_key)
             SELECT ?, ?, project_id, working_dir, auto_approve_level, model_key FROM sessions WHERE id = ?",
        )
        .bind(&*new_id.0)
        .bind(&*parent_id.0)
        .bind(&*parent_id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to fork session: {e}")))?;

        Ok(new_id)
    }

    async fn update_model_key(&self, id: &SessionId, key: &str) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE sessions SET model_key = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(key)
        .bind(&*id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to update session model key: {e}")))?;

        tracing::info!(
            "update_model_key: id={}, key={}, rows_affected={}",
            id.0,
            key,
            result.rows_affected()
        );
        Ok(result.rows_affected())
    }

    async fn get(&self, id: &SessionId) -> Result<Option<SessionInfo>> {
        let row = sqlx::query_as::<_, SessionRow>(
            "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir, project_id, auto_approve_level, model_key
             FROM sessions WHERE id = ?",
        )
        .bind(&*id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to get session: {e}")))?;

        Ok(row.map(Into::into))
    }

    async fn delete(&self, id: &SessionId) -> Result<()> {
        // Cascade: delete child subagent sessions so they don't become orphaned
        // by the ON DELETE SET NULL foreign key constraint.
        sqlx::query("DELETE FROM sessions WHERE parent_id = ? AND id LIKE 'sub_%'")
            .bind(&*id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to delete child sessions: {e}")))?;

        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&*id.0)
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
    ) -> Result<(Vec<SessionInfo>, Option<String>)> {
        let mut builder = sqlx::QueryBuilder::new(
            "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir, project_id, auto_approve_level, model_key
             FROM sessions WHERE id NOT LIKE 'sub_%'",
        );

        if let Some(pid) = project_id {
            builder.push(" AND project_id = ");
            builder.push_bind(&*pid.0);
        }
        if let Some(before) = before {
            builder.push(" AND updated_at < ");
            builder.push_bind(before.format("%Y-%m-%d %H:%M:%S").to_string());
        }

        builder.push(" ORDER BY updated_at DESC LIMIT ");
        builder.push_bind((limit + 1) as i64);

        let rows = builder
            .build_query_as::<SessionRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to list sessions: {e}")))?;

        let has_more = rows.len() > limit;
        let sessions: Vec<SessionInfo> = rows.into_iter().take(limit).map(Into::into).collect();
        let next_cursor = has_more.then(|| {
            sessions
                .last()
                .map(|s| s.updated_at.to_rfc3339())
                .unwrap_or_default()
        });
        Ok((sessions, next_cursor))
    }

    async fn update_message_count(&self, id: &SessionId, count: i64) -> Result<()> {
        sqlx::query(
            "UPDATE sessions SET message_count = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(count)
        .bind(&*id.0)
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
        .bind(&*id.0)
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

    async fn update_auto_approve_level(&self, id: &SessionId, level: &str) -> Result<u64> {
        let result = sqlx::query(
            "UPDATE sessions SET auto_approve_level = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(level)
        .bind(&*id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to update session level: {e}")))?;

        tracing::info!(
            "update_auto_approve_level: id={}, level={}, rows_affected={}",
            id.0,
            level,
            result.rows_affected()
        );
        Ok(result.rows_affected())
    }

    async fn cleanup(&self, days: i64) -> Result<Vec<SessionId>> {
        const CHUNK_SIZE: usize = 100;

        let cutoff = chrono::Utc::now() - chrono::Duration::days(days);

        let rows = sqlx::query(
            "SELECT id FROM sessions WHERE updated_at < ? ORDER BY updated_at DESC LIMIT 10000",
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to query old sessions: {e}")))?;

        if rows.is_empty() {
            return Ok(Vec::new());
        }

        let mut ids: Vec<String> = rows
            .into_iter()
            .map(|r| r.try_get::<String, _>("id").unwrap_or_default())
            .collect();

        // Also collect child subagent sessions whose parent is being deleted,
        // so they don't become orphaned by ON DELETE SET NULL.
        let child_rows = sqlx::query(
            "SELECT child.id FROM sessions AS child
             WHERE child.id LIKE 'sub_%'
             AND EXISTS (
                 SELECT 1 FROM sessions AS parent
                 WHERE parent.id = child.parent_id
                 AND parent.updated_at < ?
             )",
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to query child sessions: {e}")))?;

        for r in child_rows {
            let id = r.try_get::<String, _>("id").unwrap_or_default();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }

        // Delete in chunks to avoid too many parameters
        for chunk in ids.chunks(CHUNK_SIZE) {
            let mut builder = sqlx::QueryBuilder::new("DELETE FROM sessions WHERE id IN (");
            let mut separated = builder.separated(", ");
            for id in chunk {
                separated.push_bind(id);
            }
            separated.push_unseparated(")");

            builder
                .build()
                .execute(&self.pool)
                .await
                .map_err(|e| storage_err(format!("failed to delete old sessions: {e}")))?;
        }

        Ok(ids.into_iter().map(SessionId::from).collect())
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
    model_key: Option<String>,
}

impl From<SessionRow> for SessionInfo {
    fn from(row: SessionRow) -> Self {
        Self {
            id: SessionId::from(row.id),
            created_at: row.created_at,
            updated_at: row.updated_at,
            parent_id: row.parent_id.map(SessionId::from),
            title: row.title,
            message_count: row.message_count,
            working_dir: row.working_dir,
            project_id: row.project_id.map(crate::types::ProjectId::from),
            auto_approve_level: row.auto_approve_level,
            model_key: row.model_key,
        }
    }
}

#[cfg(test)]
#[path = "sqlite_test.rs"]
mod tests;
