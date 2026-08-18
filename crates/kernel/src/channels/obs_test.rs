use super::*;

use crate::channels::{ChannelError, ChannelEvent};
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
    fail_send_cards: std::sync::atomic::AtomicBool,
    send_card_attempts: AtomicUsize,
    fail_patches: std::sync::atomic::AtomicBool,
    patch_attempts: AtomicUsize,
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
            fail_send_cards: std::sync::atomic::AtomicBool::new(false),
            send_card_attempts: AtomicUsize::new(0),
            fail_patches: std::sync::atomic::AtomicBool::new(false),
            patch_attempts: AtomicUsize::new(0),
        })
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelEvent>,
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
        self.patch_attempts.fetch_add(1, Ordering::Relaxed);
        if self.fail_patches.load(Ordering::Relaxed) {
            return Err(ChannelError::Platform("mock patch failure".into()));
        }
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

    async fn delete_reaction(
        &self,
        _external_chat_id: &str,
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

fn compacting(active: bool) -> Event {
    Event::Model(ModelEvent::Compacting { active })
}

fn compacted(summary: &str, is_error: bool) -> Event {
    Event::Model(ModelEvent::Compacted {
        summary: summary.to_string(),
        is_error,
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
async fn card_materializes_on_running_and_never_twice() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // The first Running materializes the card with the current anchor;
    // repeat Running events (Running fires per turn) open no new cards.
    for _ in 0..3 {
        tracker
            .handle_event(&adapter, &sid, "chat-1", Some("msg-1"), &running())
            .await;
    }
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].0, "chat-1");
    assert_eq!(cards[0].2.as_deref(), Some("msg-1"));
    assert!(cards[0].1.contains("blue"));
    drop(cards);

    // Tool starts do not open new cards either.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    assert_eq!(mock.cards.lock().await.len(), 1);
}

#[tokio::test]
async fn no_tool_run_settles_card_in_place_and_sends_no_reactions() {
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

    // The card materialized at Running is settled in place (a freeze —
    // the mid-run post keeps the reply from morphing); no new message,
    // no reactions — and the receipts are cleared.
    assert_eq!(mock.cards.lock().await.len(), 1);
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("✅ **Done**"));
    drop(patches);
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
    // Every tool event PATCHes at ZERO interval: start1, end1, start2,
    // end2 (the Running event sent the card instead).
    assert_eq!(patches.len(), 4);
    let last = &patches[3].1;
    // Tool totals are no longer titled (traffic segments carry the cost
    // shape); the failed counter still is.
    assert!(last.contains("❌ 1"), "failed summary: {last}");
    // The title drops back to a thinking title after the tool ends.
    assert!(
        super::THINKING_TITLES.iter().any(|t| last.contains(t)),
        "title: {last}"
    );
    // The live trace shows the finished tool with elapsed and the failed
    // one with the error icon.
    assert!(last.contains("✅ **bash** · 2s"), "trace: {last}");
    assert!(last.contains("❌ **read**"), "trace: {last}");
    // …and the moment it finished, bash WAS the current step.
    assert!(
        patches[1].1.contains("✅ **bash** · 2s"),
        "then-current: {}",
        patches[1].1
    );
    assert_eq!(mock.cards.lock().await.len(), 1);
}

#[tokio::test]
async fn refresh_stale_patches_frozen_card() {
    // A huge patch interval suppresses event-driven PATCHes: after the
    // tool starts the card would stay frozen without the heartbeat.
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
    assert_eq!(mock.patches.lock().await.len(), 0);

    // The heartbeat refreshes the frozen card, showing the in-flight tool.
    tracker.refresh_stale(Duration::ZERO).await;
    {
        let patches = mock.patches.lock().await;
        assert_eq!(patches.len(), 1);
        assert!(patches[0].1.contains("bash"), "patch: {}", patches[0].1);
    }

    // A just-refreshed card is not stale: a huge threshold suppresses it.
    tracker.refresh_stale(Duration::from_hours(1)).await;
    assert_eq!(mock.patches.lock().await.len(), 1);
}

#[tokio::test]
async fn refresh_stale_ignores_settled_card() {
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
            &stopped(StopReason::Completed {
                finish_reason: None,
            }),
        )
        .await;
    let before = mock.patches.lock().await.len() + mock.cards.lock().await.len();

    // The settled run left the states map: the heartbeat finds nothing.
    tracker.refresh_stale(Duration::ZERO).await;
    let after = mock.patches.lock().await.len() + mock.cards.lock().await.len();
    assert_eq!(before, after);
}

