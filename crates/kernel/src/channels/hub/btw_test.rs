//! Tests for the `/btw` channel side question (hub/btw.rs).
//!
//! The test kernel is built without `kernel.start()`: its conductor
//! never consumes the `Btw` input, so the event stream is fully
//! test-driven via `publish_btw_stream` — no daemon, no model.

use std::sync::Arc;

use super::btw::*;
use super::command::parse_channel_command;
use super::handlers::handle_incoming_message;
use super::*;
use crate::channels::store::SqliteChannelStore;
use crate::channels::{ChannelConfig, ChannelMessage, ChannelStore, MappingKind, PlatformConfig};
use crate::event::{BtwEndReason, BtwEvent, Envelope, Event};
use crate::storage::migrations::run_migrations;
use crate::types::{BtwId, ContentBlock, SessionId};
use sqlx::sqlite::SqlitePoolOptions;
use tokio_util::sync::CancellationToken;

/// Card-capable mock: records cards, patches, and text messages;
/// `card_fail` makes only `send_card` fail (text fallback path).
struct BtwMockAdapter {
    cards: tokio::sync::Mutex<Vec<(String, Option<String>)>>,
    patches: tokio::sync::Mutex<Vec<String>>,
    outgoing: tokio::sync::Mutex<Vec<String>>,
    card_fail: std::sync::atomic::AtomicBool,
    /// Patch calls fail while the call count is below this value
    /// (retry-path testing).
    patch_fail_until: std::sync::atomic::AtomicUsize,
    patch_calls: std::sync::atomic::AtomicUsize,
    card_ok: bool,
}

impl BtwMockAdapter {
    fn card() -> Self {
        Self {
            cards: tokio::sync::Mutex::new(Vec::new()),
            patches: tokio::sync::Mutex::new(Vec::new()),
            outgoing: tokio::sync::Mutex::new(Vec::new()),
            card_fail: std::sync::atomic::AtomicBool::new(false),
            patch_fail_until: std::sync::atomic::AtomicUsize::new(0),
            patch_calls: std::sync::atomic::AtomicUsize::new(0),
            card_ok: true,
        }
    }

    fn text_only() -> Self {
        Self {
            card_ok: false,
            ..Self::card()
        }
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for BtwMockAdapter {
    async fn run_receiver(
        &self,
        _incoming: tokio::sync::mpsc::Sender<ChannelEvent>,
        cancel: CancellationToken,
    ) -> std::result::Result<(), crate::channels::ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        _chat: &str,
        blocks: Vec<ContentBlock>,
        _reply: Option<&str>,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        for b in &blocks {
            if let ContentBlock::Text { text } = b {
                self.outgoing.lock().await.push(text.clone());
            }
        }
        Ok(Some("msg-1".into()))
    }

    async fn send_card(
        &self,
        chat: &str,
        _card: &str,
        reply: Option<&str>,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        if self.card_fail.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::channels::ChannelError::Platform(
                "mock card fail".into(),
            ));
        }
        self.cards
            .lock()
            .await
            .push((chat.into(), reply.map(str::to_string)));
        Ok(Some("card-1".into()))
    }

    async fn update_card(
        &self,
        _id: &str,
        card: &str,
    ) -> std::result::Result<(), crate::channels::ChannelError> {
        let n = self
            .patch_calls
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n < self
            .patch_fail_until
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            return Err(crate::channels::ChannelError::Platform(
                "mock patch fail".into(),
            ));
        }
        self.patches.lock().await.push(card.to_string());
        Ok(())
    }

    fn supports_status_card(&self) -> bool {
        self.card_ok
    }
}

async fn test_store() -> Arc<dyn ChannelStore> {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    Arc::new(SqliteChannelStore::new(pool))
}

async fn test_kernel() -> Arc<crate::kernel::Kernel> {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    // The TempDir leaks with the kernel's lifetime — fine for tests.
    crate::build_kernel(&kconfig, false).await.unwrap()
}

