//! Token usage storage

use crate::types::{
    AgentId, KernelError, Result, SessionId, SessionTokenSummary, TokenRecord, UsageType,
};
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

    /// Get token usage summary for a session
    pub async fn get_session_summary(&self, session_id: &SessionId) -> Result<SessionTokenSummary> {
        let rows = sqlx::query_as::<_, TokenRow>(
            r"SELECT 
                usage_type,
                SUM(prompt_tokens) as prompt_tokens,
                SUM(completion_tokens) as completion_tokens,
                COUNT(*) as request_count
             FROM token_usage 
             WHERE session_id = ?
             GROUP BY usage_type",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to get session token summary: {e}")))?;

        let mut summary = SessionTokenSummary {
            session_id: session_id.clone(),
            ..Default::default()
        };

        for row in rows {
            summary.request_count += row.request_count as u32;
            summary.total_prompt += row.prompt_tokens as u32;
            summary.total_completion += row.completion_tokens as u32;

            if let Ok(usage_type) = row.usage_type.parse::<UsageType>() {
                match usage_type {
                    UsageType::Normal => {
                        summary.normal_prompt = row.prompt_tokens as u32;
                        summary.normal_completion = row.completion_tokens as u32;
                    }
                    UsageType::Subagent => {
                        summary.subagent_prompt = row.prompt_tokens as u32;
                        summary.subagent_completion = row.completion_tokens as u32;
                    }
                    UsageType::Compactor => {
                        summary.compactor_prompt = row.prompt_tokens as u32;
                        summary.compactor_completion = row.completion_tokens as u32;
                    }
                }
            }
        }

        Ok(summary)
    }

    /// List all token records for a session
    pub async fn list_by_session(&self, session_id: &SessionId) -> Result<Vec<TokenRecord>> {
        let rows = sqlx::query_as::<_, TokenRowFull>(
            r"SELECT 
                id, session_id, agent_id, prompt_tokens, completion_tokens,
                cached_tokens, model, provider, usage_type, created_at
             FROM token_usage 
             WHERE session_id = ?
             ORDER BY created_at DESC",
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to list token usage: {e}")))?;

        let mut records = Vec::with_capacity(rows.len());
        for row in rows {
            match row.try_into() {
                Ok(record) => records.push(record),
                Err(e) => {
                    tracing::warn!("Failed to convert token usage row: {}", e);
                }
            }
        }
        Ok(records)
    }
}

/// Internal row type for summary queries
#[derive(sqlx::FromRow)]
struct TokenRow {
    usage_type: String,
    prompt_tokens: i64,
    completion_tokens: i64,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::migrations::run_migrations;

    async fn create_test_storage() -> TokenStorage {
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&pool).await.unwrap();
        TokenStorage::new(pool)
    }

    #[tokio::test]
    async fn test_record_and_summary() {
        let storage = create_test_storage().await;
        let session_id = SessionId::new();
        let agent_id = AgentId::new();

        // Record normal usage
        let record1 = TokenRecord::new(
            session_id.clone(),
            agent_id.clone(),
            crate::providers::TokenUsage::new(100, 50, None),
            "claude-3-7-sonnet",
            "anthropic",
            UsageType::Normal,
        );
        storage.record(&record1).await.unwrap();

        // Record subagent usage
        let record2 = TokenRecord::new(
            session_id.clone(),
            agent_id.clone(),
            crate::providers::TokenUsage::new(200, 100, None),
            "claude-3-5-haiku",
            "anthropic",
            UsageType::Subagent,
        );
        storage.record(&record2).await.unwrap();

        // Get summary
        let summary = storage.get_session_summary(&session_id).await.unwrap();

        assert_eq!(summary.total_prompt, 300);
        assert_eq!(summary.total_completion, 150);
        assert_eq!(summary.request_count, 2);
        assert_eq!(summary.normal_prompt, 100);
        assert_eq!(summary.subagent_completion, 100);
    }

    #[tokio::test]
    async fn test_list_by_session() {
        let storage = create_test_storage().await;
        let session_id = SessionId::new();
        let agent_id = AgentId::new();

        let record = TokenRecord::new(
            session_id.clone(),
            agent_id,
            crate::providers::TokenUsage::new(50, 25, None),
            "gpt-4",
            "openai",
            UsageType::Compactor,
        );
        storage.record(&record).await.unwrap();

        let records = storage.list_by_session(&session_id).await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].usage_type, UsageType::Compactor);
        assert_eq!(records[0].model, "gpt-4");
    }
}
