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
        ext_tools: Vec::new(),
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
        ext_tools: Vec::new(),
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
        ext_tools: Vec::new(),
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

/// `/compact` 的事件契约：手动 compact（无 run 包裹）必须发出
/// `Compacting{true}` → `Compacting{false}` → `Compacted` 序列——频道状态卡
/// 靠 `Compacting` 建卡、靠 `Compacted` 结算；早期失败（此处用空模型表触发
/// 模型解析失败）也不例外，否则卡片永远转圈。
#[tokio::test]
async fn force_full_compact_emits_event_bracket_on_early_failure() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::event::{Event, ModelEvent};
    use crate::types::SessionId;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let event_bus = crate::comms::EventBus::new();
    let mut shared = AgentShared::new(
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
        ext_tools: Vec::new(),
    };
    let mut subscriber = event_bus.subscribe(session_id.clone());
    let mut agent = Agent::new(&shared, args).await;

    let result = agent.force_full_compact().await;
    assert!(result.is_err(), "no models configured — must fail");

    // Drain until Compacted shows up, collecting the compact event flow.
    let mut flow = Vec::new();
    while let Ok(Some((_, envelope))) =
        tokio::time::timeout(Duration::from_secs(2), subscriber.recv()).await
    {
        let is_terminal = matches!(envelope.event, Event::Model(ModelEvent::Compacted { .. }));
        match &envelope.event {
            Event::Model(ModelEvent::Compacting { active }) => {
                flow.push(format!("compacting:{active}"));
            }
            Event::Model(ModelEvent::Compacted { summary, is_error }) => {
                flow.push(format!("compacted:{is_error}:{summary}"));
            }
            _ => {}
        }
        if is_terminal {
            break;
        }
    }
    assert_eq!(flow.len(), 3, "full bracket expected, got: {flow:?}");
    assert_eq!(flow[0], "compacting:true");
    assert_eq!(flow[1], "compacting:false");
    assert!(
        flow[2].starts_with("compacted:true:Model resolution failed"),
        "early failure must settle as an error outcome: {}",
        flow[2]
    );
}

/// 从事件流里取下一条 `Compacted`（忽略其余事件）。
async fn next_compacted(subscriber: &mut crate::comms::EventBusSubscriber) -> (String, bool) {
    while let Ok(Some((_, envelope))) =
        tokio::time::timeout(Duration::from_secs(2), subscriber.recv()).await
    {
        if let crate::event::Event::Model(crate::event::ModelEvent::Compacted {
            summary,
            is_error,
        }) = envelope.event
        {
            return (summary, is_error);
        }
    }
    panic!("no Compacted event emitted");
}

/// `handle_compaction_result` 把任何结局落成 `Compacted` 事件：成功、取消、
/// API 失败都结算（取消另发 `Stopped{Cancelled}`，此处只断言 Compacted）。
#[tokio::test]
async fn compaction_result_emits_compacted_outcome() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::compactor::CompactionError;
    use crate::provider::ModelConfig;
    use crate::types::SessionId;
    use std::collections::BTreeMap;
    use std::sync::Arc;

    let event_bus = crate::comms::EventBus::new();
    let mut shared = AgentShared::new(
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
        ext_tools: Vec::new(),
    };
    let mut subscriber = event_bus.subscribe(session_id.clone());
    let mut agent = Agent::new(&shared, args).await;
    let model_config = ModelConfig::default();

    // Success with nothing to do.
    let ok = agent
        .handle_compaction_result(Ok(None), 0, &model_config)
        .await;
    assert_eq!(ok.as_deref(), Ok("No compaction needed"));
    assert_eq!(
        next_compacted(&mut subscriber).await,
        ("No compaction needed".to_string(), false)
    );

    // Cancelled.
    let cancelled = agent
        .handle_compaction_result(Err(CompactionError::Cancelled), 0, &model_config)
        .await;
    assert!(matches!(cancelled.as_deref(), Err(e) if e == "Compaction was cancelled"));
    assert_eq!(
        next_compacted(&mut subscriber).await,
        ("Compaction was cancelled".to_string(), true)
    );

    // API failure.
    let failed = agent
        .handle_compaction_result(
            Err(CompactionError::Api("boom".to_string())),
            0,
            &model_config,
        )
        .await;
    assert!(matches!(failed.as_deref(), Err(e) if e.contains("boom")));
    let (summary, is_error) = next_compacted(&mut subscriber).await;
    assert!(is_error);
    assert!(summary.contains("boom"), "summary: {summary}");
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
        ext_tools: Vec::new(),
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
        ext_tools: Vec::new(),
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

