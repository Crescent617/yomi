//! 安装记录（sqlite，yomi.db）：审计与 list 展示。
//!
//! **正确性不依赖此表**（state is cache）：remove 回滚由 symlink 指向
//! 判定 + cron 前缀清扫完成；本表回答"装了什么、从哪来、什么时候"。

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::sqlite::SqlitePool;

use super::PkgError;

/// 一次安装收编的资源清单（JSON 落库，结构化读写）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Resources {
    /// cron 全名列表（`ext:<名>:<条目>`）。
    pub cron: Vec<String>,
    /// hook 挂载相对路径（`pre_tool_use/50-guard`，不含 hooks/ 前缀）。
    pub hooks: Vec<String>,
    /// bin 挂载文件名（不含 bin/ 前缀）。
    pub bins: Vec<String>,
    /// snippet 文件名。
    pub snippets: Vec<String>,
}

/// 一条安装记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ExtInstall {
    pub name: String,
    /// 安装时的源路径（绝对）。
    pub source: String,
    /// `symlink` | `copy`。
    pub mode: String,
    pub version: String,
    pub resources: Resources,
    pub installed_at: DateTime<Utc>,
}

#[async_trait]
pub trait ExtInstallStore: Send + Sync {
    async fn upsert(&self, record: &ExtInstall) -> Result<(), PkgError>;
    async fn get(&self, name: &str) -> Result<Option<ExtInstall>, PkgError>;
    async fn list(&self) -> Result<Vec<ExtInstall>, PkgError>;
    async fn delete(&self, name: &str) -> Result<bool, PkgError>;
}

pub struct SqliteExtInstallStore {
    pool: SqlitePool,
}

impl SqliteExtInstallStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl ExtInstallStore for SqliteExtInstallStore {
    async fn upsert(&self, record: &ExtInstall) -> Result<(), PkgError> {
        let resources = serde_json::to_string(&record.resources)
            .map_err(|e| PkgError::Storage(format!("serialize resources: {e}")))?;
        sqlx::query(
            r"INSERT INTO ext_installs (name, source, mode, version, resources, installed_at)
              VALUES (?, ?, ?, ?, ?, ?)
              ON CONFLICT(name) DO UPDATE SET
                source = excluded.source,
                mode = excluded.mode,
                version = excluded.version,
                resources = excluded.resources,
                installed_at = excluded.installed_at",
        )
        .bind(&record.name)
        .bind(&record.source)
        .bind(&record.mode)
        .bind(&record.version)
        .bind(&resources)
        .bind(record.installed_at.to_rfc3339())
        .execute(&self.pool)
        .await
        .map_err(|e| PkgError::Storage(format!("upsert ext_installs: {e}")))?;
        Ok(())
    }

    async fn get(&self, name: &str) -> Result<Option<ExtInstall>, PkgError> {
        let row = sqlx::query_as::<_, ExtInstallRow>("SELECT * FROM ext_installs WHERE name = ?")
            .bind(name)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| PkgError::Storage(format!("get ext_installs: {e}")))?;
        Ok(row.map(Into::into))
    }

    async fn list(&self) -> Result<Vec<ExtInstall>, PkgError> {
        let rows = sqlx::query_as::<_, ExtInstallRow>(
            "SELECT * FROM ext_installs ORDER BY installed_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| PkgError::Storage(format!("list ext_installs: {e}")))?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn delete(&self, name: &str) -> Result<bool, PkgError> {
        let result = sqlx::query("DELETE FROM ext_installs WHERE name = ?")
            .bind(name)
            .execute(&self.pool)
            .await
            .map_err(|e| PkgError::Storage(format!("delete ext_installs: {e}")))?;
        Ok(result.rows_affected() > 0)
    }
}

#[derive(sqlx::FromRow)]
struct ExtInstallRow {
    name: String,
    source: String,
    mode: String,
    version: String,
    resources: String,
    installed_at: String,
}

impl From<ExtInstallRow> for ExtInstall {
    fn from(row: ExtInstallRow) -> Self {
        let resources = serde_json::from_str(&row.resources).unwrap_or_default();
        Self {
            name: row.name,
            source: row.source,
            mode: row.mode,
            version: row.version,
            resources,
            installed_at: DateTime::parse_from_rfc3339(&row.installed_at)
                .map_or_else(|_| Utc::now(), |dt| dt.with_timezone(&Utc)),
        }
    }
}

#[cfg(test)]
#[path = "store_test.rs"]
mod tests;