#[tokio::test]
async fn refresh_stale_breaker_trips_after_consecutive_patch_failures() {
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
    assert_eq!(mock.patch_attempts.load(Ordering::Relaxed), 1);

    // Two consecutive failures, then a success resets the counter.
    mock.fail_patches.store(true, Ordering::Relaxed);
    tracker.refresh_stale(Duration::ZERO).await;
    tracker.refresh_stale(Duration::ZERO).await;
    mock.fail_patches.store(false, Ordering::Relaxed);
    tracker.refresh_stale(Duration::ZERO).await;
    assert_eq!(mock.patch_attempts.load(Ordering::Relaxed), 4);

    // LIMIT consecutive failures trip the breaker: later heartbeats
    // don't even attempt the PATCH.
    mock.fail_patches.store(true, Ordering::Relaxed);
    for _ in 0..super::PATCH_FAILURE_LIMIT {
        tracker.refresh_stale(Duration::ZERO).await;
    }
    let attempts = mock.patch_attempts.load(Ordering::Relaxed);
    assert_eq!(attempts, 4 + super::PATCH_FAILURE_LIMIT as usize);
    tracker.refresh_stale(Duration::ZERO).await;
    assert_eq!(mock.patch_attempts.load(Ordering::Relaxed), attempts);
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

    // The card opens at Running (placeholder); patches track tool and
    // request/chunk phases.
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 4);
    assert!(patches[0].1.contains("🐹 Bash"), "tool: {}", patches[0].1);
    assert!(
        super::THINKING_TITLES
            .iter()
            .any(|t| patches[1].1.contains(t)),
        "request: {}",
        patches[1].1
    );
    assert!(
        super::TYPING_TITLES
            .iter()
            .any(|t| patches[2].1.contains(t)),
        "text chunk: {}",
        patches[2].1
    );
    assert!(
        super::THINKING_TITLES
            .iter()
            .any(|t| patches[3].1.contains(t)),
        "thinking chunk: {}",
        patches[3].1
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

    // Terminal card (last patch — the tool start patched first): green + done.
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 2);
    let terminal = &patches.last().unwrap().1;
    let card: serde_json::Value = serde_json::from_str(terminal).unwrap();
    assert!(
        card.get("header").is_none(),
        "terminal receipt has no header"
    );
    assert!(terminal.contains("✅ **Done**"));
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
    assert!(patches[0].1.contains("collapsible_panel"));
    assert!(patches[0].1.contains("⏹ **Stopped**"));
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
    let card: serde_json::Value = serde_json::from_str(&patches[0].1).unwrap();
    assert!(
        card.get("header").is_none(),
        "terminal receipt has no header"
    );
    assert!(patches[0].1.contains("❌ **Failed**"));
    assert!(patches[0].1.contains("provider exploded"));
}

#[tokio::test]
async fn failed_settle_without_tools_patches_terminal_card() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // Run fails before any tool call: the card (materialized at Running)
    // is PATCHed into the red terminal card so the user gets an
    // explanation, not just a CrossMark.
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
    drop(cards);
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    let card: serde_json::Value = serde_json::from_str(&patches[0].1).unwrap();
    assert!(
        card.get("header").is_none(),
        "terminal receipt has no header"
    );
    assert!(patches[0].1.contains("❌ **Failed**"));
    assert!(patches[0].1.contains("provider exploded"));
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

    // The error phase rides the last patch (the tool start patched first).
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 2);
    let last = &patches.last().unwrap().1;
    assert!(last.contains("⚠️ Error: rate limited"));
    assert!(last.contains("blue"), "must not settle: {patches:?}");
}

#[tokio::test]
async fn retrying_phase_shows_delay_and_reason() {
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
            &Event::Agent(AgentEvent::Retrying {
                attempt: 2,
                max_attempts: 20,
                reason: "HTTP error 429".to_string(),
                wait_ms: 34_000,
            }),
        )
        .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(
        patches[0]
            .1
            .contains("🔁 Retrying 2/20 in 34s: HTTP error 429"),
        "delay title: {}",
        patches[0].1
    );
    drop(patches);

    // Old events without a wait render without the delay.
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &Event::Agent(AgentEvent::Retrying {
                attempt: 3,
                max_attempts: 20,
                reason: "HTTP error 502".to_string(),
                wait_ms: 0,
            }),
        )
        .await;
    let patches = mock.patches.lock().await;
    let last = &patches.last().unwrap().1;
    assert!(
        last.contains("🔁 Retrying 3/20: HTTP error 502"),
        "title: {last}"
    );
    assert!(!last.contains(" in "), "no delay: {last}");
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
    assert!(patches[0].1.contains("⏰ **Timed out**"));
    drop(patches);

    // Timeout settle sends no reactions and clears the run's receipts (so a
    // later run is not misdetected as having mid-run posts).
    assert_eq!(mock.reactions_added.lock().await.len(), 0);
    assert!(!tracker.has_mid_run_posts(&sid));
}

#[tokio::test]
async fn watchdog_settles_contentless_card_as_timed_out() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // Card materialized at Running but no tool ever started → the
    // watchdog settles the placeholder card as timed out.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    assert_eq!(mock.cards.lock().await.len(), 1);
    tracker.sweep_dead_sessions(|_| false).await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("⏰ **Timed out**"));
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

// ── Standalone compact (`/compact` outside a run) ───────────────────

#[tokio::test]
async fn standalone_compact_materializes_and_settles_on_compacted() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // No run bracket: the compact materializes its own card immediately.
    tracker
        .handle_event(&adapter, &sid, "chat-1", Some("msg-1"), &compacting(true))
        .await;
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].0, "chat-1");
    assert_eq!(cards[0].2.as_deref(), Some("msg-1"));
    assert!(cards[0].1.contains("📦 Compacting context…"));
    drop(cards);

    // `Compacting { active: false }` does not settle — the outcome event
    // follows it.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(false))
        .await;
    assert!(tracker.has_state(&sid));
    assert!(mock.patches.lock().await.is_empty());

    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &compacted("Compacted 42 messages", false),
        )
        .await;
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    let terminal = &patches[0].1;
    let card: serde_json::Value = serde_json::from_str(terminal).unwrap();
    assert!(
        card.get("header").is_none(),
        "compact receipt has no header: {terminal}"
    );
    assert!(terminal.contains("✅ **Compacted** — ⏱"));
    assert!(terminal.contains("Compacted 42 messages"));
    drop(patches);

    // Settled: state gone, no reactions (the card's own send notified).
    assert!(!tracker.has_state(&sid));
    assert!(mock.reactions_added.lock().await.is_empty());
}

#[tokio::test]
async fn standalone_compact_failure_settles_with_error() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(true))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(false))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &compacted("Compaction failed: rate limited", true),
        )
        .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("❌ **Compaction failed**"));
    assert!(patches[0].1.contains("**Error**"));
    assert!(patches[0].1.contains("rate limited"));
    drop(patches);
    assert!(!tracker.has_state(&sid));
}

