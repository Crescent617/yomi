use crate::cron::types::{
    CronAction, CronError, CronJob, CronJobId, CronJobStatus, UpdateCronJobInput,
};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;

#[async_trait]
pub trait CronStore: Send + Sync {
    /// 创建任务
    async fn create(&self, job: &CronJob) -> Result<(), CronError>;
    /// 获取单个任务
    async fn get(&self, id: &CronJobId) -> Result<Option<CronJob>, CronError>;
    /// 列出任务（可按状态过滤）
    async fn list(
        &self,
        status: Option<CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<CronJob>, CronError>;
    /// 更新任务（部分更新）
    async fn update(&self, id: &CronJobId, input: &UpdateCronJobInput) -> Result<bool, CronError>;
    /// 删除任务
    async fn delete(&self, id: &CronJobId) -> Result<bool, CronError>;
    /// 获取所有 active 任务（供 scheduler 加载）
    async fn list_active(&self) -> Result<Vec<CronJob>, CronError>;
    /// `原子更新执行记录（run_count`++, `last_run_at`, `last_error`）
    async fn record_execution(
        &self,
        id: &CronJobId,
        error: Option<String>,
    ) -> Result<(), CronError>;
}

pub struct SqliteCronStore {
    pool: SqlitePool,
}

impl SqliteCronStore {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl CronStore for SqliteCronStore {
    async fn create(&self, job: &CronJob) -> Result<(), CronError> {
        let action_json = serde_json::to_string(&job.action)
            .map_err(|e| CronError::Storage(format!("serialize action: {e}")))?;

        sqlx::query(
            r"INSERT INTO cron_jobs (
                id, name, schedule, action, status, created_at, updated_at,
                next_run_at, last_run_at, run_count, max_runs, expires_at, last_error
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&job.id.0)
        .bind(&job.name)
        .bind(&job.schedule)
        .bind(&action_json)
        .bind(job.status.as_str())
        .bind(job.created_at.to_rfc3339())
        .bind(job.updated_at.to_rfc3339())
        .bind(job.next_run_at.map(|t| t.to_rfc3339()))
        .bind(job.last_run_at.map(|t| t.to_rfc3339()))
        .bind(i64::from(job.run_count))
        .bind(job.max_runs.map(i64::from))
        .bind(job.expires_at.map(|t| t.to_rfc3339()))
        .bind(job.last_error.as_ref())
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get(&self, id: &CronJobId) -> Result<Option<CronJob>, CronError> {
        let row = sqlx::query_as::<_, CronJobRow>("SELECT * FROM cron_jobs WHERE id = ?")
            .bind(&id.0)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| r.into()))
    }

    async fn list(
        &self,
        status: Option<CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<CronJob>, CronError> {
        let rows = if let Some(s) = status {
            sqlx::query_as::<_, CronJobRow>(
                "SELECT * FROM cron_jobs WHERE status = ? ORDER BY created_at DESC LIMIT ?",
            )
            .bind(s.as_str())
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, CronJobRow>(
                "SELECT * FROM cron_jobs ORDER BY created_at DESC LIMIT ?",
            )
            .bind(limit as i64)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    async fn update(&self, id: &CronJobId, input: &UpdateCronJobInput) -> Result<bool, CronError> {
        // 使用参数化查询，避免 SQL 注入
        let result = sqlx::query(
            r"UPDATE cron_jobs SET
                name = COALESCE(?, name),
                schedule = COALESCE(?, schedule),
                action = COALESCE(?, action),
                status = COALESCE(?, status),
                max_runs = COALESCE(?, max_runs),
                expires_at = COALESCE(?, expires_at),
                next_run_at = COALESCE(?, next_run_at),
                updated_at = ?
            WHERE id = ?",
        )
        .bind(&input.name)
        .bind(&input.schedule)
        .bind(
            input
                .action
                .as_ref()
                .and_then(|a| serde_json::to_string(a).ok()),
        )
        .bind(input.status.map(|s| s.as_str().to_string()))
        .bind(input.max_runs.map(i64::from))
        .bind(input.expires_at.map(|t| t.to_rfc3339()))
        .bind(input.next_run_at.map(|t| t.to_rfc3339()))
        .bind(Utc::now().to_rfc3339())
        .bind(&id.0)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn delete(&self, id: &CronJobId) -> Result<bool, CronError> {
        let result = sqlx::query("DELETE FROM cron_jobs WHERE id = ?")
            .bind(&id.0)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
    }

    async fn list_active(&self) -> Result<Vec<CronJob>, CronError> {
        let rows = sqlx::query_as::<_, CronJobRow>(
            "SELECT * FROM cron_jobs WHERE status = 'active' ORDER BY next_run_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(|r| r.into()).collect())
    }

    async fn record_execution(
        &self,
        id: &CronJobId,
        error: Option<String>,
    ) -> Result<(), CronError> {
        sqlx::query(
            r"UPDATE cron_jobs SET
                run_count = run_count + 1,
                last_run_at = ?,
                last_error = ?,
                updated_at = ?
            WHERE id = ?",
        )
        .bind(Utc::now().to_rfc3339())
        .bind(error.as_ref())
        .bind(Utc::now().to_rfc3339())
        .bind(&id.0)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct CronJobRow {
    id: String,
    name: String,
    schedule: String,
    action: String,
    status: String,
    created_at: String,
    updated_at: String,
    next_run_at: Option<String>,
    last_run_at: Option<String>,
    run_count: i64,
    max_runs: Option<i64>,
    expires_at: Option<String>,
    last_error: Option<String>,
}

impl From<CronJobRow> for CronJob {
    fn from(row: CronJobRow) -> Self {
        let action: CronAction = serde_json::from_str(&row.action).unwrap_or_else(|e| {
            tracing::error!("Failed to deserialize cron action: {}", e);
            CronAction::Internal {
                endpoint: "error".to_string(),
                payload: serde_json::json!({"error": e.to_string()}),
            }
        });

        let status = row.status.parse().unwrap_or(CronJobStatus::Failed);

        Self {
            id: CronJobId(row.id),
            name: row.name,
            schedule: row.schedule,
            action,
            status,
            created_at: parse_datetime(&row.created_at).unwrap_or_else(Utc::now),
            updated_at: parse_datetime(&row.updated_at).unwrap_or_else(Utc::now),
            next_run_at: row.next_run_at.and_then(|s| parse_datetime(&s)),
            last_run_at: row.last_run_at.and_then(|s| parse_datetime(&s)),
            run_count: row.run_count as u32,
            max_runs: row.max_runs.map(|v| v as u32),
            expires_at: row.expires_at.and_then(|s| parse_datetime(&s)),
            last_error: row.last_error,
        }
    }
}

fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}
