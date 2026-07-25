use super::{
    is_stream_completion_consistent, should_auto_continue, should_retry_streaming_error,
    wait_for_retry,
};
use crate::agent::{AgentError, CancelToken};
use crate::types::FinishReason;
use std::time::Duration;

#[test]
fn auto_continue_is_claimed_only_once_until_reset() {
    let mut used = false;
    assert!(should_auto_continue(
        &mut used,
        Some(FinishReason::MaxTokens)
    ));
    assert!(!should_auto_continue(&mut used, None));

    used = false;
    assert!(!should_auto_continue(&mut used, None));
    assert!(!used);
}

#[test]
fn auto_continue_ignores_normal_finish_reasons() {
    let mut used = false;
    assert!(!should_auto_continue(&mut used, Some(FinishReason::Stop)));
    assert!(!used);
}

#[test]
fn pause_and_refusal_are_consistent_terminals_without_tool_calls() {
    assert!(is_stream_completion_consistent(
        Some(FinishReason::PauseTurn),
        false
    ));
    assert!(is_stream_completion_consistent(
        Some(FinishReason::Refusal),
        false
    ));
    assert!(!is_stream_completion_consistent(
        Some(FinishReason::Refusal),
        true
    ));
}

#[test]
fn pause_and_refusal_do_not_consume_max_token_auto_continue() {
    let mut used = false;
    assert!(!should_auto_continue(
        &mut used,
        Some(FinishReason::PauseTurn)
    ));
    assert!(!should_auto_continue(
        &mut used,
        Some(FinishReason::Refusal)
    ));
}

#[test]
fn repeat_is_a_consistent_terminal_and_never_auto_continues() {
    assert!(is_stream_completion_consistent(
        Some(FinishReason::Repeat),
        false
    ));
    assert!(!is_stream_completion_consistent(
        Some(FinishReason::Repeat),
        true
    ));
    // Auto-continuing a repetition stop would loop forever.
    let mut used = false;
    assert!(!should_auto_continue(&mut used, Some(FinishReason::Repeat)));
}

#[test]
fn repeat_parses_from_provider_string() {
    assert_eq!(
        FinishReason::from_provider_str("repeat"),
        Some(FinishReason::Repeat)
    );
}

#[test]
fn non_retryable_streaming_error_gets_one_recovery_attempt() {
    assert!(should_retry_streaming_error(0, false));
    assert!(!should_retry_streaming_error(1, false));
}

#[test]
fn retryable_streaming_error_uses_full_retry_budget() {
    assert!(should_retry_streaming_error(0, true));
    assert!(should_retry_streaming_error(10, true));
}

#[tokio::test]
async fn retry_delay_completes_without_cancellation() {
    let token = CancelToken::new();

    let result = wait_for_retry(&token, Duration::from_millis(1)).await;

    assert!(result.is_ok());
}

#[tokio::test]
async fn retry_delay_is_interrupted_by_cancellation() {
    let token = CancelToken::new();
    let cancel = token.clone();

    let result = tokio::time::timeout(Duration::from_millis(100), async move {
        let waiting = wait_for_retry(&token, Duration::from_secs(30));
        tokio::pin!(waiting);

        tokio::task::yield_now().await;
        cancel.cancel();
        waiting.await
    })
    .await
    .expect("retry delay should stop promptly after cancellation");

    assert!(matches!(
        result,
        Err(AgentError::Cancelled(context)) if context == "streaming retry"
    ));
}

#[tokio::test]
async fn retry_delay_observes_existing_cancellation() {
    let token = CancelToken::new();
    token.cancel();

    let result = wait_for_retry(&token, Duration::from_secs(30)).await;

    assert!(matches!(result, Err(AgentError::Cancelled(_))));
}

/// Providers may emit `TokenUsage` multiple times per response (e.g. both
/// choice-level and top-level usage chunks). Only one usage record should be
/// persisted per stream, using the final reported values.
#[tokio::test]
async fn repeated_token_usage_events_are_recorded_once() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::provider::{ModelConfig, ModelStream, ModelStreamItem, TokenUsage};
    use crate::storage::UsageStore;
    use crate::types::{MessageId, SessionId};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let pool = sqlx::sqlite::SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    crate::storage::migrations::run_migrations(&pool)
        .await
        .unwrap();
    let usage_store: Arc<dyn UsageStore> = Arc::new(crate::storage::SqliteUsageStore::new(pool));

    let model_config = ModelConfig {
        name: "test".to_string(),
        model_id: "test-id".to_string(),
        ..ModelConfig::default()
    };
    let mut models = BTreeMap::new();
    models.insert("test".to_string(), model_config.clone());
    let shared = Arc::new(AgentShared::new(
        Arc::new(models),
        "test".to_string(),
        None,
        None,
        None,
        None,
        None,
        Some(usage_store.clone()),
        None,
        Vec::new(),
        None,
        None,
    ));

    let working_dir = tempfile::tempdir().unwrap();
    let args = AgentSpawnArgs {
        base_prompt: "test".to_string(),
        skills: Vec::new(),
        history: Vec::new(),
        session_id: SessionId::new().to_string(),
        parent_session_id: None,
        max_iterations: 1,
        working_dir: working_dir.path().to_path_buf(),
        cancel_token: None,
        tool_flags: crate::tools::ToolFlags::new(false),
        file_state_store: None,
        tool_blocklist: Vec::new(),
        max_tool_output_length: 1024,
        mailbox: Arc::new(crate::comms::Mailbox::new()),
        input_bus: None,
    };
    let mut agent = Agent::new(&shared, args).await;
    agent.current_model_config = Some(Arc::new(model_config));

    let usage = TokenUsage::new(100, 10, None);
    let items = vec![
        Ok(ModelStreamItem::TokenUsage(usage)),
        Ok(ModelStreamItem::TokenUsage(usage)),
        Ok(ModelStreamItem::TokenUsage(usage)),
    ];
    let mut stream: ModelStream = Box::pin(futures::stream::iter(items));

    let result = agent
        .collect_stream_output(&mut stream, MessageId::new())
        .await
        .unwrap();

    assert_eq!(result.token_usage, Some(usage));
    let records = usage_store.list_records(None, 10).await.unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].prompt_tokens, 100);
    assert_eq!(records[0].completion_tokens, 10);
}
