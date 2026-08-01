use super::{
    is_stream_completion_consistent, retry_delay, should_auto_continue,
    should_retry_streaming_error, wait_for_retry, RETRY_AFTER_CAP, RETRY_BASE_DELAY,
    RETRY_MAX_DELAY,
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

#[test]
fn retry_delay_honors_retry_after_hint_with_floor_and_cap() {
    // A server hint wins over the exponential backoff (hint + up to 25% jitter).
    let delay = retry_delay(3, Some(Duration::from_secs(7)));
    let base = Duration::from_secs(7);
    assert!(
        delay >= base && delay <= base + base / 4,
        "hint + jitter: {delay:?}"
    );

    // `Retry-After: 0` must not spin a hot retry loop — floored at 1s.
    let delay = retry_delay(1, Some(Duration::ZERO));
    let floor = Duration::from_secs(1);
    assert!(
        delay >= floor && delay <= floor + floor / 4,
        "floor + jitter: {delay:?}"
    );

    // … capped generously — window-based rate limits ask for long waits
    // (jitter is clamped back into the cap).
    assert_eq!(
        retry_delay(1, Some(Duration::from_hours(2))),
        RETRY_AFTER_CAP
    );
}

#[test]
fn retry_delay_backs_off_exponentially_with_jitter_and_cap() {
    let mut prev_base = Duration::ZERO;
    for attempt in 1u32..=25 {
        let shift = attempt.saturating_sub(1).min(20);
        let base = RETRY_BASE_DELAY
            .saturating_mul(1u32 << shift)
            .min(RETRY_MAX_DELAY);
        let delay = retry_delay(attempt, None);
        assert!(
            delay >= base,
            "attempt {attempt}: {delay:?} below base {base:?}"
        );
        assert!(
            delay <= base + base / 4,
            "attempt {attempt}: {delay:?} above base {base:?} + 25% jitter"
        );
        assert!(base >= prev_base, "backoff never shrinks");
        prev_base = base;
    }
    // Long attempts saturate at the cap.
    assert_eq!(
        RETRY_BASE_DELAY
            .saturating_mul(1u32 << 20)
            .min(RETRY_MAX_DELAY),
        RETRY_MAX_DELAY
    );
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

/// `/clear` must not drop the system prompt: the conversation is wiped but
/// the agent keeps the prompt assembled at spawn.
#[tokio::test]
async fn handle_clear_keeps_system_prompt() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::types::{ContentBlock, Message, Role, SessionId};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let shared = Arc::new(AgentShared::new(
        Arc::new(BTreeMap::new()),
        "test".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
    ));

    let working_dir = tempfile::tempdir().unwrap();
    let args = AgentSpawnArgs {
        base_prompt: "BASE_PROMPT_MARKER".to_string(),
        skills: Vec::new(),
        history: vec![
            Arc::new(Message::user("hello")),
            Arc::new(Message::user("again")),
        ],
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

    // Sanity: assembled system prompt + two history messages.
    assert_eq!(agent.message_buffer.len(), 3);

    agent.handle_clear().await;

    let messages = agent.message_buffer.messages();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].role, Role::System);
    let prompt_text = messages[0]
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .expect("system prompt has text");
    assert!(prompt_text.contains("BASE_PROMPT_MARKER"));
}

/// Compaction results intentionally exclude system messages (see
/// `Compactor::full_compact`); `apply_compacted_messages` must re-prepend the
/// buffer's system prompt so `/compact` and auto-compaction never lose it.
#[tokio::test]
async fn apply_compacted_messages_keeps_system_prompt() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::types::{ContentBlock, Message, Role, SessionId};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let shared = Arc::new(AgentShared::new(
        Arc::new(BTreeMap::new()),
        "test".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
    ));

    let working_dir = tempfile::tempdir().unwrap();
    let args = AgentSpawnArgs {
        base_prompt: "BASE_PROMPT_MARKER".to_string(),
        skills: Vec::new(),
        history: vec![
            Arc::new(Message::user("hello")),
            Arc::new(Message::user("again")),
        ],
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

    // Full-compaction-shaped result: summary + recent, no system message.
    let compacted = vec![
        Arc::new(Message::user("[summary] talked about greetings")),
        Arc::new(Message::user("again")),
    ];
    let rewritten = agent.apply_compacted_messages(compacted).await;

    assert!(rewritten);
    let messages = agent.message_buffer.messages();
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].role, Role::System);
    let prompt_text = messages[0]
        .content
        .iter()
        .find_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .expect("system prompt has text");
    assert!(prompt_text.contains("BASE_PROMPT_MARKER"));
    assert_eq!(messages[1].role, Role::User);
}

