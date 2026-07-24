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
    reactions_removed: Mutex<Vec<(String, String)>>,     // msg_id, reaction_id
    content_msgs: Mutex<Vec<String>>,                    // sent content replies
    counter: AtomicUsize,
}

impl MockAdapter {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            cards: Mutex::new(Vec::new()),
            patches: Mutex::new(Vec::new()),
            reactions_added: Mutex::new(Vec::new()),
            reactions_removed: Mutex::new(Vec::new()),
            content_msgs: Mutex::new(Vec::new()),
            counter: AtomicUsize::new(0),
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

    async fn remove_reaction(
        &self,
        message_id: &str,
        reaction_id: &str,
    ) -> Result<(), ChannelError> {
        self.reactions_removed
            .lock()
            .await
            .push((message_id.to_string(), reaction_id.to_string()));
        Ok(())
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
    assert!(cards[0].1.contains("🐶 Bash"));
    assert!(cards[0].1.contains("blue"));
    drop(cards);

    // Subsequent tool starts do not open new cards.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("read"))
        .await;
    assert_eq!(mock.cards.lock().await.len(), 1);
}

#[tokio::test]
async fn no_tool_run_never_shows_card_but_settles_receipts() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker.record_receipt(&sid, "user-msg-1".into(), "ack-1".into());
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

    // No card, no patches — but the reaction state machine still settles.
    assert_eq!(mock.cards.lock().await.len(), 0);
    assert_eq!(mock.patches.lock().await.len(), 0);
    let added = mock.reactions_added.lock().await;
    assert_eq!(
        added.as_slice(),
        [("user-msg-1".to_string(), "DONE".to_string())]
    );
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
    assert!(last.contains("🐶 Read"), "title: {last}");
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
    assert!(mock.cards.lock().await[0].1.contains("🐶 Bash"));
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
async fn settle_completed_switches_all_reactions() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker.record_receipt(&sid, "user-msg-1".into(), "ack-1".into());
    tracker.record_receipt(&sid, "user-msg-2".into(), "ack-2".into());
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker.record_content_msg(&sid, "content-9".into());
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

    // Ack reactions removed; DONE applied to user messages + last content.
    let removed = mock.reactions_removed.lock().await;
    assert_eq!(
        removed.as_slice(),
        [
            ("user-msg-1".to_string(), "ack-1".to_string()),
            ("user-msg-2".to_string(), "ack-2".to_string()),
        ]
    );
    let added = mock.reactions_added.lock().await;
    assert_eq!(
        added.as_slice(),
        [
            ("user-msg-1".to_string(), "DONE".to_string()),
            ("user-msg-2".to_string(), "DONE".to_string()),
            ("content-9".to_string(), "DONE".to_string()),
        ]
    );
}

#[tokio::test]
async fn settle_cancelled_uses_crossmark() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker.record_receipt(&sid, "user-msg-1".into(), "ack-1".into());
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

    let added = mock.reactions_added.lock().await;
    assert_eq!(
        added.as_slice(),
        [("user-msg-1".to_string(), "CrossMark".to_string())]
    );
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
async fn watchdog_settles_dead_session_card_without_touching_receipts() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker.record_receipt(&sid, "user-msg-1".into(), "ack-1".into());
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

    // Receipts untouched on timeout.
    assert_eq!(mock.reactions_added.lock().await.len(), 0);
    assert_eq!(mock.reactions_removed.lock().await.len(), 0);
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
