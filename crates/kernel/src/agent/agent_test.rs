use super::{last_user_message_is_continue, wait_for_retry};
use crate::agent::{AgentError, CancelToken};
use crate::types::Message;
use std::sync::Arc;
use std::time::Duration;

#[test]
fn auto_continue_is_limited_until_next_user_message() {
    let mut messages = vec![
        Arc::new(Message::system("system")),
        Arc::new(Message::user("original request")),
        Arc::new(Message::assistant("partial response")),
    ];
    assert!(!last_user_message_is_continue(&messages));

    messages.push(Arc::new(Message::user("continue")));
    messages.push(Arc::new(Message::assistant("still partial")));
    assert!(last_user_message_is_continue(&messages));

    messages.push(Arc::new(Message::user("new request")));
    assert!(!last_user_message_is_continue(&messages));
}

#[test]
fn auto_continue_check_uses_trimmed_user_text() {
    let messages = vec![Arc::new(Message::user("  continue\n"))];
    assert!(last_user_message_is_continue(&messages));
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