/// 中断标记（mark_interrupted）：落库为带 metadata 的 user 消息，且通过
/// has_user_after guard 使 pending_tool_calls 收口为 None——被打断的工具
/// 批不会在重生后被静默重跑。
#[tokio::test]
async fn interrupted_marker_closes_pending_tool_batch() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::provider::ModelConfig;
    use crate::storage::UsageStore;
    use crate::types::{Message, Role, SessionId, ToolCall};
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

    let mut models = BTreeMap::new();
    models.insert(
        "test".to_string(),
        ModelConfig {
            name: "test".to_string(),
            model_id: "test-id".to_string(),
            ..ModelConfig::default()
        },
    );
    let store_dir = tempfile::tempdir().unwrap();
    let message_store: Arc<dyn crate::storage::MessageStore> =
        Arc::new(crate::storage::message::jsonl::JsonlMessageStore::new(
            store_dir.path().to_path_buf(),
            store_dir.path().to_path_buf(),
        ));
    let shared = Arc::new(AgentShared::new(
        Arc::new(models),
        "test".to_string(),
        None,
        None,
        None,
        None,
        Some(message_store),
        Some(usage_store),
        None,
        Vec::new(),
        None,
        None,
    ));

    // 历史：一个被打断的工具批（assistant 带 tool_calls、无结果）。
    let call = ToolCall {
        id: "call-1".to_string(),
        name: "shell".to_string(),
        arguments: serde_json::json!({"command": "brew upgrade yomi"}),
    };
    let history = vec![
        Arc::new(Message::user("升级一下")),
        Arc::new(Message {
            role: Role::Assistant,
            tool_calls: Some(vec![call]),
            ..Default::default()
        }),
    ];

    let working_dir = tempfile::tempdir().unwrap();
    let args = AgentSpawnArgs {
        base_prompt: "test".to_string(),
        skills: Vec::new(),
        history,
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
        ext_tools: Vec::new(),
    };
    let mut agent = Agent::new(&shared, args).await;

    // 打断前有 pending；标记后收口。
    assert!(agent.pending_tool_calls().is_some());
    agent
        .mark_interrupted("daemon restarting — outcome of interrupted work unknown")
        .await;

    // 直写落盘：store 里能读回（不依赖事件总线）。
    let sid = agent.session_id.0.clone();
    let persisted = shared
        .message_store
        .as_ref()
        .unwrap()
        .get(&sid)
        .await
        .unwrap();
    let marker = persisted.last().expect("marker persisted");
    assert_eq!(marker.role, Role::User);
    assert_eq!(
        marker
            .metadata
            .as_ref()
            .and_then(|m| m.get(crate::types::INTERRUPTED_META_KEY))
            .map(String::as_str),
        Some("true")
    );

    let messages = agent.message_buffer.messages();
    let last = messages.last().unwrap();
    assert_eq!(last.role, Role::User);
    let text = match &last.content[0] {
        crate::types::ContentBlock::Text { text } => text.as_str(),
        other => panic!("expected text block, got {other:?}"),
    };
    assert!(text.contains("interrupted: daemon restarting"));
    assert_eq!(
        last.metadata
            .as_ref()
            .and_then(|m| m.get(crate::types::INTERRUPTED_META_KEY))
            .map(String::as_str),
        Some("true")
    );
    assert!(agent.pending_tool_calls().is_none());
}

/// Rewind 语义：以 message id 为准，上下文（buffer）和 checkpoint 两个存储
/// 各自尽力处理——有哪边处理哪边，两边都没有才报 "not found"。
/// 背景：compaction 会重写 buffer 但保留 checkpoint（旧 id 悬空）；
/// 反过来消息也可能没有 checkpoint。
mod rewind_tests {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::checkpoint::{CheckpointStore, FilesystemCheckpointStore, RewindTarget};
    use crate::provider::ModelConfig;
    use crate::types::{Message, MessageId, SessionId};
    use std::collections::BTreeMap;
    use std::sync::Arc;

    struct RewindHarness {
        agent: Agent,
        checkpoint_store: Arc<dyn CheckpointStore>,
        session_id: String,
        event_bus: Arc<crate::comms::EventBus>,
        // 持有 tempdir，drop 时自动清理。
        _data_dir: tempfile::TempDir,
        _working_dir: tempfile::TempDir,
    }

    async fn build(history: Vec<Arc<Message>>) -> RewindHarness {
        let data_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let checkpoint_store: Arc<dyn CheckpointStore> =
            Arc::new(FilesystemCheckpointStore::new(data_dir.path()));

        let mut models = BTreeMap::new();
        models.insert(
            "test".to_string(),
            ModelConfig {
                name: "test".to_string(),
                model_id: "test-id".to_string(),
                ..ModelConfig::default()
            },
        );
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
            Some(checkpoint_store.clone()),
        );
        let event_bus = crate::comms::EventBus::new();
        shared.event_bus = Some(event_bus.clone());
        let shared = Arc::new(shared);

