use super::*;

use crate::provider::TokenUsage;
use crate::storage::migrations::run_migrations;
use crate::storage::usage::UsageType;
use crate::types::SessionId;

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

    let record = UsageRecord::new(
        session_id.clone(),
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
