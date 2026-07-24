use super::*;

use crate::channels::{ChannelError, ChannelMessage};
use crate::types::ContentBlock;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::sync::{mpsc, Mutex};
use tokio_util::sync::CancellationToken;

// ── Mock adapter ────────────────────────────────────────────────────

struct MockAdapter {
    cards: Mutex<Vec<(String, String, Option<String>)>>, // chat_id, json, anchor
    patches: Mutex<Vec<(String, String)>>,               // msg_id, json
    reactions_added: Mutex<Vec<(String, String)>>,       // msg_id, emoji
    content_msgs: Mutex<Vec<String>>,                    // sent content replies
    counter: AtomicUsize,
    fail_send_cards: std::sync::atomic::AtomicBool,
    send_card_attempts: AtomicUsize,
}

impl MockAdapter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cards: Mutex::new(Vec::new()),
            patches: Mutex::new(Vec::new()),
            reactions_added: Mutex::new(Vec::new()),
            content_msgs: Mutex::new(Vec::new()),
            counter: AtomicUsize::new(0),
            fail_send_cards: std::sync::atomic::AtomicBool::new(false),
            send_card_attempts: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelMessage>,
        cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        _external_chat_id: &str,
        blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        let id = format!("content-{}", self.counter.fetch_add(1, Ordering::Relaxed));
        if let Some(ContentBlock::Text { text }) = blocks.first() {
            self.content_msgs.lock().await.push(text.clone());
        }
        Ok(Some(id))
    }

    async fn send_card(
        &self,
        external_chat_id: &str,
        card_json: &str,
        reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        self.send_card_attempts.fetch_add(1, Ordering::Relaxed);
        if self.fail_send_cards.load(Ordering::Relaxed) {
            return Err(ChannelError::Platform("mock send_card failure".into()));
        }
        self.cards.lock().await.push((
            external_chat_id.to_string(),
            card_json.to_string(),
            reply_msg_id.map(str::to_string),
        ));
        Ok(Some(format!(
            "card-{}",
            self.counter.fetch_add(1, Ordering::Relaxed)
        )))
    }

    async fn update_card(&self, message_id: &str, card_json: &str) -> Result<(), ChannelError> {
        self.patches
            .lock()
            .await
            .push((message_id.to_string(), card_json.to_string()));
        Ok(())
    }

    async fn send_reaction(
        &self,
        _external_chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<Option<String>, ChannelError> {
        self.reactions_added
            .lock()
            .await
            .push((message_id.to_string(), emoji.to_string()));
        Ok(Some(format!(
            "reaction-{}",
            self.counter.fetch_add(1, Ordering::Relaxed)
        )))
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn adapter_ref(mock: &Arc<MockAdapter>) -> Arc<dyn PlatformAdapter> {
    mock.clone()
}

fn sid() -> SessionId {
    SessionId::new()
}

fn running() -> Event {
    Event::Agent(AgentEvent::Lifecycle {
        state: AgentStatus::Running,
    })
}

fn stopped(reason: StopReason) -> Event {
    Event::Agent(AgentEvent::Lifecycle {
        state: AgentStatus::Stopped { reason },
    })
}

fn tool_start(name: &str) -> Event {
    Event::Tool(ToolEvent::Start {
        message_id: crate::types::MessageId::new(),
        tool_id: "t1".to_string(),
        tool_name: name.to_string(),
        arguments: None,
    })
}

fn tool_end(name: &str, elapsed_ms: u64, is_error: bool) -> Event {
    Event::Tool(ToolEvent::End {
        message_id: crate::types::MessageId::new(),
        tool_id: "t1".to_string(),
        tool_name: name.to_string(),
        content_blocks: vec![],
        elapsed_ms,
        is_error,
    })
}

fn request() -> Event {
    Event::Model(ModelEvent::Request {
        message_id: crate::types::MessageId::new(),
        message_count: 1,
    })
}

fn text_chunk(text: &str) -> Event {
    Event::Model(ModelEvent::Chunk {
        message_id: crate::types::MessageId::new(),
        content: crate::event::ContentChunk::Text(text.to_string()),
    })
}

fn thinking_chunk(text: &str) -> Event {
    Event::Model(ModelEvent::Chunk {
        message_id: crate::types::MessageId::new(),
        content: crate::event::ContentChunk::Thinking {
            thinking: text.to_string(),
            signature: None,
        },
    })
}

// ── Lifecycle ───────────────────────────────────────────────────────

#[tokio::test]
async fn card_materializes_on_first_tool_start_not_on_running() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // Running alone never sends a card, no matter how many times it fires.
    for _ in 0..3 {
        tracker
            .handle_event(&adapter, &sid, "chat-1", Some("msg-1"), &running())
            .await;
    }
    assert_eq!(mock.cards.lock().await.len(), 0);

    // First tool start materializes the card with the current anchor.
    tracker
        .handle_event(&adapter, &sid, "chat-1", Some("msg-1"), &tool_start("bash"))
        .await;
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].0, "chat-1");
    assert_eq!(cards[0].2.as_deref(), Some("msg-1"));
    assert!(cards[0].1.contains("🐹 Bash"));
    assert!(cards[0].1.contains("blue"));
    drop(cards);

    // Subsequent tool starts do not open new cards.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("read"))
        .await;
    assert_eq!(mock.cards.lock().await.len(), 1);
}