#[tokio::test]
async fn standalone_compact_failure_without_card_sends_explanation() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // The materialize send fails (API down) — no card this compact.
    mock.fail_send_cards.store(true, Ordering::Relaxed);
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(true))
        .await;
    assert!(mock.cards.lock().await.is_empty());
    mock.fail_send_cards.store(false, Ordering::Relaxed);

    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &compacted("Compaction failed: boom", true),
        )
        .await;
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert!(cards[0].1.contains("❌ **Compaction failed**"));
    drop(cards);
    assert!(!tracker.has_state(&sid));
}

#[tokio::test]
async fn standalone_compact_success_without_card_sends_nothing() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    mock.fail_send_cards.store(true, Ordering::Relaxed);
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(true))
        .await;
    mock.fail_send_cards.store(false, Ordering::Relaxed);

    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &compacted("Compacted 3 messages", false),
        )
        .await;
    assert!(mock.cards.lock().await.is_empty());
    assert!(!tracker.has_state(&sid));
}

#[tokio::test]
async fn mid_run_compact_only_flips_phase_and_ignores_compacted() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // A live run owns the card; an (auto) compact mid-run only flips its
    // phase and never settles it.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(true))
        .await;
    assert_eq!(mock.cards.lock().await.len(), 1);
    let patches = mock.patches.lock().await;
    assert!(patches.last().unwrap().1.contains("📦 Compacting context…"));
    drop(patches);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(false))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &compacted("Compacted 10 messages", false),
        )
        .await;

    // Still live: no terminal settle, and the run settles normally later.
    assert!(tracker.has_state(&sid));
    let patches = mock.patches.lock().await;
    assert!(!patches.iter().any(|(_, c)| c.contains("**Compacted**")));
    drop(patches);

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
    assert!(patches.last().unwrap().1.contains("✅ **Done**"));
    assert!(!tracker.has_state(&sid));
}

#[tokio::test]
async fn terminal_receipt_title_matches_live_segments() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // Live run with model/usage/failure: the settled receipt's trace
    // title must render the very same segments the live card showed
    // (only expand differs; the ~-marked ↑ estimate is live-only, real
    // usage renders identically on both).
    tracker.set_model(&sid, "k3-hs".to_string());
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_end("bash", 100, true))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &end_without_text())
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &token_usage_event(12_345, 200_000),
        )
        .await;

    let patches = mock.patches.lock().await;
    let live = patches.last().unwrap().1.clone();
    assert!(
        live.contains("🐾 0s · 💬 1 · 10.0k↓ · 2.3k↑ · ❌ 1 · k3-hs · 6%"),
        "live title: {live}"
    );
    drop(patches);

    tracker
        .freeze_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            true,
        )
        .await;
    let patches = mock.patches.lock().await;
    let terminal = &patches.last().unwrap().1;
    assert!(
        terminal.contains("🐾 0s · 💬 1 · 10.0k↓ · 2.3k↑ · ❌ 1 · k3-hs · 6%"),
        "terminal title matches live: {terminal}"
    );
}

#[tokio::test]
async fn compacting_inactive_without_state_is_noop() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(false))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &compacted("Compacted 1 messages", false),
        )
        .await;
    assert!(mock.cards.lock().await.is_empty());
    assert!(mock.patches.lock().await.is_empty());
    assert!(!tracker.has_state(&sid));
}

#[tokio::test]
async fn cancelled_compact_settles_via_generic_stopped_path() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // /stop mid-compact: the agent emits Stopped{Cancelled} (operation
    // cancelled), which settles the compact-only card through the generic
    // forwarder path as ⏹; the trailing Compacted is a no-op.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(true))
        .await;
    assert_eq!(mock.cards.lock().await.len(), 1);

    let outcome = tracker
        .handle_stopped(
            &sid,
            &StopReason::Cancelled {
                operation: Some("compaction".to_string()),
            },
            None,
        )
        .await;
    assert!(outcome.unsettled.is_none());
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("⏹ **Stopped**"));
    drop(patches);
    assert!(!tracker.has_state(&sid));
    // Cancelled runs never react.
    assert!(mock.reactions_added.lock().await.is_empty());

    // The trailing Compacted finds no state — no second settle.
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &compacted("Compaction was cancelled", true),
        )
        .await;
    assert_eq!(mock.patches.lock().await.len(), 1);
    assert!(mock.cards.lock().await.len() == 1);
}

#[tokio::test]
async fn compact_settle_leaves_receipts_for_the_next_run() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // A message posted mid-compact is the next run's trigger — its
    // receipt must survive the compact's settlement (that run still
    // needs it for the morph/split decision).
    tracker.record_receipt(&sid, "user-msg-1".into());
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &compacting(true))
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &compacted("Compacted 5 messages", false),
        )
        .await;
    assert!(tracker.has_mid_run_posts(&sid));

    // The follow-up run opens a fresh card on the same session.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    assert_eq!(mock.cards.lock().await.len(), 2);
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
fn token_footer_formats_compact() {
    assert_eq!(fmt_tokens(999), "999");
    assert_eq!(fmt_tokens(12_345), "12.3k");
    assert_eq!(fmt_tokens(200_000), "200.0k");
    assert_eq!(fmt_tokens(999_999), "1000.0k");
    assert_eq!(fmt_tokens(1_234_567), "1.2m");
    assert_eq!(fmt_tokens(200_000_000), "200.0m");
}

