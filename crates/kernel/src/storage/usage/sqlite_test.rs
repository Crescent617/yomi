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

#[tokio::test]
async fn test_by_model_summary() {
    let store = create_test_store().await;
    let session_id = SessionId::new();

    let records = [
        UsageRecord::new(
            session_id.clone(),
            TokenUsage::new(100, 50, Some(10)),
            "model-a",
            "provider-1",
            UsageType::Normal,
        ),
        UsageRecord::new(
            session_id.clone(),
            TokenUsage::new(200, 100, Some(30)),
            "model-a",
            "provider-1",
            UsageType::Normal,
        ),
        UsageRecord::new(
            session_id.clone(),
            TokenUsage::new(1000, 500, None),
            "model-b",
            "provider-2",
            UsageType::Normal,
        ),
    ];
    for r in &records {
        store.record(r).await.unwrap();
    }

    let rows = store
        .by_model_summary(Utc::now() - chrono::Duration::hours(1), Utc::now(), None)
        .await
        .unwrap();

    assert_eq!(rows.len(), 2);
    // ordered by total tokens desc
    assert_eq!(rows[0].model, "model-b");
    assert_eq!(rows[0].provider, "provider-2");
    assert_eq!(rows[0].prompt_tokens, 1000);
    assert_eq!(rows[0].completion_tokens, 500);
    assert_eq!(rows[0].request_count, 1);

    assert_eq!(rows[1].model, "model-a");
    assert_eq!(rows[1].prompt_tokens, 300);
    assert_eq!(rows[1].completion_tokens, 150);
    assert_eq!(rows[1].cached_tokens, 40);
    assert_eq!(rows[1].request_count, 2);
}

#[tokio::test]
async fn test_by_model_summary_with_filter() {
    let store = create_test_store().await;
    let session_id = SessionId::new();

    for (model, usage_type) in [
        ("model-a", UsageType::Normal),
        ("model-a", UsageType::Compactor),
        ("model-b", UsageType::Normal),
    ] {
        store
            .record(&UsageRecord::new(
                session_id.clone(),
                TokenUsage::new(10, 5, None),
                model,
                "provider-1",
                usage_type,
            ))
            .await
            .unwrap();
    }

    let filter = crate::storage::usage::UsageFilter {
        model: None,
        provider: None,
        usage_type: Some(UsageType::Normal),
    };
    let rows = store
        .by_model_summary(
            Utc::now() - chrono::Duration::hours(1),
            Utc::now(),
            Some(&filter),
        )
        .await
        .unwrap();

    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.request_count == 1));
}
