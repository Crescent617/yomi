use crate::cron::types::{
    CronAction, CronError, CronJob, CronJobId, CronJobStatus, UpdateCronJobInput, NEVER_EXPIRES,
    UNLIMITED_MAX_RUNS,
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
    /// 按名字获取任务（name 全库唯一）
    async fn get_by_name(&self, name: &str) -> Result<Option<CronJob>, CronError>;
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
    /// 原子更新执行记录（`run_count`++, `last_run_at`, `last_error`）
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
                next_run_at, last_run_at, run_count, max_runs, expires_at, last_error,
                precheck
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&*job.id.0)
        .bind(&job.name)
        .bind(&job.schedule)
        .bind(&action_json)
        .bind(job.status.as_str())
        .bind(job.created_at.to_rfc3339())
        .bind(job.updated_at.to_rfc3339())
        .bind(job.next_run_at.map(|t| t.to_rfc3339()))
        .bind(job.last_run_at.map(|t| t.to_rfc3339()))
        .bind(i64::from(job.run_count))
        .bind(i64::from(job.max_runs))
        .bind(job.expires_at.to_rfc3339())
        .bind(job.last_error.as_ref())
        .bind(job.precheck.as_ref())
        .execute(&self.pool)
        .await
        .map_err(|e| map_name_conflict(e, &job.name))?;

        Ok(())
    }

    async fn get(&self, id: &CronJobId) -> Result<Option<CronJob>, CronError> {
        let row = sqlx::query_as::<_, CronJobRow>("SELECT * FROM cron_jobs WHERE id = ?")
            .bind(&*id.0)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| r.into()))
    }

    async fn get_by_name(&self, name: &str) -> Result<Option<CronJob>, CronError> {
        let row = sqlx::query_as::<_, CronJobRow>("SELECT * FROM cron_jobs WHERE name = ?")
            .bind(name)
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
        // 使用参数化查询，避免 SQL 注入。
        // max_runs = 0 / expires_at = 零时间戳即“不限制/永不过期”，直接落库。
        // precheck 三态：None = 不变；空白串 = 清除（落 NULL，归一在 Rust 侧做，
        // 不用 SQL TRIM——它只剥 U+0020，与 Rust trim() 的 Unicode 语义不一致）；
        // 其他 = 原样设置。
        let precheck = input.precheck.as_ref().filter(|s| !s.trim().is_empty());
        let result = sqlx::query(
            r"UPDATE cron_jobs SET
                name = COALESCE(?, name),
                schedule = COALESCE(?, schedule),
                action = COALESCE(?, action),
                status = COALESCE(?, status),
                max_runs = COALESCE(?, max_runs),
                expires_at = COALESCE(?, expires_at),
                next_run_at = COALESCE(?, next_run_at),
                precheck = CASE WHEN ? THEN ? ELSE precheck END,
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
        .bind(input.precheck.is_some())
        .bind(precheck)
        .bind(Utc::now().to_rfc3339())
        .bind(&*id.0)
        .execute(&self.pool)
        .await
        .map_err(|e| match &input.name {
            Some(name) => map_name_conflict(e, name),
            None => CronError::from(e),
        })?;

        Ok(result.rows_affected() > 0)
    }

    async fn delete(&self, id: &CronJobId) -> Result<bool, CronError> {
        let result = sqlx::query("DELETE FROM cron_jobs WHERE id = ?")
            .bind(&*id.0)
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
        .bind(&*id.0)
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
    precheck: Option<String>,
}

impl From<CronJobRow> for CronJob {
    fn from(row: CronJobRow) -> Self {
        let mut status = row.status.parse().unwrap_or(CronJobStatus::Failed);
        let mut last_error = row.last_error;

        let action: CronAction = match serde_json::from_str(&row.action) {
            Ok(action) => action,
            Err(e) => {
                tracing::warn!(
                    cron_job_id = %row.id,
                    "Failed to deserialize cron action: {}. Marking job as failed.",
                    e
                );
                status = CronJobStatus::Failed;
                last_error = Some(format!("Malformed action: {e}"));
                CronAction::Internal {
                    endpoint: "error".to_string(),
                    payload: serde_json::json!({"error": e.to_string()}),
                }
            }
        };

        Self {
            id: CronJobId::from(row.id),
            name: row.name,
            schedule: row.schedule,
            action,
            status,
            created_at: parse_datetime(&row.created_at).unwrap_or_else(Utc::now),
            updated_at: parse_datetime(&row.updated_at).unwrap_or_else(Utc::now),
            next_run_at: row.next_run_at.and_then(|s| parse_datetime(&s)),
            last_run_at: row.last_run_at.and_then(|s| parse_datetime(&s)),
            run_count: row.run_count as u32,
            // Legacy NULLs read back as the "no limit" / "never" sentinels.
            max_runs: row
                .max_runs
                .and_then(|v| u32::try_from(v).ok())
                .unwrap_or(UNLIMITED_MAX_RUNS),
            expires_at: row
                .expires_at
                .and_then(|s| parse_datetime(&s))
                .unwrap_or(NEVER_EXPIRES),
            last_error,
            precheck: row.precheck,
        }
    }
}

/// 把 name 唯一索引冲突翻译成领域错误；其他错误原样转换。
fn map_name_conflict(e: sqlx::Error, name: &str) -> CronError {
    match &e {
        sqlx::Error::Database(db_err) if db_err.is_unique_violation() => {
            CronError::DuplicateName(name.to_string())
        }
        _ => CronError::from(e),
    }
}

fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
#[path = "store_test.rs"]
mod tests;
