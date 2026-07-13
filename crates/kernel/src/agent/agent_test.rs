use super::{should_auto_continue, wait_for_retry};
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
    assert!(should_auto_continue(&mut used, None));
}

#[test]
fn auto_continue_ignores_normal_finish_reasons() {
    let mut used = false;
    assert!(!should_auto_continue(&mut used, Some(FinishReason::Stop)));
    assert!(!used);
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