#[tokio::test]
async fn no_tool_run_never_shows_card_and_sends_no_reactions() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker.record_receipt(&sid, "user-msg-1".into());
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &stopped(StopReason::Completed {
                finish_reason: None,
            }),
        )
        .await;

    // No card, no patches, no reactions — and the receipts are cleared.
    assert_eq!(mock.cards.lock().await.len(), 0);
    assert_eq!(mock.patches.lock().await.len(), 0);
    assert!(mock.reactions_added.lock().await.is_empty());
    assert!(!tracker.has_mid_run_posts(&sid));
}

#[tokio::test]
async fn card_reopens_after_settle() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &stopped(StopReason::Completed {
                finish_reason: None,
            }),
        )
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("read"))
        .await;

    assert_eq!(mock.cards.lock().await.len(), 2);
}

// ── Patching & throttling ───────────────────────────────────────────

#[tokio::test]
async fn tool_events_patch_card_with_stats() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &tool_end("bash", 2300, false),
        )
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("read"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_end("read", 100, true))
        .await;

    let patches = mock.patches.lock().await;
    // Tool ends are watchdog liveness only; only the second tool start PATCHes
    // (the first one sent the card).
    assert_eq!(patches.len(), 1);
    let last = &patches[0].1;
    assert!(last.contains("2 tools"), "tools summary: {last}");
    assert!(last.contains("🐹 Read"), "title: {last}");
    // No per-tool elapsed or failure noise in the body.
    assert!(!last.contains("last:"), "no last tool: {last}");
    assert!(!last.contains("failed"), "no failed count: {last}");
    assert_eq!(mock.cards.lock().await.len(), 1);
}

#[tokio::test]
async fn patch_is_throttled_within_interval() {
    let tracker = ObsTracker::with_patch_interval(Duration::from_hours(1));
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &tool_end("bash", 100, false),
        )
        .await;

    assert_eq!(mock.patches.lock().await.len(), 0);
    // Card was still materialized by the tool start (send is not a PATCH).
    assert_eq!(mock.cards.lock().await.len(), 1);
}

#[tokio::test]
async fn title_reflects_thinking_typing_and_tool_states() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &request())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &text_chunk("hi"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &thinking_chunk("hmm"))
        .await;

    // Card opens with the tool state; patches track request/chunk phases.
    assert!(mock.cards.lock().await[0].1.contains("🐹 Bash"));
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 3);
    assert!(
        patches[0].1.contains("💭 Thinking…"),
        "request: {}",
        patches[0].1
    );
    assert!(
        patches[1].1.contains("🐾 Typing…"),
        "text chunk: {}",
        patches[1].1
    );
    assert!(
        patches[2].1.contains("💭 Thinking…"),
        "thinking chunk: {}",
        patches[2].1
    );
}

