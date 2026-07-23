//! `SQLite` implementation of `SessionStore`

use super::{storage_err, SessionInfo, SessionListScope, SessionStore};
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
            "INSERT INTO sessions (id, project_id, working_dir, auto_approve_level, model_key)
             SELECT ?, project_id, working_dir, auto_approve_level, model_key FROM sessions WHERE id = ?",
        )
        .bind(&*new_id.0)
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
        scope: crate::storage::session::SessionListScope,
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
        if matches!(scope, SessionListScope::Assigned) {
            builder.push(" AND project_id IS NOT NULL");
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

    async fn list_subagents(&self, parent_id: &SessionId) -> Result<Vec<SessionInfo>> {
        let rows = sqlx::query_as::<_, SessionRow>(
            "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir, project_id, auto_approve_level, model_key
             FROM sessions
             WHERE parent_id = ? AND id LIKE 'sub_%'
             ORDER BY updated_at DESC",
        )
        .bind(&*parent_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to list subagents: {e}")))?;

        Ok(rows.into_iter().map(Into::into).collect())
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

    async fn list_expired(
        &self,
        cutoff: DateTime<Utc>,
        keep_pinned: bool,
    ) -> Result<Vec<SessionId>> {
        // NOTE: sessions.updated_at is stored as sqlite text 'YYYY-MM-DD HH:MM:SS'
        // (datetime('now')), while chrono DateTime binds as RFC3339 with a 'T'
        // separator, which breaks lexicographic comparison. Format explicitly.
        let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();

        // Phase 1: regular (non-subagent) expired sessions
        let mut builder = sqlx::QueryBuilder::new("SELECT id FROM sessions WHERE updated_at < ");
        builder.push_bind(&cutoff_str);
        builder.push(" AND id NOT LIKE 'sub_%'");
        if keep_pinned {
            builder.push(" AND id NOT IN (SELECT session_id FROM pinned_sessions)");
        }
        builder.push(" ORDER BY updated_at ASC LIMIT 10000");

        let rows = builder
            .build()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to query expired sessions: {e}")))?;

        let mut ids: Vec<String> = rows
            .into_iter()
            .filter_map(|r| r.try_get::<String, _>("id").ok())
            .collect();

        // Phase 2: child subagent sessions of the expired parents (chunked IN)
        let parents: Vec<String> = ids.clone();
        for chunk in parents.chunks(100) {
            let mut b = sqlx::QueryBuilder::new(
                "SELECT id FROM sessions WHERE id LIKE 'sub_%' AND parent_id IN (",
            );
            let mut sep = b.separated(", ");
            for id in chunk {
                sep.push_bind(id);
            }
            sep.push_unseparated(")");
            let child_rows = b
                .build()
                .fetch_all(&self.pool)
                .await
                .map_err(|e| storage_err(format!("failed to query child sessions: {e}")))?;
            for r in child_rows {
                if let Ok(id) = r.try_get::<String, _>("id") {
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        }

        // Phase 3: orphaned subagent sessions (parent already gone via ON DELETE
        // SET NULL) that are themselves expired.
        let orphan_rows = sqlx::query(
            "SELECT id FROM sessions
             WHERE id LIKE 'sub_%' AND parent_id IS NULL AND updated_at < ?",
        )
        .bind(&cutoff_str)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to query orphan subagent sessions: {e}")))?;
        for r in orphan_rows {
            if let Ok(id) = r.try_get::<String, _>("id") {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }

        Ok(ids.into_iter().map(SessionId::from).collect())
    }

    async fn delete_batch(&self, ids: &[SessionId]) -> Result<u64> {
        const CHUNK_SIZE: usize = 100;

        let mut deleted = 0u64;
        for chunk in ids.chunks(CHUNK_SIZE) {
            let mut builder = sqlx::QueryBuilder::new("DELETE FROM sessions WHERE id IN (");
            let mut separated = builder.separated(", ");
            for id in chunk {
                separated.push_bind(&*id.0);
            }
            separated.push_unseparated(")");

            let result = builder
                .build()
                .execute(&self.pool)
                .await
                .map_err(|e| storage_err(format!("failed to delete sessions: {e}")))?;
            deleted += result.rows_affected();
        }
        Ok(deleted)
    }

    async fn list_ids_by_project(
        &self,
        project_id: &crate::types::ProjectId,
    ) -> Result<Vec<SessionId>> {
        // Sessions of the project, plus subagent children whose parent
        // belongs to the project (children inherit project_id on fork, but be
        // defensive and match via parent_id as well).
        let rows = sqlx::query(
            "SELECT id FROM sessions WHERE project_id = ?
             UNION
             SELECT child.id FROM sessions AS child
             JOIN sessions AS parent ON child.parent_id = parent.id
             WHERE parent.project_id = ?",
        )
        .bind(&*project_id.0)
        .bind(&*project_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to list sessions by project: {e}")))?;

        Ok(rows
            .into_iter()
            .filter_map(|r| r.try_get::<String, _>("id").ok())
            .map(SessionId::from)
            .collect())
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