/// Create a real session so the arm's session-existence check passes.
async fn btw_session(kernel: &crate::kernel::Kernel) -> SessionId {
    kernel
        .create_session(crate::kernel::CreateSessionInput {
            project_id: None,
            working_dir: None,
            auto_approve_level: None,
            tool_blocklist: vec![],
            model_key: None,
            context_window: None,
        })
        .await
        .unwrap()
}

fn btw_msg(text: &str) -> ChannelMessage {
    ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_1".to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some(text.to_string()),
        content: vec![],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: false,
        create_time: None,
        doc_comment: None,
    }
}

fn mock_config() -> ChannelConfig {
    ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        ..Default::default()
    }
}

/// Publish a Start → Delta → Done btw stream for `sid` on the kernel bus.
fn publish_btw_stream(
    bus: &crate::comms::EventBus,
    sid: &SessionId,
    answer: &str,
    reason: BtwEndReason,
) {
    let rid = BtwId::from("btw_test1");
    let fire = |ev: BtwEvent| {
        bus.publish(sid.clone(), Envelope::new(sid.clone(), Event::Btw(ev)))
            .unwrap();
    };
    fire(BtwEvent::Start {
        request_id: rid.clone(),
    });
    fire(BtwEvent::Delta {
        request_id: rid.clone(),
        text: answer.to_string(),
    });
    fire(BtwEvent::Done {
        request_id: rid,
        reason,
    });
}