// ── Settlement ──────────────────────────────────────────────────────

#[tokio::test]
async fn settle_completed_freezes_card_and_sends_no_reactions() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker.record_receipt(&sid, "user-msg-1".into());
    tracker.record_receipt(&sid, "user-msg-2".into());
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &stopped(StopReason::Completed {
                finish_reason: None,
            }),
        )
        .await;

    // Terminal card: green + done.
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("green"));
    assert!(patches[0].1.contains("✅ Done"));
    drop(patches);

    // No reactions at settlement; receipts cleared.
    assert!(mock.reactions_added.lock().await.is_empty());
    assert!(!tracker.has_mid_run_posts(&sid));
}

#[tokio::test]
async fn settle_cancelled_freezes_card_and_sends_no_reactions() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker.record_receipt(&sid, "user-msg-1".into());
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &stopped(StopReason::Cancelled {
                operation: Some("streaming".to_string()),
            }),
        )
        .await;

    let patches = mock.patches.lock().await;
    assert!(patches[0].1.contains("grey"));
    assert!(patches[0].1.contains("⏹ Stopped"));
    drop(patches);

    assert!(mock.reactions_added.lock().await.is_empty());
    assert!(!tracker.has_mid_run_posts(&sid));
}

#[tokio::test]
async fn settle_failed_shows_error_summary() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &stopped(StopReason::Failed {
                error: "provider exploded".to_string(),
            }),
        )
        .await;

    let patches = mock.patches.lock().await;
    assert!(patches[0].1.contains("red"));
    assert!(patches[0].1.contains("❌ Failed"));
    assert!(patches[0].1.contains("provider exploded"));
}

#[tokio::test]
async fn failed_settle_without_tools_sends_terminal_card() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // Run fails before any tool call: the terminal card is SENT (not
    // skipped) so the user gets an explanation, not just a CrossMark.
    tracker
        .handle_event(&adapter, &sid, "chat-1", Some("msg-1"), &running())
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &stopped(StopReason::Failed {
                error: "provider exploded".to_string(),
            }),
        )
        .await;

    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].0, "chat-1");
    assert_eq!(cards[0].2.as_deref(), Some("msg-1"));
    assert!(cards[0].1.contains("red"));
    assert!(cards[0].1.contains("❌ Failed"));
    assert!(cards[0].1.contains("provider exploded"));
    drop(cards);
    assert_eq!(mock.patches.lock().await.len(), 0);
}

#[tokio::test]
async fn error_event_updates_phase_but_never_settles() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &Event::Agent(AgentEvent::Error {
                phase: crate::event::ErrorPhase::Streaming,
                error: "rate limited".to_string(),
                is_recoverable: false,
            }),
        )
        .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("⚠️ Error: rate limited"));
    assert!(
        patches[0].1.contains("blue"),
        "must not settle: {patches:?}"
    );
}

// ── Watchdog ────────────────────────────────────────────────────────

#[tokio::test]
async fn watchdog_settles_dead_session_card_and_clears_receipts() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker.record_receipt(&sid, "user-msg-1".into());
    tracker.record_receipt(&sid, "user-msg-2".into());
    assert!(tracker.has_mid_run_posts(&sid));
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;

    // Session's agent is gone (crash / lost Stopped).
    tracker.sweep_dead_sessions(|_| false).await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("⏰ Timed out"));
    drop(patches);

    // Timeout settle sends no reactions and clears the run's receipts (so a
    // later run is not misdetected as having mid-run posts).
    assert_eq!(mock.reactions_added.lock().await.len(), 0);
    assert!(!tracker.has_mid_run_posts(&sid));
}