        let session_id = SessionId::new().to_string();
        let args = AgentSpawnArgs {
            base_prompt: "test".to_string(),
            skills: Vec::new(),
            history,
            session_id: session_id.clone(),
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
            ext_tools: Vec::new(),
        };
        let agent = Agent::new(&shared, args).await;
        RewindHarness {
            agent,
            checkpoint_store,
            session_id,
            event_bus,
            _data_dir: data_dir,
            _working_dir: working_dir,
        }
    }

    /// 消息在 buffer 里、但没有 checkpoint：只截断上下文，不报错。
    #[tokio::test]
    async fn rewind_without_checkpoint_only_truncates_context() {
        let m1 = Arc::new(Message::user("first"));
        let m2 = Arc::new(Message::user("second"));
        let target_id = m1.id.clone();
        let mut harness = build(vec![m1, m2]).await;
        let before = harness.agent.message_buffer.len();

        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        harness
            .agent
            .process_rewind(target_id.clone(), RewindTarget::Both, tx)
            .await
            .expect("rewind should succeed without checkpoint");
        rx.recv().await.expect("result sent").expect("ok result");

        let messages = harness.agent.message_buffer.messages();
        assert_eq!(messages.len(), before - 2);
        assert!(!messages.iter().any(|m| m.id == target_id));
    }

    /// compact 后的场景：消息已不在 buffer，但 checkpoint 还在（悬空 id）：
    /// 只处理 checkpoint（删除目标及之后的 checkpoint），不报错。
    #[tokio::test]
    async fn rewind_stale_checkpoint_after_compact_only_processes_checkpoint() {
        let mut harness = build(vec![Arc::new(Message::user("kept"))]).await;
        let stale_id = MessageId::new();
        let later_id = MessageId::new();
        harness
            .checkpoint_store
            .create_checkpoint(&harness.session_id, stale_id.as_str(), "old turn", vec![])
            .await
            .unwrap();
        harness
            .checkpoint_store
            .create_checkpoint(&harness.session_id, later_id.as_str(), "later turn", vec![])
            .await
            .unwrap();

        let before_msgs = harness.agent.message_buffer.len();
        let mut subscriber = harness
            .event_bus
            .subscribe(SessionId::from(harness.session_id.clone()));
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        harness
            .agent
            .process_rewind(stale_id.clone(), RewindTarget::Both, tx)
            .await
            .expect("rewind should succeed for stale checkpoint");
        rx.recv().await.expect("result sent").expect("ok result");

        // 上下文不动，目标及之后的 checkpoint 被删除。
        assert_eq!(harness.agent.message_buffer.len(), before_msgs);
        let remaining = harness
            .checkpoint_store
            .get_session_checkpoints(&harness.session_id)
            .await
            .unwrap();
        assert!(remaining.is_empty(), "checkpoints should be rewound");

        // 即使上下文没动，也必须发 MessageReplaced：checkpoint 回滚会把磁盘
        // 消息快照恢复成旧版本，持久化层需要用当前 buffer 覆盖回去，否则
        // session 重载后被 compact 掉的旧消息会复活。
        let (_, envelope) =
            tokio::time::timeout(std::time::Duration::from_secs(2), subscriber.recv())
                .await
                .expect("MessageReplaced emitted")
                .unwrap();
        assert!(
            matches!(
                envelope.event,
                crate::event::Event::Internal(crate::event::InternalEvent::MessageReplaced { .. })
            ),
            "expected MessageReplaced, got {:?}",
            envelope.event
        );
    }

    /// 正常 /undo 主路径：消息在 buffer 且有 checkpoint——两边都处理：
    /// 截断上下文并删除目标及之后的 checkpoint。
    #[tokio::test]
    async fn rewind_with_both_sides_processes_context_and_checkpoint() {
        let m1 = Arc::new(Message::user("first"));
        let m2 = Arc::new(Message::user("second"));
        let target_id = m1.id.clone();
        let mut harness = build(vec![m1, m2]).await;
        harness
            .checkpoint_store
            .create_checkpoint(&harness.session_id, target_id.as_str(), "turn one", vec![])
            .await
            .unwrap();
        harness
            .checkpoint_store
            .create_checkpoint(
                &harness.session_id,
                MessageId::new().as_str(),
                "turn two",
                vec![],
            )
            .await
            .unwrap();
        let before = harness.agent.message_buffer.len();

        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        harness
            .agent
            .process_rewind(target_id.clone(), RewindTarget::Both, tx)
            .await
            .expect("rewind should succeed");
        rx.recv().await.expect("result sent").expect("ok result");

        let messages = harness.agent.message_buffer.messages();
        assert_eq!(messages.len(), before - 2);
        assert!(!messages.iter().any(|m| m.id == target_id));
        let remaining = harness
            .checkpoint_store
            .get_session_checkpoints(&harness.session_id)
            .await
            .unwrap();
        assert!(
            remaining.is_empty(),
            "target and later checkpoints should be deleted"
        );
    }

    /// Files-only 回滚（TUI 可达）：只处理 checkpoint，对话上下文必须不动。
    #[tokio::test]
    async fn rewind_files_only_keeps_context() {
        let m1 = Arc::new(Message::user("first"));
        let target_id = m1.id.clone();
        let mut harness = build(vec![m1]).await;
        harness
            .checkpoint_store
            .create_checkpoint(&harness.session_id, target_id.as_str(), "turn one", vec![])
            .await
            .unwrap();
        let before = harness.agent.message_buffer.len();

        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        harness
            .agent
            .process_rewind(target_id.clone(), RewindTarget::Files, tx)
            .await
            .expect("files-only rewind should succeed");
        rx.recv().await.expect("result sent").expect("ok result");

        assert_eq!(harness.agent.message_buffer.len(), before);
        assert!(
            harness
                .agent
                .message_buffer
                .messages()
                .iter()
                .any(|m| m.id == target_id),
            "files-only rewind must not truncate the conversation"
        );
        let remaining = harness
            .checkpoint_store
            .get_session_checkpoints(&harness.session_id)
            .await
            .unwrap();
        assert!(remaining.is_empty());
    }

    /// 两边都没有这个 id：仍然报 not found。
    #[tokio::test]
    async fn rewind_unknown_id_still_errors() {
        let mut harness = build(vec![Arc::new(Message::user("kept"))]).await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let result = harness
            .agent
            .process_rewind(MessageId::new(), RewindTarget::Both, tx)
            .await;
        let err = result.expect_err("unknown id should error");
        assert!(err.to_string().contains("not found"), "err: {err}");
        rx.recv()
            .await
            .expect("result sent")
            .expect_err("err result");
    }
}