#[test]
fn card_json_uses_default_width_layout() {
    let card = card_json_elements(
        "blue",
        "💭 Thinking…",
        &[serde_json::json!({ "tag": "markdown", "text_size": "notation", "content": "⏱ 0s" })],
    );
    let v: serde_json::Value = serde_json::from_str(&card).unwrap();
    // No width_mode override: the card uses the platform default width
    // (600px, matching the reply card), not the narrow compact layout.
    assert!(v["config"]["width_mode"].is_null());
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
            &token_usage_event(12_345, 200_000),
        )
        .await;

    // Tokens merged into the trace panel's title (last patch — the tool
    // start patched first); the live card has no separate top stats line.
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 2);
    let last = &patches.last().unwrap().1;
    assert!(last.contains("6%"), "{last}");
}

// ── Live output estimate ────────────────────────────────────────────

fn tool_call_delta(args_delta: &str) -> Event {
    Event::Model(ModelEvent::ToolCallDelta {
        message_id: crate::types::MessageId::new(),
        tool_id: "t1".to_string(),
        tool_name: "bash".to_string(),
        arguments_delta: args_delta.to_string(),
    })
}

fn token_usage_event(total_tokens: u32, context_window: u32) -> Event {
    Event::Model(ModelEvent::TokenUsage {
        message_id: crate::types::MessageId::new(),
        prompt_tokens: 10_000,
        completion_tokens: 2_345,
        total_tokens,
        context_window,
    })
}

#[test]
fn out_estimate_ratios() {
    let mut s = ObsCardState::new(adapter_ref(&MockAdapter::new()), "chat-1", None);
    assert_eq!(s.out_estimate(), 0);
    s.out_text_bytes = 400; // ≈4 bytes/token → 100
    assert_eq!(s.out_estimate(), 100);
    s.out_json_bytes = 200; // ≈2 bytes/token → +100
    assert_eq!(s.out_estimate(), 200);

    // Folding moves the in-flight estimate into the run total.
    s.fold_out_estimate();
    assert_eq!(s.current_out_estimate(), 0);
    assert_eq!(s.out_estimate(), 200);
    // ...and further streaming adds on top.
    s.out_text_bytes = 40;
    assert_eq!(s.out_estimate(), 210);
}

#[tokio::test]
async fn thinking_only_stream_shows_estimate_on_placeholder_card() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &request())
        .await;

    // Bare placeholder while nothing has streamed (no lone timer).
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(!last.contains("⏱"), "bare placeholder: {last}");
    assert!(!last.contains('↑'), "no estimate yet: {last}");
    drop(patches);

    // Thinking-only stream: the placeholder card gains stats + estimate.
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &thinking_chunk(&"x".repeat(400)),
        )
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("⏱"), "stats joined the placeholder: {last}");
    assert!(last.contains("~100↑"), "live estimate: {last}");
    assert!(!last.contains('%'), "no real usage yet: {last}");
}

#[tokio::test]
async fn retried_request_discards_failed_attempt_estimate() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &request())
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &text_chunk(&"x".repeat(400)),
        )
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("~100↑"), "first attempt: {last}");
    drop(patches);

    // Stream fails; the retry re-fires Request and the failed attempt's
    // bytes are discarded — never folded, never double-counted.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &request())
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(!last.contains('↑'), "discarded at retry: {last}");
    drop(patches);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &text_chunk(&"y".repeat(40)))
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("~10↑"), "restarted from zero: {last}");
    assert!(!last.contains("~110↑"), "no double count: {last}");
}

#[tokio::test]
async fn tool_call_deltas_count_toward_estimate() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &request())
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &tool_call_delta(&"a".repeat(200)),
        )
        .await;

    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("~100↑"), "json estimate: {last}");
}

#[tokio::test]
async fn estimate_accumulates_across_the_run() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &request())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &text_chunk(&"y".repeat(40)))
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("~10↑"), "estimate while streaming: {last}");
    drop(patches);

    // Response ends: the estimate folds into the run total — it persists
    // through tool execution instead of vanishing.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &end_with_text("done"))
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("~10↑"), "folded at end: {last}");
    drop(patches);

    // Next request: the run total carries over, new streaming adds on
    // top, real ctx rides along.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &request())
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &token_usage_event(12_345, 200_000),
        )
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &text_chunk(&"z".repeat(40)))
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("6%"), "real usage: {last}");
    // Real total (2_345) plus the in-flight estimate (~10) ride one ↑
    // segment; the folded estimate was zeroed when real usage landed.
    assert!(last.contains("2.4k↑"), "real + in-flight estimate: {last}");
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

/// A tool-call-only turn: the model response completed without any text.
fn end_without_text() -> Event {
    Event::Model(ModelEvent::End {
        message_id: crate::types::MessageId::new(),
        content: vec![],
    })
}

#[tokio::test]
async fn fresh_card_shows_placeholder_until_first_content() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // The card materializes at Running with no tools and no text yet.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;

    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert!(
        super::IDLE_PLACEHOLDERS
            .iter()
            .any(|p| cards[0].1.contains(p)),
        "fresh card shows a placeholder: {}",
        cards[0].1
    );
    drop(cards);

    // First tool start: the placeholder is gone, the trace takes over.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap();
    assert!(
        !super::IDLE_PLACEHOLDERS.iter().any(|p| last.1.contains(p)),
        "placeholder replaced by trace: {}",
        last.1
    );
}

#[tokio::test]
async fn running_card_shows_live_trace() {
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

    // The card materialized at Running; the tool start PATCH carries the
    // trace: the tool shows as running (⏳) with its arg summary inline.
    assert_eq!(mock.cards.lock().await.len(), 1);
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0]
        .1
        .contains("⏳ **shell** · `cargo test -p kernel`"));
    drop(patches);

    // Tool end flips the line to ✅ with the elapsed time.
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &tool_end("shell", 65_000, false),
        )
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().expect("a patch after tool end");
    assert!(last
        .1
        .contains("✅ **shell** · `cargo test -p kernel` · 1m05s"));
}