/// A cancelled agent exits its loop instead of resetting and continuing:
/// the next input respawns it with freshly assembled context (this is what
/// makes `/cancel` double as a session reload).
#[tokio::test]
async fn cancelled_agent_exits_loop() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::types::SessionId;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let shared = Arc::new(AgentShared::new(
        Arc::new(BTreeMap::new()),
        "test".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
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
    let agent = Agent::new(&shared, args).await;
    agent.cancel_token.cancel();

    let result = tokio::time::timeout(Duration::from_secs(2), agent.start_loop())
        .await
        .expect("cancelled agent should exit promptly");
    assert!(result.is_ok());
}

/// 429 + `Retry-After` end to end: the stub model endpoint always returns
/// 429 with `Retry-After: 1`; the hint must flow HTTP header → provider
/// error → retry loop → the emitted `Retrying` event's `wait_ms` (floored
/// at 1s, plus up to 25% jitter).
#[tokio::test]
async fn retrying_event_carries_retry_after_wait() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::provider::ModelConfig;
    use crate::types::SessionId;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // One retry only: the first failure emits the Retrying event under
    // test, the second fails the run.
    std::env::set_var("YOMI_STREAM_MAX_RETRIES", "1");

    // Stub model endpoint: read the full request (so the client never sees
    // a premature close), then always 429 with `Retry-After: 1`.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                let header_end = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
                    {
                        break pos;
                    }
                };
                let headers = String::from_utf8_lossy(&buf[..header_end]).to_string();
                let content_length: usize = headers
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse().ok())
                    })
                    .unwrap_or(0);
                while buf.len() - header_end < content_length {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                }
                let body = r#"{"error":{"message":"rate limited","type":"rate_limit_error","code":"rate_limit_exceeded"}}"#;
                let resp = format!(
                    "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: 1\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = sock.write_all(resp.as_bytes()).await;
            });
        }
    });

    let model_config = ModelConfig {
        name: "test".to_string(),
        model_id: "stub-model".to_string(),
        endpoint: format!("http://{addr}"),
        api_key: "stub-key".to_string(),
        context_window: 128_000,
        ..ModelConfig::default()
    };
    let mut models = BTreeMap::new();
    models.insert("test".to_string(), model_config);
    let event_bus = crate::comms::EventBus::new();
    let mut shared = AgentShared::new(
        Arc::new(models),
        "test".to_string(),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Vec::new(),
        None,
        None,
    );
    shared.event_bus = Some(event_bus.clone());
    let shared = Arc::new(shared);

    let working_dir = tempfile::tempdir().unwrap();
    let session_id = SessionId::new();
    let args = AgentSpawnArgs {
        base_prompt: "test".to_string(),
        skills: Vec::new(),
        history: Vec::new(),
        session_id: session_id.to_string(),
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
    let mut subscriber = event_bus.subscribe(session_id.clone());
    let mut agent = Agent::new(&shared, args).await;

    let result = agent.handle_streaming_with_retry().await;
    std::env::remove_var("YOMI_STREAM_MAX_RETRIES");
    assert!(result.is_err(), "stub always 429s — the run must fail");

    // Drain events until the Retrying one shows up.
    let mut seen = None;
    let mut error_before_retrying = false;
    while let Ok(Some((_, envelope))) =
        tokio::time::timeout(Duration::from_secs(2), subscriber.recv()).await
    {
        match &envelope.event {
            crate::event::Event::Agent(crate::event::AgentEvent::Error { .. }) => {
                error_before_retrying = true;
            }
            crate::event::Event::Agent(crate::event::AgentEvent::Retrying { .. }) => {
                seen = Some(envelope.event);
                break;
            }
            _ => {}
        }
    }
    // The Error event must precede Retrying so the status card's single
    // phase slot settles on the richer Retrying title (attempt + delay).
    assert!(
        error_before_retrying,
        "Error must be emitted before Retrying"
    );
    let Some(crate::event::Event::Agent(crate::event::AgentEvent::Retrying {
        attempt,
        max_attempts,
        reason,
        wait_ms,
    })) = seen
    else {
        panic!("no Retrying event emitted");
    };
    assert_eq!((attempt, max_attempts), (1, 1));
    assert!(reason.contains("429"), "reason: {reason}");
    // `Retry-After: 1` + up to 25% jitter.
    assert!((1000..=1250).contains(&wait_ms), "wait_ms: {wait_ms}");
}