/// 空 completion 毒化回归：模型抽风返回零内容（只有 usage + 无法映射的
/// finish_reason）时，回合以 Failed 干净收场，且**不落盘**空 assistant
/// 消息——否则它随每次后续请求重放，被严格网关以 400 "assistant must not
/// be empty" 拒绝，session 被永久毒化。
#[tokio::test]
async fn empty_completion_is_not_persisted_and_fails_turn_cleanly() {
    use crate::agent::{Agent, AgentShared, AgentSpawnArgs};
    use crate::provider::ModelConfig;
    use crate::types::{Role, SessionId};
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Stub model endpoint: read the full request, then answer with an empty
    // completion — no content deltas, only usage and an unmappable
    // finish_reason (=> FinishReason::Unknown).
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
                let body = concat!(
                    "data: {\"id\":\"chatcmpl-poison\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\"},\"finish_reason\":null}]}\n\n",
                    "data: {\"id\":\"chatcmpl-poison\",\"object\":\"chat.completion.chunk\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"mystery_hiccup\"}],\"usage\":{\"prompt_tokens\":500,\"completion_tokens\":1,\"total_tokens\":501}}\n\n",
                    "data: [DONE]\n\n",
                );
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
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
        ext_tools: Vec::new(),
    };
    let mut subscriber = event_bus.subscribe(session_id.clone());
    let mut agent = Agent::new(&shared, args).await;

    let result = agent.handle_streaming_with_retry().await;
    assert!(
        result.is_ok(),
        "turn must fail gracefully, not with an error: {result:?}"
    );

    // The poison: nothing may be persisted for the empty completion.
    assert!(
        !agent
            .message_buffer
            .messages()
            .iter()
            .any(|m| m.role == Role::Assistant),
        "empty completion must not persist an assistant message"
    );

    // ...but the turn must surface as Failed, not silently Completed.
    let mut failed_error = None;
    while let Ok(Some((_, envelope))) =
        tokio::time::timeout(Duration::from_secs(2), subscriber.recv()).await
    {
        if let crate::event::Event::Agent(crate::event::AgentEvent::Lifecycle {
            state:
                crate::event::AgentStatus::Stopped {
                    reason: crate::event::StopReason::Failed { error },
                },
        }) = &envelope.event
        {
            failed_error = Some(error.clone());
            break;
        }
    }
    let error = failed_error.expect("turn must end with a Failed lifecycle event");
    assert!(
        error.contains("inconsistent model stream completion"),
        "error: {error}"
    );
}
