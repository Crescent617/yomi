//! Token usage storage

use crate::types::{AgentId, KernelError, Result, SessionId, TokenRecord};
use sqlx::sqlite::SqlitePool;

/// Storage for token usage records
#[derive(Debug, Clone)]
pub struct TokenStorage {
    pool: SqlitePool,
}

impl TokenStorage {
    /// Create new `TokenStorage` with `SQLite` pool
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// Record a token usage entry
    pub async fn record(&self, record: &TokenRecord) -> Result<()> {
        sqlx::query(
            r"INSERT INTO token_usage 
               (id, session_id, agent_id, prompt_tokens, completion_tokens, total_tokens, cached_tokens, model, provider, usage_type, created_at)
               VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
        )
        .bind(&record.id)
        .bind(&record.session_id.0)
        .bind(record.agent_id.as_str())
        .bind(i64::from(record.usage.prompt_tokens))
        .bind(i64::from(record.usage.completion_tokens))
        .bind(i64::from(record.total_tokens()))
        .bind(record.usage.cached_tokens.map(i64::from))
        .bind(&record.model)
        .bind(&record.provider)
        .bind(record.usage_type.as_str())
        .bind(record.created_at)
        .execute(&self.pool)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to record token usage: {e}")))?;

        Ok(())
    }

    /// Get token usage summary for a time range
    pub async fn get_summary_by_time_range(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> Result<crate::storage::TokenUsageSummary> {
        let row = sqlx::query_as::<_, TokenSummaryRow>(
            r"SELECT 
                COALESCE(SUM(prompt_tokens), 0) as total_prompt,
                COALESCE(SUM(completion_tokens), 0) as total_completion,
                COALESCE(SUM(cached_tokens), 0) as total_cached,
                COUNT(*) as request_count
             FROM token_usage 
             WHERE created_at >= ? AND created_at <= ?",
        )
        .bind(start)
        .bind(end)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to get token usage summary: {e}")))?;

        Ok(crate::storage::TokenUsageSummary {
            total_prompt: row.total_prompt as u64,
            total_completion: row.total_completion as u64,
            total_cached: row.total_cached as u64,
            request_count: row.request_count as u64,
        })
    }
}

/// Internal row type for time range summary queries
#[derive(sqlx::FromRow)]
struct TokenSummaryRow {
    total_prompt: i64,
    total_completion: i64,
    total_cached: i64,
    request_count: i64,
}

/// Internal row type for full record queries
#[derive(sqlx::FromRow)]
struct TokenRowFull {
    id: String,
    session_id: String,
    agent_id: String,
    prompt_tokens: i64,
    completion_tokens: i64,
    cached_tokens: Option<i64>,
    model: String,
    provider: String,
    usage_type: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

impl TryFrom<TokenRowFull> for TokenRecord {
    type Error = String;

    fn try_from(row: TokenRowFull) -> std::result::Result<Self, Self::Error> {
        let usage = crate::providers::TokenUsage {
            prompt_tokens: row.prompt_tokens as u32,
            completion_tokens: row.completion_tokens as u32,
            cached_tokens: row.cached_tokens.map(|v| v as u32),
        };
        Ok(TokenRecord {
            id: row.id,
            session_id: SessionId(row.session_id),
            agent_id: AgentId::from_string(row.agent_id),
            usage,
            model: row.model,
            provider: row.provider,
            usage_type: row.usage_type.parse()?,
            created_at: row.created_at,
        })
    }
}
