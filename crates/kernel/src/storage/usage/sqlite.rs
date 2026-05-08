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

    /// Append filter conditions to SQL string and return bound values.
    fn push_filter(
        sql: &mut String,
        filter: Option<&super::UsageFilter>,
    ) -> (Option<String>, Option<String>, Option<String>) {
        let (model, provider, usage_type) = match filter {
            Some(f) => (
                f.model.clone(),
                f.provider.clone(),
                f.usage_type.map(|t| t.as_str().to_string()),
            ),
            None => (None, None, None),
        };
        if model.is_some() {
            sql.push_str(" AND model = ?");
        }
        if provider.is_some() {
            sql.push_str(" AND provider = ?");
        }
        if usage_type.is_some() {
            sql.push_str(" AND usage_type = ?");
        }
        (model, provider, usage_type)
    }
}

#[async_trait]
impl UsageStore for SqliteUsageStore {
    async fn record(&self, record: &UsageRecord) -> Result<()> {
        sqlx::query(
            "INSERT INTO token_usage 
             (id, session_id, agent_id, prompt_tokens, completion_tokens, total_tokens, cached_tokens, model, provider, usage_type, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&record.id)
        .bind(&record.session_id.0)
        .bind(record.agent_id.as_str())
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
        let mut sql = String::from(
            "SELECT 
                COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                COALESCE(SUM(cached_tokens), 0) as cached_tokens,
                COUNT(*) as request_count
             FROM token_usage 
             WHERE created_at >= ? AND created_at <= ?",
        );

        let (model, provider, usage_type) = Self::push_filter(&mut sql, filter);

        let mut query = sqlx::query_as::<_, SummaryRow>(&sql).bind(start).bind(end);
        if let Some(v) = model {
            query = query.bind(v);
        }
        if let Some(v) = provider {
            query = query.bind(v);
        }
        if let Some(v) = usage_type {
            query = query.bind(v);
        }

        let row = query
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
        let mut sql = String::from(
            "SELECT 
                date(created_at, 'localtime') as date,
                COALESCE(SUM(prompt_tokens), 0) as prompt_tokens,
                COALESCE(SUM(completion_tokens), 0) as completion_tokens,
                COALESCE(SUM(cached_tokens), 0) as cached_tokens,
                COUNT(*) as request_count,
                COALESCE(GROUP_CONCAT(DISTINCT model), '') as models
             FROM token_usage 
             WHERE created_at >= ? AND created_at <= ?",
        );

        let (model, provider, usage_type) = Self::push_filter(&mut sql, filter);
        sql.push_str(
            " GROUP BY date(created_at, 'localtime') ORDER BY date(created_at, 'localtime') ASC",
        );

        let mut query = sqlx::query_as::<_, DailyRow>(&sql).bind(start).bind(end);
        if let Some(v) = model {
            query = query.bind(v);
        }
        if let Some(v) = provider {
            query = query.bind(v);
        }
        if let Some(v) = usage_type {
            query = query.bind(v);
        }

        let rows = query
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
}

/// Internal row type for summary queries
#[derive(sqlx::FromRow)]
struct SummaryRow {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::TokenUsage;
    use crate::storage::migrations::run_migrations;
    use crate::storage::usage::UsageType;
    use crate::types::{AgentId, SessionId};

    async fn create_test_store() -> SqliteUsageStore {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        SqliteUsageStore::new(pool)
    }

    #[tokio::test]
    async fn test_record_and_summarize() {
        let store = create_test_store().await;
        let session_id = SessionId::new();
        let agent_id = AgentId::new();

        let record = UsageRecord::new(
            session_id.clone(),
            agent_id.clone(),
            TokenUsage::new(100, 50, Some(10)),
            "claude-3-5-sonnet",
            "anthropic",
            UsageType::Normal,
        );
        store.record(&record).await.unwrap();

        let summary = store
            .summarize(Utc::now() - chrono::Duration::hours(1), Utc::now(), None)
            .await
            .unwrap();

        assert_eq!(summary.prompt_tokens, 100);
        assert_eq!(summary.completion_tokens, 50);
        assert_eq!(summary.cached_tokens, 10);
        assert_eq!(summary.request_count, 1);
    }
}