#[tokio::test]
async fn model_stashed_before_running_lands_on_card() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // The forwarder sets the model at the run's first Running, before the
    // state exists — it must reach the materialized card's stats line.
    tracker.set_model(&sid, "nova-2".to_string());
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;

    let patches = mock.patches.lock().await;
    let last = &patches.last().unwrap().1;
    assert!(
        last.contains("nova-2"),
        "stats line carries the model: {last}"
    );
}

#[tokio::test]
async fn set_model_on_live_state_updates_in_place() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    // Late set (state already live): the next PATCH picks the model up.
    tracker.set_model(&sid, "k2".to_string());
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;

    let patches = mock.patches.lock().await;
    let last = &patches.last().unwrap().1;
    assert!(last.contains("k2"), "live update: {last}");
}

#[tokio::test]
async fn stats_line_omits_model_when_unknown() {
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

    let patches = mock.patches.lock().await;
    let body: serde_json::Value = serde_json::from_str(&patches.last().unwrap().1).unwrap();
    // The stats line rides the trace panel's title now (no top-level stats).
    let stats = body["body"]["elements"][0]["header"]["title"]["content"]
        .as_str()
        .unwrap();
    // No model, no tokens, no completed steps yet: the title is bare
    // elapsed (tool totals are not shown since the traffic redesign).
    assert_eq!(stats, "🐾 0s", "stats: {stats}");
    // Title carries no grey wrapper (notation size alone de-emphasizes it).
    assert_eq!(stats.matches("<font").count(), 0, "stats: {stats}");
}

#[tokio::test]
async fn running_card_trace_caps_at_ten_entries() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    for i in 0..12 {
        tracker
            .handle_event(
                &adapter,
                &sid,
                "chat-1",
                None,
                &tool_start_with_args("shell", &format!(r#"{{"command":"cmd-{i}"}}"#)),
            )
            .await;
    }

    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap();
    let body: serde_json::Value = serde_json::from_str(&last.1).unwrap();
    let elements = body["body"]["elements"].as_array().unwrap();
    // Layout (no whisper): just the live trace inside a (started-expanded)
    // collapsible panel — reading bots strip it, the human still sees it.
    assert_eq!(elements[0]["tag"], "collapsible_panel");
    let content = elements[0]["elements"][0]["content"].as_str().unwrap();
    assert!(content.contains("··· and 2 earlier entries"));
    assert!(content.contains("cmd-11"), "most recent kept");
    assert!(!content.contains("cmd-1`"), "oldest dropped");
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
    // No chunks at all (e.g. lost on the bus): the completed text still
    // lands in the trace as a narration (full text is authoritative).
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
async fn completed_text_renders_flattened_head_capped_narration() {
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
    // Narration: flattened, head-capped with an ellipsis — and shown
    // exactly once (the whisper cleared instead of duplicating it).
    assert!(last.contains("💬 line one line two"), "narration: {last}");
    assert!(last.contains('…'), "head cap: {last}");
    assert_eq!(last.matches("💬 line one").count(), 1, "shown once: {last}");
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
    assert!(terminal.contains("✅ **Done**"));
    assert!(!terminal.contains("💬"), "terminal drops the whisper");
    // …while the stats line survives on the receipt (tool totals are no
    // longer titled since the traffic redesign).
    assert!(terminal.contains("⏱ 0s"), "stats line: {terminal}");
    assert!(terminal.contains("collapsible_panel"), "{terminal}");
    assert!(terminal.contains("⏳ **bash**"), "{terminal}");
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

#[test]
fn whisper_snippet_sanitizes_markdown_structural_chars() {
    // 未闭合的反引号/星号/尖括号会撑破卡片 markdown（整元素纯文本回退），
    // whisper 渲染前全角化（与 reply::md_safe 同一约定）。
    let out = whisper_snippet("a `code` **bold** <tag> 还有 `未闭合");
    assert!(out.contains("｀code｀"), "{out}");
    assert!(out.contains("＊＊bold＊＊"), "{out}");
    assert!(out.contains("＜tag＞"), "{out}");
    assert!(out.contains("｀未闭合"), "{out}");
    assert!(!out.contains('`'), "{out}");
    assert!(!out.contains('*'), "{out}");
}

#[tokio::test]
async fn trace_inline_arg_summary_is_capped() {
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

    let patches = mock.patches.lock().await;
    let body: serde_json::Value = serde_json::from_str(&patches.last().unwrap().1).unwrap();
    // Collect text from top-level elements and any nested collapsible panel
    // (the live trace now lives inside one).
    fn collect(e: &serde_json::Value, out: &mut Vec<String>) {
        if let Some(c) = e["content"].as_str() {
            out.push(c.to_string());
        }
        if let Some(inner) = e["elements"].as_array() {
            for i in inner {
                collect(i, out);
            }
        }
    }
    let mut parts = Vec::new();
    for e in body["body"]["elements"].as_array().unwrap() {
        collect(e, &mut parts);
    }
    let content = parts.join("\n");
    // Long args stay on one truncated inline line (ARG_SUMMARY_MAX_CHARS + ellipsis).
    let tool_line = content
        .lines()
        .find(|l| l.starts_with('⏳'))
        .expect("trace tool line");
    assert!(tool_line.ends_with("…`"), "line: {tool_line}");
    assert!(tool_line.chars().count() <= 140, "line: {tool_line}");
    assert!(!content.lines().any(|l| l.starts_with('↳')));
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
    // Streamed chunks feed the live whisper (tail kept).
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &text_chunk(&"y".repeat(300)),
        )
        .await;

    let patches = mock.patches.lock().await;
    let body: serde_json::Value = serde_json::from_str(&patches.last().unwrap().1).unwrap();
    // Live content rides inside the collapsible panel (humans see it
    // expanded; reading bots strip it) — the whisper tail is its first
    // line, the trace lines follow.
    let panel = &body["body"]["elements"][0];
    assert_eq!(panel["tag"], "collapsible_panel");
    let content = panel["elements"][0]["content"].as_str().unwrap();
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

    // 隐私不变量：whisper/trace/stats 全部收进面板——顶层没有 markdown
    // 元素（读取路径只取顶层 markdown，面板整体被剥离，其他 bot 读不到
    // 任何暂态内容）。Stop 按钮行（column_set）是唯一例外：只含空
    // markdown + 按钮，无暂态内容（sid 是标识符非凭证——`/stop` 命令
    // 本就无门槛，与 mailbox 卡按钮携带 sid 的既有模式相同）。
    let top = body["body"]["elements"].as_array().unwrap();
    assert!(
        top.iter()
            .all(|e| e["tag"] == "collapsible_panel" || e["tag"] == "column_set"),
        "live content must not leak into top-level markdown: {top:?}"
    );
}

// ── Morph settlement (one message per run) ──────────────────────────

fn reply_with(text: &str) -> crate::channels::reply::FinalReply {
    let mut buf = crate::channels::reply::RunReplyBuffer::new();
    buf.record_model_end("intermediate thought");
    buf.record_tool_start("t1", "shell", Some(r#"{"command":"cargo test"}"#));
    buf.record_tool_end("t1", 1200, false);
    buf.record_model_end(text);
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

    // The status-card send failed at Running → no card to morph; the
    // reply falls back to a new message.
    mock.fail_send_cards.store(true, Ordering::Relaxed);
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    assert_eq!(mock.cards.lock().await.len(), 0);
    mock.fail_send_cards.store(false, Ordering::Relaxed);

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

    // The status-card send failed at Running → no card to settle; the
    // failure must still be explained as a new notice card.
    mock.fail_send_cards.store(true, Ordering::Relaxed);
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    mock.fail_send_cards.store(false, Ordering::Relaxed);

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
async fn first_text_chunk_patches_card_without_tools() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", Some("msg-1"), &running())
        .await;
    assert_eq!(
        mock.cards.lock().await.len(),
        1,
        "Running materializes the card"
    );

    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            Some("msg-1"),
            &text_chunk("Hello"),
        )
        .await;
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1, "first text patches the card");
    assert!(patches[0].1.contains("💬 Hello"));
    assert!(super::TYPING_TITLES
        .iter()
        .any(|t| patches[0].1.contains(t)));
    drop(patches);

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
async fn thinking_chunk_keeps_thinking_title_without_leaking_content() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &thinking_chunk("hmm"))
        .await;
    // The thinking chunk patches the (already materialized) card with a
    // thinking title — reasoning models can think for a long time.
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(super::THINKING_TITLES
        .iter()
        .any(|t| patches[0].1.contains(t)));
    // Thinking content itself is never rendered (internal reasoning).
    assert!(!patches[0].1.contains("hmm"));
    assert!(!patches[0].1.contains("💬"));
}

#[tokio::test]
async fn tool_start_updates_are_throttled_uniformly() {
    // Huge patch interval: no throttled PATCH may fire — tool starts mutate
    // state only; the tool title rides the next regular render (the card
    // materialized at Running shows the placeholder until then).
    let tracker = ObsTracker::with_patch_interval(Duration::from_hours(1));
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "Running materializes");
    assert!(
        super::IDLE_PLACEHOLDERS
            .iter()
            .any(|p| cards[0].1.contains(p)),
        "creation render shows the placeholder: {}",
        cards[0].1
    );
    drop(cards);

    // Tool starts inside the throttle window do NOT PATCH.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("read"))
        .await;
    assert_eq!(mock.patches.lock().await.len(), 0, "uniform throttling");
}

