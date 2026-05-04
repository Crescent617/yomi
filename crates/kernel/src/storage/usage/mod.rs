//! Token usage tracking - record and aggregate token consumption

use crate::providers::TokenUsage as ProviderTokenUsage;
use crate::types::{AgentId, KernelError, Result, SessionId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

/// Usage type categorization
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsageType {
    /// Normal agent conversation
    #[default]
    Normal,
    /// Subagent tool execution
    Subagent,
    /// Context compaction
    Compactor,
}

impl UsageType {
    /// String representation for storage
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Subagent => "subagent",
            Self::Compactor => "compactor",
        }
    }
}

impl std::fmt::Display for UsageType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for UsageType {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "normal" => Ok(Self::Normal),
            "subagent" => Ok(Self::Subagent),
            "compactor" => Ok(Self::Compactor),
            _ => Err(format!("unknown usage_type: {s}")),
        }
    }
}

/// Single token usage record
#[derive(Debug, Clone)]
pub struct UsageRecord {
    pub id: String,
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub model: String,
    pub provider: String,
    pub usage_type: UsageType,
    pub created_at: DateTime<Utc>,
}

impl UsageRecord {
    /// Create a new usage record
    pub fn new(
        session_id: SessionId,
        agent_id: AgentId,
        usage: ProviderTokenUsage,
        model: impl Into<String>,
        provider: impl Into<String>,
        usage_type: UsageType,
    ) -> Self {
        Self {
            id: uuid::Uuid::now_v7().to_string(),
            session_id,
            agent_id,
            prompt_tokens: u64::from(usage.prompt_tokens),
            completion_tokens: u64::from(usage.completion_tokens),
            cached_tokens: u64::from(usage.cached_tokens.unwrap_or(0)),
            model: model.into(),
            provider: provider.into(),
            usage_type,
            created_at: Utc::now(),
        }
    }

    /// Total tokens (prompt + completion)
    pub const fn total_tokens(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
}

/// Aggregated usage summary for a time range
#[derive(Debug, Clone, Default)]
pub struct UsageSummary {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub request_count: u64,
}

impl UsageSummary {
    /// Total tokens (prompt + completion)
    pub const fn total_tokens(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
}

/// Daily usage summary
#[derive(Debug, Clone)]
pub struct DailyUsage {
    /// Date in local timezone (YYYY-MM-DD)
    pub date: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub cached_tokens: u64,
    pub request_count: u64,
}

impl DailyUsage {
    /// Total tokens (prompt + completion)
    pub const fn total_tokens(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
}

/// Storage for token usage records and aggregation
#[async_trait]
pub trait UsageStore: Send + Sync {
    /// Record a token usage entry
    async fn record(&self, record: &UsageRecord) -> Result<()>;

    /// Get aggregated summary for a time range
    async fn summarize(&self, start: DateTime<Utc>, end: DateTime<Utc>) -> Result<UsageSummary>;

    /// Get daily aggregated usage for a time range
    async fn daily_summary(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<DailyUsage>>;
}

/// Helper for storage errors
fn storage_err(msg: impl Into<String>) -> KernelError {
    KernelError::Storage(msg.into())
}

pub mod sqlite;
pub use sqlite::SqliteUsageStore;
