//! `SQLite` implementation of `UsageStore`

use super::{storage_err, DailyUsage, UsageRecord, UsageStore, UsageSummary};
use crate::types::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::sqlite::SqlitePool;

/// SQLite-based usage storage
#[derive(Debug, Clone)]
pub struct SqliteUsageStore {
    pool: SqlitePool,
}

impl SqliteUsageStore {
    /// Create new store with `SQLite` pool
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
}

#[async_trait]
impl UsageStore for SqliteUsageStore {
    async fn record(&self, record: &UsageRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO token_usage 
             (id, session_id, prompt_tokens, completion_tokens, total_tokens, cached_tokens, model, provider, usage_type, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&record.id)
        .bind(&*record.session_id.0)
        .bind(record.prompt_tokens as i64)
        .bind(record.completion_tokens as i64)
        .bind(record.total_tokens() as i64)
        .bind(record.cached_tokens as i64)
        .bind(&record.model)
        .bind(&record.provider)
        .bind(record.usage_type.as_str())
        .bind(record.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| storage_err(format!("failed to record usage: {e}")))?;

        Ok(())
    }

    async fn summarize(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        filter: Option<&super::UsageFilter>,
    ) -> Result<UsageSummary> {
        let mut builder = sqlx::QueryBuilder::new(
            "SELECT 
                COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                COALESCE(SUM(cached_tokens), 0) as cached_tokens,
                COUNT(*) as request_count
             FROM token_usage 
             WHERE 1=1",
        );

        push_summary_filters(&mut builder, start, end, filter);

        let row = builder
            .build_query_as::<SummaryRow>()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to summarize usage: {e}")))?;

        Ok(UsageSummary {
            prompt_tokens: row.prompt_tokens as u64,
            completion_tokens: row.completion_tokens as u64,
            cached_tokens: row.cached_tokens as u64,
            request_count: row.request_count as u64,
        })
    }

    async fn daily_summary(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        filter: Option<&super::UsageFilter>,
    ) -> Result<Vec<DailyUsage>> {
        let mut builder = sqlx::QueryBuilder::new(
            "SELECT 
                date(created_at, 'localtime') as date,
                COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                COALESCE(SUM(cached_tokens), 0) as cached_tokens,
                COUNT(*) as request_count,
                COALESCE(GROUP_CONCAT(DISTINCT model), '') as models
             FROM token_usage 
             WHERE 1=1",
        );

        push_summary_filters(&mut builder, start, end, filter);

        builder.push(
            " GROUP BY date(created_at, 'localtime') ORDER BY date(created_at, 'localtime') ASC",
        );

        let rows = builder
            .build_query_as::<DailyRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to get daily summary: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|r| DailyUsage {
                date: r.date,
                prompt_tokens: r.prompt_tokens as u64,
                completion_tokens: r.completion_tokens as u64,
                cached_tokens: r.cached_tokens as u64,
                request_count: r.request_count as u64,
                models: if r.models.is_empty() {
                    Vec::new()
                } else {
                    r.models.split(',').map(|s| s.to_string()).collect()
                },
            })
            .collect())
    }

    async fn by_model_summary(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
        filter: Option<&super::UsageFilter>,
    ) -> Result<Vec<super::ModelUsage>> {
        let mut builder = sqlx::QueryBuilder::new(
            "SELECT
                model,
                provider,
                COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                COALESCE(SUM(cached_tokens), 0) as cached_tokens,
                COUNT(*) as request_count
             FROM token_usage
             WHERE 1=1",
        );

        push_summary_filters(&mut builder, start, end, filter);

        builder
            .push(" GROUP BY model, provider ORDER BY SUM(prompt_tokens + completion_tokens) DESC");

        let rows = builder
            .build_query_as::<ModelRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to get model summary: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|r| super::ModelUsage {
                model: r.model,
                provider: r.provider,
                prompt_tokens: r.prompt_tokens as u64,
                completion_tokens: r.completion_tokens as u64,
                cached_tokens: r.cached_tokens as u64,
                request_count: r.request_count as u64,
            })
            .collect())
    }

    async fn list_records(
        &self,
        before_id: Option<&str>,
        limit: usize,
    ) -> Result<Vec<UsageRecord>> {
        let mut builder = sqlx::QueryBuilder::new(
            "SELECT
                id,
                session_id,
                prompt_tokens,
                completion_tokens,
                cached_tokens,
                model,
                provider,
                usage_type,
                created_at
             FROM token_usage
             WHERE 1=1",
        );

        if let Some(id) = before_id {
            builder.push(" AND (created_at < (SELECT created_at FROM token_usage WHERE id = ");
            builder.push_bind(id);
            builder.push(") OR (created_at = (SELECT created_at FROM token_usage WHERE id = ");
            builder.push_bind(id);
            builder.push(") AND id < ");
            builder.push_bind(id);
            builder.push("))");
        }

        builder.push(" ORDER BY created_at DESC, id DESC LIMIT ");
        builder.push_bind(limit as i64);

        let rows = builder
            .build_query_as::<RecordRow>()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| storage_err(format!("failed to list usage records: {e}")))?;

        Ok(rows
            .into_iter()
            .map(|r| UsageRecord {
                id: r.id,
                session_id: crate::types::SessionId::from(r.session_id),
                prompt_tokens: r.prompt_tokens as u64,
                completion_tokens: r.completion_tokens as u64,
                cached_tokens: r.cached_tokens as u64,
                model: r.model,
                provider: r.provider,
                usage_type: r.usage_type.parse().unwrap_or_default(),
                created_at: r.created_at,
            })
            .collect())
    }
}

/// Push the shared WHERE clauses (time range + optional filter) onto a
/// summary-query builder that already ends with `WHERE 1=1`.
fn push_summary_filters(
    builder: &mut sqlx::QueryBuilder<sqlx::Sqlite>,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    filter: Option<&super::UsageFilter>,
) {
    builder.push(" AND created_at >= ");
    builder.push_bind(start);
    builder.push(" AND created_at <= ");
    builder.push_bind(end);

    if let Some(f) = filter {
        if let Some(model) = &f.model {
            builder.push(" AND model = ");
            builder.push_bind(model);
        }
        if let Some(provider) = &f.provider {
            builder.push(" AND provider = ");
            builder.push_bind(provider);
        }
        if let Some(usage_type) = f.usage_type {
            builder.push(" AND usage_type = ");
            builder.push_bind(usage_type.as_str());
        }
    }
}

/// Internal row type for summary queries
#[derive(sqlx::FromRow)]
struct SummaryRow {
    prompt_tokens: i64,
    completion_tokens: i64,
    cached_tokens: i64,
    request_count: i64,
}

/// Internal row type for per-model summary queries
#[derive(sqlx::FromRow)]
struct ModelRow {
    model: String,
    provider: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    cached_tokens: i64,
    request_count: i64,
}

/// Internal row type for daily summary queries
#[derive(sqlx::FromRow)]
struct DailyRow {
    date: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    cached_tokens: i64,
    request_count: i64,
    models: String,
}

/// Internal row type for raw record queries
#[derive(sqlx::FromRow)]
struct RecordRow {
    id: String,
    session_id: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    cached_tokens: i64,
    model: String,
    provider: String,
    usage_type: String,
    created_at: DateTime<Utc>,
}

#[cfg(test)]
#[path = "sqlite_test.rs"]
mod tests;