#[test]
fn mid_run_posts_detection_uses_receipts() {
    let tracker = ObsTracker::new();
    let sid = sid();
    // Receipts hold only messages posted while the agent was running (run
    // triggers are never recorded), so any receipt means mid-run posts.
    assert!(!tracker.has_mid_run_posts(&sid));
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
    assert!(returned.unsettled.is_some(), "no state → reply handed back");
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
    assert!(
        returned.unsettled.is_some(),
        "settle send failed → reply handed back"
    );
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

#[test]
fn random_title_picks_from_the_list() {
    for _ in 0..50 {
        let title = super::random_title(super::THINKING_TITLES);
        assert!(super::THINKING_TITLES.contains(&title));
    }
    // Over 100 draws we are effectively guaranteed to see more than one
    // distinct title ((1/6)^100 chance of flaking).
    let distinct: std::collections::HashSet<_> = (0..100)
        .map(|_| super::random_title(super::THINKING_TITLES))
        .collect();
    assert!(distinct.len() > 1);
}

#[tokio::test]
async fn stats_line_shows_steps_after_first_model_end() {
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

    // No completed model response yet (the tool-start patch): no steps
    // (tool totals are not shown since the traffic redesign).
    let patches = mock.patches.lock().await;
    let first = patches[0].1.clone();
    assert!(!first.contains("💬"), "no steps yet: {first}");
    drop(patches);

    // First completed model response: the stats line gains a step.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &end_with_text("done one"))
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("💬 1"), "one step: {last}");
    drop(patches);

    // The in-progress text of the next turn is not a step yet.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &text_chunk("drafting"))
        .await;
    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("💬 1"), "still one step: {last}");
}

#[tokio::test]
async fn stats_line_counts_textless_model_end_as_step() {
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
    // Tool-call-only turn: the model response carried no text — the step
    // count tracks model ends, not completed texts.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &end_without_text())
        .await;

    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(
        last.contains("💬 1"),
        "tool-call-only turn is a step: {last}"
    );
}

// ── Settle reaction (completion signal for silent card settles) ─────