/// Poll `what` until it returns `Some` or the deadline passes.
async fn wait_for<T>(mut what: impl FnMut() -> Option<T>) -> Option<T> {
    for _ in 0..100 {
        if let Some(v) = what() {
            return Some(v);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    None
}

#[test]
fn parse_btw_command() {
    assert!(matches!(
        parse_channel_command(Some("/btw what was that variable?")),
        ChannelCommand::Btw(q) if q == "what was that variable?"
    ));
    assert!(matches!(
        parse_channel_command(Some("/btw")),
        ChannelCommand::InvalidBtwCommand
    ));
    // Whitespace-only question is not a question.
    assert!(matches!(
        parse_channel_command(Some("/btw   ")),
        ChannelCommand::InvalidBtwCommand
    ));
    // Not command-shaped: bare word passes through as a normal message.
    assert!(matches!(
        parse_channel_command(Some("btw: what?")),
        ChannelCommand::None
    ));
}

#[test]
fn btw_card_live_phases_render() {
    let pending = btw_card("什么是 GEE？", "", BtwPhase::Pending);
    assert!(pending.contains("wathet"), "{pending}");
    assert!(pending.contains("💭 btw · 什么是 GEE？"), "{pending}");
    assert!(pending.contains("Thinking"), "{pending}");

    let streaming = btw_card("什么是 GEE？", "答案是 42", BtwPhase::Streaming);
    assert!(streaming.contains("答案是 42"), "{streaming}");
    assert!(streaming.contains("Answering"), "{streaming}");
}

#[test]
fn btw_card_final_renders() {
    let done = btw_card_final("q", "答案", &BtwEnd::Reason(BtwEndReason::Stop));
    assert!(done.contains("答案"), "{done}");
    assert!(
        done.contains("side question · never enters history"),
        "{done}"
    );

    // Pure tool_use: fallback body replaces the empty answer.
    let tool_use = btw_card_final("q", "", &BtwEnd::Reason(BtwEndReason::ToolUse));
    assert!(tool_use.contains("needs a real prompt"), "{tool_use}");

    // Error reason renders the red note.
    let err = btw_card_final("q", "", &BtwEnd::Reason(BtwEndReason::Error("boom".into())));
    assert!(err.contains("<font color='red'>"), "{err}");
    assert!(err.contains("boom"), "{err}");

    // Long questions truncate in the header.
    let long_q = "一".repeat(100);
    let card = btw_card(&long_q, "", BtwPhase::Pending);
    assert!(!card.contains(&"一".repeat(50)), "{card}");
}

#[tokio::test]
async fn btw_streams_to_card_and_freezes() {
    let store = test_store().await;
    let kernel = test_kernel().await;
    let sid = btw_session(&kernel).await;
    store
        .save_mapping("mock", "oc_1", &sid, "oc_1", None, MappingKind::Normal)
        .await
        .unwrap();

    let mock = Arc::new(BtwMockAdapter::card());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let msg = btw_msg("/btw 刚才那个变量叫什么？");

    let reply = handle_incoming_message(
        "mock",
        &mock_config(),
        &store,
        kernel.clone(),
        msg,
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    // The card is the reply — no command text.
    assert!(reply.is_none(), "{reply:?}");
    // Initial pending card landed (sent by the spawned delivery task).
    wait_for(|| (!mock.cards.try_lock().ok()?.is_empty()).then_some(()))
        .await
        .expect("initial btw card");

    publish_btw_stream(
        kernel.event_bus().as_ref().unwrap(),
        &sid,
        "那个变量叫 `answer`。",
        BtwEndReason::Stop,
    );

    let final_patch = wait_for(|| {
        let patches = mock.patches.try_lock().ok()?;
        patches
            .last()
            .filter(|p| p.contains("side question · never enters history"))
            .cloned()
    })
    .await
    .expect("final btw card patch");
    assert!(final_patch.contains("那个变量叫"), "{final_patch}");
}

#[tokio::test]
async fn btw_without_session_replies_error() {
    let store = test_store().await;
    let kernel = test_kernel().await;
    let mock = Arc::new(BtwMockAdapter::card());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());

    let reply = handle_incoming_message(
        "mock",
        &mock_config(),
        &store,
        kernel,
        btw_msg("/btw anything"),
        &obs,
        &adapter,
    )
    .await
    .unwrap()
    .expect("error reply");
    assert!(reply.contains("No session here yet"), "{reply}");
    assert!(mock.cards.lock().await.is_empty());
}

#[tokio::test]
async fn btw_text_mode_sends_single_message() {
    let store = test_store().await;
    let kernel = test_kernel().await;
    let sid = btw_session(&kernel).await;
    store
        .save_mapping("mock", "oc_1", &sid, "oc_1", None, MappingKind::Normal)
        .await
        .unwrap();

    let mock = Arc::new(BtwMockAdapter::text_only());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());

    let reply = handle_incoming_message(
        "mock",
        &mock_config(),
        &store,
        kernel.clone(),
        btw_msg("/btw 问题"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(reply.is_none(), "{reply:?}");
    assert!(mock.cards.lock().await.is_empty(), "no cards in text mode");

    publish_btw_stream(
        kernel.event_bus().as_ref().unwrap(),
        &sid,
        "文本答案",
        BtwEndReason::Stop,
    );

    let text = wait_for(|| {
        let out = mock.outgoing.try_lock().ok()?;
        out.first().cloned()
    })
    .await
    .expect("text reply");
    assert!(text.contains("文本答案"), "{text}");
    assert!(text.contains("💭 btw · 问题"), "{text}");
    assert!(text.contains("never enters history"), "{text}");
    // Text platforms must not receive card markup.
    assert!(!text.contains("<font"), "{text}");
}

#[tokio::test]
async fn btw_card_send_failure_falls_back_to_text() {
    let store = test_store().await;
    let kernel = test_kernel().await;
    let sid = btw_session(&kernel).await;
    store
        .save_mapping("mock", "oc_1", &sid, "oc_1", None, MappingKind::Normal)
        .await
        .unwrap();

    let mock = Arc::new(BtwMockAdapter::card());
    mock.card_fail
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());

    let reply = handle_incoming_message(
        "mock",
        &mock_config(),
        &store,
        kernel.clone(),
        btw_msg("/btw 问题"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(reply.is_none(), "{reply:?}");

    publish_btw_stream(
        kernel.event_bus().as_ref().unwrap(),
        &sid,
        "降级文本答案",
        BtwEndReason::Stop,
    );

    let text = wait_for(|| {
        let out = mock.outgoing.try_lock().ok()?;
        out.first().cloned()
    })
    .await
    .expect("fallback text reply");
    assert!(text.contains("降级文本答案"), "{text}");
}

#[tokio::test]
async fn btw_stale_mapping_replies_error() {
    let store = test_store().await;
    let kernel = test_kernel().await;
    // Routing row pointing at a session the kernel doesn't have.
    let ghost = SessionId::from("sess_ghost");
    store
        .save_mapping("mock", "oc_1", &ghost, "oc_1", None, MappingKind::Normal)
        .await
        .unwrap();

    let mock = Arc::new(BtwMockAdapter::card());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());

    let reply = handle_incoming_message(
        "mock",
        &mock_config(),
        &store,
        kernel,
        btw_msg("/btw anything"),
        &obs,
        &adapter,
    )
    .await
    .unwrap()
    .expect("error reply");
    assert!(reply.contains("No session here yet"), "{reply}");
}

#[tokio::test]
async fn btw_ignores_other_request_ids() {
    let store = test_store().await;
    let kernel = test_kernel().await;
    let sid = btw_session(&kernel).await;
    store
        .save_mapping("mock", "oc_1", &sid, "oc_1", None, MappingKind::Normal)
        .await
        .unwrap();

    let mock = Arc::new(BtwMockAdapter::card());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    handle_incoming_message(
        "mock",
        &mock_config(),
        &store,
        kernel.clone(),
        btw_msg("/btw 问题"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();

    // Two interleaved streams (e.g. a GUI-initiated btw replaced by ours):
    // only the pinned rid may render.
    let bus = kernel.event_bus().unwrap();
    let rid_a = BtwId::from("btw_a");
    let rid_b = BtwId::from("btw_b");
    let fire = |ev: BtwEvent| {
        bus.publish(sid.clone(), Envelope::new(sid.clone(), Event::Btw(ev)))
            .unwrap();
    };
    fire(BtwEvent::Start {
        request_id: rid_a.clone(),
    });
    fire(BtwEvent::Delta {
        request_id: rid_b.clone(),
        text: "GUI的答案，不该出现".to_string(),
    });
    fire(BtwEvent::Delta {
        request_id: rid_a.clone(),
        text: "飞书的答案".to_string(),
    });
    fire(BtwEvent::Done {
        request_id: rid_b,
        reason: BtwEndReason::Replaced,
    });
    fire(BtwEvent::Done {
        request_id: rid_a,
        reason: BtwEndReason::Stop,
    });

    let final_patch = wait_for(|| {
        let patches = mock.patches.try_lock().ok()?;
        patches
            .last()
            .filter(|p| p.contains("side question · never enters history"))
            .cloned()
    })
    .await
    .expect("final btw card patch");
    assert!(final_patch.contains("飞书的答案"), "{final_patch}");
    assert!(
        !final_patch.contains("GUI的答案，不该出现"),
        "{final_patch}"
    );
}

#[tokio::test]
async fn btw_final_patch_retries_until_land() {
    let store = test_store().await;
    let kernel = test_kernel().await;
    let sid = btw_session(&kernel).await;
    store
        .save_mapping("mock", "oc_1", &sid, "oc_1", None, MappingKind::Normal)
        .await
        .unwrap();

    let mock = Arc::new(BtwMockAdapter::card());
    // The first two patch attempts fail; the terminal freeze must retry
    // instead of leaving a spinner card behind.
    mock.patch_fail_until
        .store(2, std::sync::atomic::Ordering::SeqCst);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    handle_incoming_message(
        "mock",
        &mock_config(),
        &store,
        kernel.clone(),
        btw_msg("/btw 问题"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();

    publish_btw_stream(
        kernel.event_bus().as_ref().unwrap(),
        &sid,
        "重试后的答案",
        BtwEndReason::Stop,
    );

    let final_patch = wait_for(|| {
        let patches = mock.patches.try_lock().ok()?;
        patches
            .last()
            .filter(|p| p.contains("side question · never enters history"))
            .cloned()
    })
    .await
    .expect("final btw card patch after retries");
    assert!(final_patch.contains("重试后的答案"), "{final_patch}");
}

#[test]
fn btw_card_balances_unclosed_fence() {
    // A truncated/cancelled answer ending inside a ``` fence would
    // degrade the whole card element; the renderer must close it.
    let card = btw_card_final(
        "q",
        "说明\n```rust\nlet x = 1;",
        &BtwEnd::Reason(BtwEndReason::Cancelled),
    );
    let body = card
        .split("let x = 1;")
        .nth(1)
        .expect("answer body in card");
    assert!(body.contains("```"), "closing fence in {body}");
}

#[tokio::test]
async fn btw_tick_throttles_streaming_patches() {
    let store = test_store().await;
    let kernel = test_kernel().await;
    let sid = btw_session(&kernel).await;
    store
        .save_mapping("mock", "oc_1", &sid, "oc_1", None, MappingKind::Normal)
        .await
        .unwrap();

    let mock = Arc::new(BtwMockAdapter::card());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    handle_incoming_message(
        "mock",
        &mock_config(),
        &store,
        kernel.clone(),
        btw_msg("/btw 问题"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();

    // A delta with no Done: the tick arm must flush a streaming patch
    // (the tick arm never runs in the Done-immediately-after-Delta cases).
    let bus = kernel.event_bus().unwrap();
    bus.publish(
        sid.clone(),
        Envelope::new(
            sid.clone(),
            Event::Btw(BtwEvent::Start {
                request_id: BtwId::from("btw_tick"),
            }),
        ),
    )
    .unwrap();
    bus.publish(
        sid.clone(),
        Envelope::new(
            sid.clone(),
            Event::Btw(BtwEvent::Delta {
                request_id: BtwId::from("btw_tick"),
                text: "流式中的一段".to_string(),
            }),
        ),
    )
    .unwrap();

    let streaming_patch = wait_for(|| {
        let patches = mock.patches.try_lock().ok()?;
        patches
            .iter()
            .find(|p| p.contains("流式中的一段") && p.contains("Answering"))
            .cloned()
    })
    .await
    .expect("throttled streaming patch");
    assert!(
        !streaming_patch.contains("side question · never enters history"),
        "not settled yet: {streaming_patch}"
    );
}

#[tokio::test]
async fn btw_bus_close_freezes_lost_card() {
    let store = test_store().await;
    let kernel = test_kernel().await;
    let sid = btw_session(&kernel).await;
    store
        .save_mapping("mock", "oc_1", &sid, "oc_1", None, MappingKind::Normal)
        .await
        .unwrap();

    let mock = Arc::new(BtwMockAdapter::card());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    handle_incoming_message(
        "mock",
        &mock_config(),
        &store,
        kernel.clone(),
        btw_msg("/btw 问题"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();

    // Publish a delta, then close the bus mid-answer: the delivery task
    // must freeze a terminal "Event stream interrupted" card instead of leaving the
    // spinner card. A probe subscriber with the same filter makes
    // the shutdown point deterministic: shutdown drops events still in
    // the forwarder, so wait until both events reached a subscriber
    // queue before closing.
    let bus = kernel.event_bus().unwrap();
    let mut probe = bus.subscribe_filtered(sid.clone(), |env: &Envelope| {
        matches!(env.event, Event::Btw(_))
    });
    let fire = |ev: BtwEvent| {
        bus.publish(sid.clone(), Envelope::new(sid.clone(), Event::Btw(ev)))
            .unwrap();
    };
    fire(BtwEvent::Start {
        request_id: BtwId::from("btw_lost"),
    });
    fire(BtwEvent::Delta {
        request_id: BtwId::from("btw_lost"),
        text: "收到的一半答案".to_string(),
    });
    // Both events dequeued by the probe = both are past the forwarder
    // and in subscriber queues; shutdown is now race-free.
    probe.recv().await;
    probe.recv().await;
    bus.shutdown();

    let final_patch = wait_for(|| {
        let patches = mock.patches.try_lock().ok()?;
        patches
            .iter()
            .find(|p| p.contains("Event stream interrupted"))
            .cloned()
    })
    .await
    .expect("lost-terminal card patch");
    assert!(final_patch.contains("收到的一半答案"), "{final_patch}");
}