#[tokio::test]
async fn watchdog_drops_unmaterialized_state_silently() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // Run tracked but no tool ever started → nothing visible to settle.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker.sweep_dead_sessions(|_| false).await;

    assert_eq!(mock.cards.lock().await.len(), 0);
    assert_eq!(mock.patches.lock().await.len(), 0);
}

#[tokio::test]
async fn watchdog_keeps_card_while_session_alive() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;

    // A live session (e.g. mid long-running tool call) is never settled,
    // no matter how quiet the event stream is.
    tracker.sweep_dead_sessions(|_| true).await;
    assert!(tracker.states.contains_key(&sid));
    assert!(mock.patches.lock().await.is_empty());
}

// ── Render helpers ──────────────────────────────────────────────────

#[test]
fn fmt_elapsed_human_readable() {
    assert_eq!(fmt_elapsed(Duration::from_secs(45)), "45s");
    assert_eq!(fmt_elapsed(Duration::from_secs(72)), "1m12s");
    assert_eq!(
        fmt_elapsed(Duration::from_hours(1) + Duration::from_mins(2)),
        "1h2m"
    );
}

#[test]
fn humanize_tool_name_camelizes() {
    assert_eq!(humanize_tool_name("web_fetch"), "WebFetch");
    assert_eq!(humanize_tool_name("bash"), "Bash");
    assert_eq!(humanize_tool_name("send_message"), "SendMessage");
    assert_eq!(humanize_tool_name("_multi__seg_"), "MultiSeg");
}

#[test]
fn truncate_chars_appends_ellipsis() {
    assert_eq!(truncate_chars("short", 20), "short");
    assert_eq!(
        truncate_chars("a]very long piece of text here", 5),
        "a]ver…"
    );
}

#[test]
fn token_footer_formats_compact() {
    assert_eq!(fmt_k(999), "999");
    assert_eq!(fmt_k(12_345), "12.3k");
    assert_eq!(fmt_k(200_000), "200.0k");
}

#[test]
fn card_json_uses_compact_layout() {
    let card = card_json("blue", "💭 Thinking…", "⏱ 0s");
    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    assert_eq!(v["config"]["width_mode"], "compact");
    assert_eq!(v["header"]["padding"], "4px 12px 4px 12px");
    assert_eq!(v["body"]["padding"], "8px 12px 8px 12px");
    let elements = v["body"]["elements"].as_array().unwrap();
    assert_eq!(elements.len(), 1);
    assert_eq!(elements[0]["tag"], "markdown");
    assert_eq!(elements[0]["text_size"], "notation");
}

#[tokio::test]
async fn token_usage_adds_footer() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &Event::Model(ModelEvent::TokenUsage {
                message_id: crate::types::MessageId::new(),
                prompt_tokens: 10_000,
                completion_tokens: 2_345,
                total_tokens: 12_345,
                context_window: 200_000,
            }),
        )
        .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    // Tokens merged into the ⏱ stats line.
    assert!(patches[0].1.contains("⏱"));
    assert!(patches[0].1.contains("tokens: 12.3k / 200.0k"));
}

// ── Last tool & whisper ─────────────────────────────────────────────

fn tool_start_with_args(name: &str, args: &str) -> Event {
    Event::Tool(ToolEvent::Start {
        message_id: crate::types::MessageId::new(),
        tool_id: "t1".to_string(),
        tool_name: name.to_string(),
        arguments: Some(args.to_string()),
    })
}

fn end_with_text(text: &str) -> Event {
    Event::Model(ModelEvent::End {
        message_id: crate::types::MessageId::new(),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
    })
}

#[tokio::test]
async fn running_card_shows_last_tool_with_arg_summary() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &tool_start_with_args("shell", r#"{"command":"cargo test -p kernel"}"#),
        )
        .await;

    // The materialized card already carries the last-tool line.
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert!(cards[0].1.contains("🔧 shell · cargo test -p kernel"));
}

