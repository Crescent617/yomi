//! `SQLite` implementation of `ProjectStore`

use super::{storage_err, ProjectStore};
use crate::types::{Project, ProjectId, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;

/// SQLite-based project storage
#[derive(Debug, Clone)]
pub struct SqliteProjectStore {
    pool: SqlitePool,
}

impl SqliteProjectStore {
    /// Create new store with `SQLite` pool
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ProjectStore for SqliteProjectStore {
    async fn create(&self, id: &ProjectId, name: &str, dir: &str) -> Result<()> {
        sqlx::query("INSERT INTO projects (id, name, dir) VALUES (?, ?, ?)")
            .bind(&id.0)
            .bind(name)
            .bind(dir)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to create project: {e}")))?;
        Ok(())
    }

    async fn get(&self, id: &ProjectId) -> Result<Option<Project>> {
        let row = sqlx::query_as::<_, ProjectRow>(
            "SELECT id, name, dir, created_at, updated_at FROM projects WHERE id = ?",
        )
        .bind(&id.0)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to get project: {e}")))?;

        Ok(row.map(Into::into))
    }

    async fn get_by_dir(&self, dir: &str) -> Result<Option<Project>> {
        let row = sqlx::query_as::<_, ProjectRow>(
            "SELECT id, name, dir, created_at, updated_at FROM projects WHERE dir = ?",
        )
        .bind(dir)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to get project by dir: {e}")))?;

        Ok(row.map(Into::into))
    }

    async fn list(&self) -> Result<Vec<Project>> {
        let rows = sqlx::query_as::<_, ProjectRow>(
            "SELECT id, name, dir, created_at, updated_at FROM projects ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to list projects: {e}")))?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn update_name(&self, id: &ProjectId, name: &str) -> Result<()> {
        sqlx::query("UPDATE projects SET name = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(name)
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to update project name: {e}")))?;
        Ok(())
    }

    async fn touch(&self, id: &ProjectId) -> Result<()> {
        sqlx::query("UPDATE projects SET updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to touch project: {e}")))?;
        Ok(())
    }

    async fn delete(&self, id: &ProjectId) -> Result<()> {
        sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(&id.0)
            .execute(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to delete project: {e}")))?;
        Ok(())
    }
}

/// Internal row type for SQL mapping
#[derive(sqlx::FromRow)]
struct ProjectRow {
    id: String,
    name: String,
    dir: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<ProjectRow> for Project {
    fn from(row: ProjectRow) -> Self {
        Self {
            id: ProjectId(row.id),
            name: row.name,
            dir: row.dir.into(),
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}