/// Drive a run far enough to materialize the status card (Running sends
/// it; the tool start gives the card some content).
async fn drive_materialized_run(tracker: &ObsTracker, mock: &Arc<MockAdapter>, sid: &SessionId) {
    let adapter = adapter_ref(mock);
    tracker
        .handle_event(&adapter, sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, sid, "chat-1", None, &tool_start("bash"))
        .await;
}

#[tokio::test]
async fn settle_morph_reacts_done_on_latest_user_message() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_user_msg(&sid, "user-msg-1".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("the final answer")),
        )
        .await;

    // The card morphed in place (silent) — the reaction is the only
    // completion signal.
    assert_eq!(mock.patches.lock().await.len(), 1);
    let reactions = mock.reactions_added.lock().await;
    assert_eq!(
        reactions.as_slice(),
        [("user-msg-1".to_string(), "DONE".to_string())]
    );
}

#[tokio::test]
async fn settle_failed_reacts_cross_mark() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_user_msg(&sid, "user-msg-1".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Failed {
                error: "provider exploded".to_string(),
            },
            Some(reply_with("partial answer")),
        )
        .await;

    let reactions = mock.reactions_added.lock().await;
    assert_eq!(
        reactions.as_slice(),
        [("user-msg-1".to_string(), "CrossMark".to_string())]
    );
}

#[tokio::test]
async fn settle_timeout_reacts_cross_mark() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_user_msg(&sid, "user-msg-1".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker
        .handle_timeout(&sid, Some(reply_with("partial output")))
        .await;

    let reactions = mock.reactions_added.lock().await;
    assert_eq!(
        reactions.as_slice(),
        [("user-msg-1".to_string(), "CrossMark".to_string())]
    );
}

#[tokio::test]
async fn settle_cancelled_sends_no_reaction() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_user_msg(&sid, "user-msg-1".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Cancelled {
                operation: Some("streaming".to_string()),
            },
            Some(reply_with("partial answer")),
        )
        .await;

    // The user stopped the run themselves — the card still morphs, but no
    // completion signal is needed.
    assert_eq!(mock.patches.lock().await.len(), 1);
    assert!(mock.reactions_added.lock().await.is_empty());
}

#[tokio::test]
async fn settle_with_mid_run_posts_sends_no_reaction() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_user_msg(&sid, "user-msg-1".into());
    // A mid-run post means the reply lands as a NEW message below it —
    // that message notifies by itself.
    tracker.record_receipt(&sid, "mid-run".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
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
}

// ── Mid-run split freeze ────────────────────────────────────────────

#[tokio::test]
async fn freeze_stopped_patches_terminal_receipt_without_trace() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_receipt(&sid, "mid-run".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
    // keep_trace = false: the reply message carries the trace, so the
    // frozen card is a stats-only receipt.
    tracker
        .freeze_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            false,
        )
        .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1, "frozen in place");
    let card: serde_json::Value = serde_json::from_str(&patches[0].1).unwrap();
    assert!(
        card.get("header").is_none(),
        "terminal receipt has no header"
    );
    assert!(patches[0].1.contains("✅ **Done**"));
    assert!(
        !patches[0].1.contains("collapsible_panel"),
        "stats-only receipt: {patches:?}"
    );
    drop(patches);
    // Receipts cleared; the reply message notifies by itself — no reaction.
    assert!(!tracker.has_mid_run_posts(&sid));
    assert!(mock.reactions_added.lock().await.is_empty());
}

#[tokio::test]
async fn freeze_stopped_keep_trace_true_keeps_panel() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker
        .freeze_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            true,
        )
        .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("✅ **Done**"));
    assert!(
        patches[0].1.contains("collapsible_panel") && patches[0].1.contains("🐾"),
        "no reply to carry the trace — the card keeps it: {patches:?}"
    );
}

#[tokio::test]
async fn freeze_stopped_failed_without_card_sends_terminal_card() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // The status card never materialized (send failed).
    mock.fail_send_cards.store(true, Ordering::Relaxed);
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    mock.fail_send_cards.store(false, Ordering::Relaxed);
    // A tool ran during the run — the trace has an entry to show.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;

    // A failure still gets its explanation as a NEW terminal card, trace
    // included regardless of keep_trace.
    tracker
        .freeze_stopped(
            &sid,
            &StopReason::Failed {
                error: "boom".to_string(),
            },
            false,
        )
        .await;

    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "terminal card sent as a new message");
    let card: serde_json::Value = serde_json::from_str(&cards[0].1).unwrap();
    assert!(
        card.get("header").is_none(),
        "terminal receipt has no header"
    );
    assert!(cards[0].1.contains("❌ **Failed**"));
    assert!(cards[0].1.contains("collapsible_panel"));
    assert!(mock.patches.lock().await.is_empty());
}

#[tokio::test]
async fn freeze_stopped_completed_without_card_sends_nothing() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    mock.fail_send_cards.store(true, Ordering::Relaxed);
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;

    tracker
        .freeze_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            false,
        )
        .await;

    assert!(mock.cards.lock().await.is_empty());
    assert!(mock.patches.lock().await.is_empty());
}

#[tokio::test]
async fn freeze_timeout_patches_terminal_card_and_clears_receipts() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_receipt(&sid, "mid-run".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker.freeze_timeout(&sid, true).await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("⏰ **Timed out**"));
    assert!(patches[0].1.contains("collapsible_panel"));
    drop(patches);
    assert!(!tracker.has_mid_run_posts(&sid));
    assert!(mock.reactions_added.lock().await.is_empty());
}

#[tokio::test]
async fn settle_without_recorded_user_msg_sends_no_reaction() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("the final answer")),
        )
        .await;

    assert_eq!(mock.patches.lock().await.len(), 1);
    assert!(mock.reactions_added.lock().await.is_empty());
}