#[tokio::test]
async fn whisper_accumulates_streamed_text_and_clears_on_request() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &text_chunk("Let me "))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &text_chunk("run the tests."),
        )
        .await;

    let patches = mock.patches.lock().await;
    let last = &patches.last().unwrap().1;
    assert!(last.contains("💬 Let me run the tests."), "patch: {last}");
    drop(patches);

    // A new model request clears the whisper for the next turn.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &request())
        .await;
    let patches = mock.patches.lock().await;
    let last = &patches.last().unwrap().1;
    assert!(!last.contains("💬"), "whisper cleared: {last}");
}

#[tokio::test]
async fn whisper_self_heals_from_end_text() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    // No chunks at all (e.g. lost on the bus): End still restores the whisper.
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &end_with_text("Full answer."),
        )
        .await;

    let patches = mock.patches.lock().await;
    let last = &patches.last().unwrap().1;
    assert!(last.contains("💬 Full answer."), "patch: {last}");
}

#[tokio::test]
async fn whisper_shows_single_line_tail_when_long() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &end_with_text(&format!("line one\nline two\n{}", "x".repeat(200))),
        )
        .await;

    let patches = mock.patches.lock().await;
    let last = &patches.last().unwrap().1;
    assert!(last.contains("💬 …"), "tail marker: {last}");
    assert!(!last.contains("line one"), "flattened to tail: {last}");
}

#[tokio::test]
async fn terminal_card_freezes_without_whisper() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &text_chunk("working on it"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &stopped(StopReason::Completed {
                finish_reason: None,
            }),
        )
        .await;

    let patches = mock.patches.lock().await;
    let terminal = &patches.last().unwrap().1;
    assert!(terminal.contains("✅ Done"));
    assert!(!terminal.contains("💬"), "terminal drops the whisper");
    assert!(
        !terminal.contains("🔧"),
        "terminal drops the last-tool line"
    );
}

#[test]
fn whisper_snippet_caps_at_100_chars() {
    let long = "x".repeat(300);
    let snippet = whisper_snippet(&long);
    assert_eq!(snippet.chars().count(), 100);
    assert!(snippet.starts_with('…'));

    // Multibyte chars count as one char each.
    let multibyte = "汉".repeat(150);
    let snippet = whisper_snippet(&multibyte);
    assert_eq!(snippet.chars().count(), 100);

    assert_eq!(whisper_snippet("short text"), "short text");
    // Newlines are flattened before measuring.
    assert_eq!(whisper_snippet("line one\nline two"), "line one line two");
}

#[tokio::test]
async fn last_tool_line_is_capped_at_100_chars() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    let long_command = format!(r#"{{"command":"{}"}}"#, "x".repeat(300));
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &tool_start_with_args("shell", &long_command),
        )
        .await;

    let cards = mock.cards.lock().await;
    let body: serde_json::Value = serde_json::from_str(&cards[0].1).unwrap();
    let content = body["body"]["elements"][0]["content"].as_str().unwrap();
    let tool_line = content
        .lines()
        .find(|l| l.starts_with('🔧'))
        .expect("last-tool line");
    // "🔧 " prefix + capped text (≤100 chars, ellipsis included). The
    // arg summary itself caps at 60 (shared with the reply trace), so the
    // composed line stays well under the limit.
    assert!(tool_line.chars().count() <= 2 + 100, "line: {tool_line}");
    assert!(tool_line.ends_with('…'));
}

#[tokio::test]
async fn whisper_line_is_capped_at_100_chars() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &end_with_text(&"y".repeat(300)),
        )
        .await;

    let patches = mock.patches.lock().await;
    let body: serde_json::Value = serde_json::from_str(&patches.last().unwrap().1).unwrap();
    let content = body["body"]["elements"][0]["content"].as_str().unwrap();
    let whisper_line = content
        .lines()
        .find(|l| l.contains('💬'))
        .expect("whisper line");
    // "<font color='grey'>💬 …" wrapper + ≤100-char snippet.
    let snippet = whisper_line
        .trim_start_matches("<font color='grey'>💬 ")
        .trim_end_matches("</font>");
    assert_eq!(snippet.chars().count(), 100, "snippet: {snippet}");
    assert!(snippet.starts_with('…'));
}

// ── Morph settlement (one message per run) ──────────────────────────

fn reply_with(text: &str) -> crate::channels::reply::FinalReply {
    let mut buf = crate::channels::reply::RunReplyBuffer::new();
    buf.record_text("intermediate thought".to_string());
    buf.record_tool_start("t1", "shell", Some(r#"{"command":"cargo test"}"#));
    buf.record_tool_end("t1", 1200, false);
    buf.record_text(text.to_string());
    buf.into_reply()
}

#[tokio::test]
async fn stopped_morphs_status_card_into_final_reply() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("the final answer")),
        )
        .await;

    // No new message: the status card is PATCHed into the reply.
    assert_eq!(mock.cards.lock().await.len(), 1);
    let patches = mock.patches.lock().await;
    let morphed: serde_json::Value = serde_json::from_str(&patches.last().unwrap().1).unwrap();
    assert!(morphed["header"].is_null(), "no header after morph");
    let elements = morphed["body"]["elements"].as_array().unwrap();
    assert_eq!(elements[0]["content"], "the final answer");
    assert_eq!(elements[1]["tag"], "collapsible_panel");
    let panel = elements[1]["elements"][0]["content"].as_str().unwrap();
    assert!(panel.contains("💬 intermediate thought"));
    assert!(panel.contains("✅ **shell** · `cargo test`"));
}

#[tokio::test]
async fn stopped_without_card_sends_reply_as_new_message() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // Pure Q&A run: no tools → no status card was materialized.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("plain answer")),
        )
        .await;

    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    let card: serde_json::Value = serde_json::from_str(&cards[0].1).unwrap();
    assert!(card["header"].is_null());
    assert_eq!(card["body"]["elements"][0]["content"], "plain answer");
    assert!(mock.patches.lock().await.is_empty());
}

#[tokio::test]
async fn stopped_failed_shows_error_notice_in_content() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Failed {
                error: "provider exploded".to_string(),
            },
            Some(reply_with("partial answer")),
        )
        .await;

    let patches = mock.patches.lock().await;
    let morphed: serde_json::Value = serde_json::from_str(&patches.last().unwrap().1).unwrap();
    assert!(morphed["header"].is_null(), "no red header after morph");
    let elements = morphed["body"]["elements"].as_array().unwrap();
    assert_eq!(elements[0]["content"], "❌ **Error**  provider exploded");
    assert_eq!(elements[1]["content"], "partial answer");
}

#[tokio::test]
async fn stopped_failed_without_card_sends_notice_card() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    // No tools, no text — but the failure must still be explained.
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Failed {
                error: "stream died".to_string(),
            },
            None,
        )
        .await;

    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert!(cards[0].1.contains("stream died"));
}

#[tokio::test]
async fn timeout_morphs_card_and_late_stopped_clears_receipts() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker.record_receipt(&sid, "user-msg-1".into());
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;

    // Watchdog fires first (agent crash): card morphs with a timeout notice;
    // receipts are cleared by the timeout settle.
    tracker
        .handle_timeout(&sid, Some(reply_with("partial output")))
        .await;
    let patches = mock.patches.lock().await;
    let morphed: serde_json::Value = serde_json::from_str(&patches.last().unwrap().1).unwrap();
    let elements = morphed["body"]["elements"].as_array().unwrap();
    assert_eq!(elements[0]["content"], "⏰ Session lost (timed out)");
    assert_eq!(elements[1]["content"], "partial output");
    assert!(!tracker.has_mid_run_posts(&sid), "cleared by timeout");
    tracker.record_receipt(&sid, "mid-run".into());
    drop(patches);

    // The late real `Stopped` clears the receipts; no reactions are sent.
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            None,
        )
        .await;
    assert!(mock.reactions_added.lock().await.is_empty());
    assert!(!tracker.has_mid_run_posts(&sid));
}