#[tokio::test]
async fn settle_reaction_targets_latest_recorded_message() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_user_msg(&sid, "user-msg-1".into());
    tracker.record_user_msg(&sid, "user-msg-2".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("the final answer")),
        )
        .await;

    let reactions = mock.reactions_added.lock().await;
    assert_eq!(
        reactions.as_slice(),
        [("user-msg-2".to_string(), "DONE".to_string())]
    );
}

#[tokio::test]
async fn settle_reaction_skipped_when_card_never_materialized() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_user_msg(&sid, "user-msg-1".into());
    let adapter = adapter_ref(&mock);
    // The status-card send failed at Running → no card to morph.
    mock.fail_send_cards.store(true, Ordering::Relaxed);
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    mock.fail_send_cards.store(false, Ordering::Relaxed);

    // No chunks: the settle sends the reply as a NEW card
    // message (which notifies), so no reaction.
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("plain answer")),
        )
        .await;

    assert_eq!(mock.cards.lock().await.len(), 1);
    assert!(mock.patches.lock().await.is_empty());
    assert!(mock.reactions_added.lock().await.is_empty());
}

#[tokio::test]
async fn repeated_settle_on_same_message_replaces_previous_reaction() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_user_msg(&sid, "user-msg-1".into());

    // Two consecutive runs settle on the same silent session.
    for _ in 0..2 {
        drive_materialized_run(&tracker, &mock, &sid).await;
        tracker
            .handle_stopped(
                &sid,
                &StopReason::Completed {
                    finish_reason: None,
                },
                Some(reply_with("the final answer")),
            )
            .await;
    }

    // Both settles added DONE on the same message; the second one deleted
    // the first reaction beforehand so the platform re-surfaces the signal
    // instead of deduplicating it.
    let added = mock.reactions_added.lock().await;
    assert_eq!(added.len(), 2);
    assert!(
        added
            .iter()
            .all(|(msg, emoji)| msg == "user-msg-1" && emoji == "DONE"),
        "two DONE adds on the trigger message: {added:?}"
    );
    let removed = mock.reactions_removed.lock().await;
    assert_eq!(removed.len(), 1, "previous reaction deleted once");
    assert_eq!(removed[0].0, "user-msg-1");
    assert!(removed[0].1.starts_with("reaction-"));
}

#[tokio::test]
async fn settle_reaction_on_fresh_message_deletes_nothing() {
    let tracker = ObsTracker::new();
    let mock = MockAdapter::new();
    let sid = sid();

    tracker.record_user_msg(&sid, "user-msg-1".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("answer one")),
        )
        .await;

    // A new user message moves the target; the next settle adds a fresh
    // reaction without touching the old one.
    tracker.record_user_msg(&sid, "user-msg-2".into());
    drive_materialized_run(&tracker, &mock, &sid).await;
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("answer two")),
        )
        .await;

    assert!(mock.reactions_removed.lock().await.is_empty());
    let added = mock.reactions_added.lock().await;
    assert_eq!(added.len(), 2);
    assert_eq!(added[0].0, "user-msg-1");
    assert_eq!(added[1].0, "user-msg-2");
}

#[tokio::test]
async fn live_card_carries_stop_button_terminal_card_drops_it() {
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    // Fresh card (placeholder state): the Stop button rides from the start.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    let cards = mock.cards.lock().await;
    let fresh = &cards.last().unwrap().1;
    assert!(
        fresh.contains("\"act_stop\""),
        "fresh card stop button: {fresh}"
    );
    assert!(
        fresh.contains(&format!("\"sid\":\"{}\"", sid.0)),
        "button carries the session id: {fresh}"
    );
    drop(cards);

    // Trace state (tool running): button still there.
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &tool_start("bash"))
        .await;
    let patches = mock.patches.lock().await;
    let live = &patches.last().unwrap().1;
    assert!(
        live.contains("\"act_stop\""),
        "live card stop button: {live}"
    );
    drop(patches);

    // Settlement morphs the card into the terminal receipt: no button.
    tracker
        .handle_stopped(
            &sid,
            &StopReason::Completed {
                finish_reason: None,
            },
            Some(reply_with("done")),
        )
        .await;
    let patches = mock.patches.lock().await;
    let terminal = &patches.last().unwrap().1;
    assert!(
        !terminal.contains("act_stop"),
        "terminal card drops the button: {terminal}"
    );
}

#[tokio::test]
async fn usage_event_before_end_fold_never_double_counts() {
    // Production order: TokenUsage arrives inside the stream, BEFORE the
    // response's End (provider usage chunks ride the stream tail). Real
    // usage zeroes the whole estimate, so the End fold that follows
    // adds 0 — the ↑ segment never re-folds the same response's estimate
    // on top of its true completion count.
    let tracker = ObsTracker::with_patch_interval(Duration::ZERO);
    let mock = MockAdapter::new();
    let sid = sid();
    let adapter = adapter_ref(&mock);

    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &running())
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &request())
        .await;
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &text_chunk(&"x".repeat(400)),
        )
        .await;
    // True usage (completion 2_345) lands mid-stream, then End folds.
    tracker
        .handle_event(
            &adapter,
            &sid,
            "chat-1",
            None,
            &token_usage_event(12_345, 200_000),
        )
        .await;
    tracker
        .handle_event(&adapter, &sid, "chat-1", None, &end_with_text("done"))
        .await;

    let patches = mock.patches.lock().await;
    let last = patches.last().unwrap().1.clone();
    assert!(last.contains("10.0k↓"), "prompt total: {last}");
    assert!(
        last.contains("2.3k↑"),
        "true completion only, no estimate re-fold: {last}"
    );
    assert!(
        !last.contains('~'),
        "no estimate marker once real usage landed: {last}"
    );
}