#[tokio::test]
async fn first_text_chunk_materializes_card_without_tools() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", Some("msg-1"), &running())
        .await;
    assert_eq!(mock.cards.lock().await.len(), 0);

    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            Some("msg-1"),
            &text_chunk("Hello"),
        )
        .await;
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "text output materializes the card");
    assert!(cards[0].1.contains("💬 Hello"));
    assert!(cards[0].1.contains("🐾 Typing"));
    drop(cards);

    // The run then morphs that very card into the final reply.
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("Hello, done.")),
        )
        .await;
    assert_eq!(mock.cards.lock().await.len(), 1, "no extra message");
    let patches = mock.patches.lock().await;
    let morphed: serde_json::Value = serde_json::from_str(&patches.last().unwrap().1).unwrap();
    assert_eq!(morphed["body"]["elements"][0]["content"], "Hello, done.");
}

#[tokio::test]
async fn thinking_chunk_materializes_card_with_thinking_title() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &thinking_chunk("hmm"))
        .await;
    // The card appears as soon as the model starts responding, even while
    // it is still thinking (reasoning models can think for a long time).
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert!(cards[0].1.contains("💭 Thinking"));
    // Thinking content itself is never rendered (internal reasoning).
    assert!(!cards[0].1.contains("hmm"));
    assert!(!cards[0].1.contains("💬"));
}

#[tokio::test]
async fn tool_start_updates_are_throttled_uniformly() {
    // Huge patch interval: no throttled PATCH may fire — tool starts mutate
    // state only; the tool title rides the next regular render (or the
    // materialization render, for the tool that opens the card).
    let tracker = ObsTracker::with_patch_interval(Duration::from_hours(1));
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "first tool materializes");
    assert!(
        cards[0].1.contains("🐹 Bash"),
        "creation render shows the tool"
    );
    drop(cards);

    // A subsequent tool start inside the throttle window does NOT PATCH.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("read"))
        .await;
    assert_eq!(mock.patches.lock().await.len(), 0, "uniform throttling");
}

#[test]
fn mid_run_posts_detection_uses_receipts() {
    let tracker = ObsTracker::new();
    let sid = sid();
    // No receipts yet (or only the trigger) → no mid-run posts.
    assert!(!tracker.has_mid_run_posts(&sid));
    tracker.record_receipt(&sid, "trigger".into());
    assert!(!tracker.has_mid_run_posts(&sid));
    // A second platform message during the run → mid-run post.
    tracker.record_receipt(&sid, "mid-run".into());
    assert!(tracker.has_mid_run_posts(&sid));
}

// ── Settle fallback & materialize failure ───────────────────────────

#[tokio::test]
async fn settle_returns_reply_when_no_run_state() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    // No `Running` ever tracked (event lost): the settle must hand the
    // reply back so the caller can fall back to a plain send.
    let returned = tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("the answer")),
        )
        .await;
    assert!(returned.is_some(), "no state → reply handed back");
    assert!(mock.cards.lock().await.is_empty());
    assert!(mock.patches.lock().await.is_empty());
}

#[tokio::test]
async fn settle_returns_reply_when_settle_send_fails() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // State exists but the card never materialized (send fails throughout).
    mock.fail_send_cards.store(true, Ordering::Relaxed);
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    let returned = tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("the answer")),
        )
        .await;
    assert!(returned.is_some(), "settle send failed → reply handed back");
}

#[tokio::test]
async fn materialize_send_failure_disables_retries_for_the_run() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);
    mock.fail_send_cards.store(true, Ordering::Relaxed);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("read"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &text_chunk("hi"))
        .await;

    // Exactly one materialize attempt despite several triggers — no storm
    // against a struggling API endpoint.
    assert_eq!(mock.send_card_attempts.load(Ordering::Relaxed), 1);
    assert_eq!(mock.cards.lock().await.len(), 0);
}
