use super::*;
use crate::channels::reply;

use crate::channels::{
    hub_command::*, hub_context::*, hub_deliver::*, hub_gate::*, hub_handlers::*, hub_routing::*,
};
use crate::channels::{HistoryContainer, HistoryMessage, PlatformAdapter};
use crate::types::ContentBlock;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::channels::store::SqliteChannelStore;
use crate::channels::PlatformConfig;
use crate::storage::migrations::run_migrations;
use crate::storage::{NewSession, SessionStore, SqliteSessionStore};
use sqlx::sqlite::SqlitePoolOptions;

pub struct MockAdapter {
    pub outgoing: tokio::sync::Mutex<Vec<(String, Vec<ContentBlock>)>>,
    pub reactions: tokio::sync::Mutex<Vec<(String, String)>>,
    pub quoted: tokio::sync::Mutex<Option<crate::channels::HistoryMessage>>,
    /// Per-id quoted responses (quote chains); consulted before `quoted`.
    pub quoted_map:
        tokio::sync::Mutex<std::collections::HashMap<String, crate::channels::HistoryMessage>>,
    pub quoted_calls: tokio::sync::Mutex<Vec<String>>,
    /// When false (default), image downloads fail — mirroring the trait's
    /// default unsupported behavior so degradation paths stay testable.
    pub image_download_ok: tokio::sync::Mutex<bool>,
    /// Doc-comment replies sent: (`comment_id`, chunk text).
    pub comment_replies: tokio::sync::Mutex<Vec<(String, String)>>,
    /// Thread id → root message id, returned by `thread_root_id`.
    pub thread_roots: tokio::sync::Mutex<std::collections::HashMap<String, String>>,
    /// `thread_root_id` backing-lookup call log (cache hits are not
    /// logged, mirroring the real adapter's caching contract).
    pub thread_root_calls: tokio::sync::Mutex<Vec<String>>,
    /// Successful-lookup cache for `thread_root_id`.
    thread_root_cache: tokio::sync::Mutex<std::collections::HashMap<String, String>>,
    /// Cards sent: (chat, card json, reply anchor).
    pub cards: tokio::sync::Mutex<Vec<(String, String, Option<String>)>>,
    /// Gates `supports_status_card` (default false → text fallback).
    pub status_card_ok: std::sync::atomic::AtomicBool,
    /// When true, `send_message` returns synthetic ids (msg-1, msg-2, …)
    /// — off by default so existing send-result-agnostic tests are
    /// unaffected.
    pub issue_ids: std::sync::atomic::AtomicBool,
    pub send_counter: std::sync::atomic::AtomicUsize,
}

impl MockAdapter {
    pub fn new(_name: impl Into<String>) -> Self {
        Self {
            outgoing: tokio::sync::Mutex::new(Vec::new()),
            reactions: tokio::sync::Mutex::new(Vec::new()),
            quoted: tokio::sync::Mutex::new(None),
            quoted_map: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            quoted_calls: tokio::sync::Mutex::new(Vec::new()),
            image_download_ok: tokio::sync::Mutex::new(false),
            comment_replies: tokio::sync::Mutex::new(Vec::new()),
            thread_roots: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            thread_root_calls: tokio::sync::Mutex::new(Vec::new()),
            thread_root_cache: tokio::sync::Mutex::new(std::collections::HashMap::new()),
            cards: tokio::sync::Mutex::new(Vec::new()),
            status_card_ok: std::sync::atomic::AtomicBool::new(false),
            issue_ids: std::sync::atomic::AtomicBool::new(false),
            send_counter: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

/// Text of the mock's most recent outgoing message (info-command replies
/// travel the adapter, not the handler's return value).
async fn last_outgoing_text(mock: &MockAdapter) -> String {
    mock.outgoing
        .lock()
        .await
        .last()
        .and_then(|(_, blocks)| match blocks.first() {
            Some(ContentBlock::Text { text }) => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelEvent>,
        cancel: CancellationToken,
    ) -> std::result::Result<(), crate::channels::ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        external_chat_id: &str,
        blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        self.outgoing
            .lock()
            .await
            .push((external_chat_id.to_string(), blocks));
        let id = self
            .issue_ids
            .load(std::sync::atomic::Ordering::Relaxed)
            .then(|| {
                format!(
                    "msg-{}",
                    self.send_counter
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                        + 1
                )
            });
        Ok(id)
    }

    async fn send_reaction(
        &self,
        _external_chat_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        self.reactions
            .lock()
            .await
            .push((message_id.to_string(), emoji.to_string()));
        Ok(Some("reaction-1".to_string()))
    }

    async fn fetch_message(
        &self,
        message_id: &str,
    ) -> std::result::Result<Option<crate::channels::HistoryMessage>, crate::channels::ChannelError>
    {
        self.quoted_calls.lock().await.push(message_id.to_string());
        if let Some(m) = self.quoted_map.lock().await.get(message_id) {
            return Ok(Some(m.clone()));
        }
        Ok(self.quoted.lock().await.clone())
    }

    async fn thread_root_id(
        &self,
        thread_id: &str,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        if let Some(root) = self.thread_root_cache.lock().await.get(thread_id) {
            return Ok(Some(root.clone()));
        }
        self.thread_root_calls
            .lock()
            .await
            .push(thread_id.to_string());
        let root = self.thread_roots.lock().await.get(thread_id).cloned();
        if let Some(root) = &root {
            self.thread_root_cache
                .lock()
                .await
                .insert(thread_id.to_string(), root.clone());
        }
        Ok(root)
    }

    async fn message_link(&self, chat_id: &str, message_id: &str) -> Option<String> {
        Some(format!("link://{chat_id}/{message_id}"))
    }

    async fn chat_link(&self, chat_id: &str) -> Option<String> {
        Some(format!("link://chat/{chat_id}"))
    }

    async fn thread_link(&self, chat_id: &str, message_id: &str) -> Option<String> {
        Some(format!("link://thread/{chat_id}/{message_id}"))
    }

    fn supports_status_card(&self) -> bool {
        self.status_card_ok
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    async fn send_card(
        &self,
        external_chat_id: &str,
        card_json: &str,
        reply_msg_id: Option<&str>,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        self.cards.lock().await.push((
            external_chat_id.to_string(),
            card_json.to_string(),
            reply_msg_id.map(str::to_string),
        ));
        Ok(Some("card-1".to_string()))
    }

    async fn download_message_image(
        &self,
        _message_id: &str,
        image_key: &str,
    ) -> std::result::Result<ContentBlock, crate::channels::ChannelError> {
        if !*self.image_download_ok.lock().await {
            return Err(crate::channels::ChannelError::Platform(
                "mock: image download disabled".into(),
            ));
        }
        Ok(ContentBlock::ImageUrl {
            image_url: format!("data:image/png;base64,fake-{image_key}").into(),
        })
    }

    async fn reply_doc_comment(
        &self,
        _file_token: &str,
        _file_type: &str,
        comment_id: &str,
        text: &str,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        self.comment_replies
            .lock()
            .await
            .push((comment_id.to_string(), text.to_string()));
        Ok(Some(format!(
            "reply-{}",
            self.comment_replies.lock().await.len()
        )))
    }
}

async fn create_test_pool() -> (sqlx::SqlitePool, Arc<SqliteChannelStore>) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    let store = Arc::new(SqliteChannelStore::new(pool.clone()));
    (pool, store)
}

async fn create_model_key_test_stores() -> (Arc<dyn ChannelStore>, Arc<dyn SessionStore>) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    run_migrations(&pool).await.unwrap();
    (
        Arc::new(SqliteChannelStore::new(pool.clone())),
        Arc::new(SqliteSessionStore::new(pool)),
    )
}

async fn create_session_with_model(
    store: &Arc<dyn SessionStore>,
    model_key: Option<&str>,
) -> SessionId {
    let id = SessionId::new();
    store
        .create(NewSession {
            model_key: model_key.map(str::to_string),
            ..NewSession::new(id.clone())
        })
        .await
        .unwrap();
    id
}

#[tokio::test]
async fn test_thread_session_inherits_parent_chat_model_key() {
    let (channel_store, session_store) = create_model_key_test_stores().await;
    let parent_id = create_session_with_model(&session_store, Some("parent-model")).await;
    channel_store
        .save_mapping("feishu", "chat-1", &parent_id, "chat-1", None)
        .await
        .unwrap();

    let model_key = model_key_for_new_channel_session(
        "feishu",
        "chat-1",
        "chat-1:thread-1",
        &channel_store,
        &session_store,
    )
    .await
    .unwrap();

    assert_eq!(model_key.as_deref(), Some("parent-model"));
}

#[tokio::test]
async fn test_thread_session_leaves_model_key_unset_without_explicit_parent_model() {
    let (channel_store, session_store) = create_model_key_test_stores().await;
    let parent_id = create_session_with_model(&session_store, None).await;
    channel_store
        .save_mapping("feishu", "chat-1", &parent_id, "chat-1", None)
        .await
        .unwrap();

    let without_model = model_key_for_new_channel_session(
        "feishu",
        "chat-1",
        "chat-1:thread-1",
        &channel_store,
        &session_store,
    )
    .await
    .unwrap();
    let without_mapping = model_key_for_new_channel_session(
        "feishu",
        "chat-2",
        "chat-2:thread-1",
        &channel_store,
        &session_store,
    )
    .await
    .unwrap();

    let missing_parent_id = SessionId::new();
    channel_store
        .save_mapping("feishu", "chat-3", &missing_parent_id, "chat-3", None)
        .await
        .unwrap();
    let without_parent_session = model_key_for_new_channel_session(
        "feishu",
        "chat-3",
        "chat-3:thread-1",
        &channel_store,
        &session_store,
    )
    .await
    .unwrap();

    assert_eq!(without_model, None);
    assert_eq!(without_mapping, None);
    assert_eq!(without_parent_session, None);
}

#[tokio::test]
async fn test_non_thread_session_does_not_inherit_model_key() {
    let (channel_store, session_store) = create_model_key_test_stores().await;
    let parent_id = create_session_with_model(&session_store, Some("parent-model")).await;
    channel_store
        .save_mapping("feishu", "chat-1", &parent_id, "chat-1", None)
        .await
        .unwrap();

    let model_key = model_key_for_new_channel_session(
        "feishu",
        "chat-1",
        "chat-1",
        &channel_store,
        &session_store,
    )
    .await
    .unwrap();

    assert_eq!(model_key, None);
}

#[tokio::test]
async fn test_start_and_shutdown() {
    let (_pool, store) = create_test_pool().await;
    let cancel = CancellationToken::new();
    let hub = ChannelHub::new(store);

    let configs = vec![
        ChannelConfig {
            name: "mock1".to_string(),
            enabled: true,
            platform: crate::channels::PlatformConfig::Telegram {
                token: "fake".to_string(),
            },
            allowed_chats: vec![],
            allowed_users: vec![],
            blocked_chats: vec![],
            blocked_users: vec![],
            require_mention: false,
            reply_in_thread: false,
            auto_approve_level: crate::permission::Level::Safe,
            observability: true,
            tool_trace: true,
            mid_run_split: true,
            history_context: 0,
            approval_chat_id: None,
            admin_users: vec![],
            disabled_events: vec![],
        },
        ChannelConfig {
            name: "mock2".to_string(),
            enabled: true,
            platform: crate::channels::PlatformConfig::Telegram {
                token: "fake2".to_string(),
            },
            allowed_chats: vec![],
            allowed_users: vec![],
            blocked_chats: vec![],
            blocked_users: vec![],
            require_mention: false,
            reply_in_thread: false,
            auto_approve_level: crate::permission::Level::Safe,
            observability: true,
            tool_trace: true,
            mid_run_split: true,
            history_context: 0,
            approval_chat_id: None,
            admin_users: vec![],
            disabled_events: vec![],
        },
    ];

    hub.start_all(cancel.clone(), configs, std::sync::Weak::new())
        .await
        .unwrap();

    let channels = hub.list_channels();
    assert_eq!(channels.len(), 2);

    cancel.cancel();
}

#[tokio::test]
async fn test_disabled_channel_skipped() {
    let (_pool, store) = create_test_pool().await;
    let cancel = CancellationToken::new();
    let ch = ChannelHub::new(store);

    let configs = vec![
        ChannelConfig {
            name: "enabled".to_string(),
            enabled: true,
            platform: PlatformConfig::Telegram {
                token: "fake".into(),
            },
            ..Default::default()
        },
        ChannelConfig {
            name: "disabled".to_string(),
            enabled: false,
            platform: PlatformConfig::Telegram {
                token: "fake".into(),
            },
            ..Default::default()
        },
    ];

    ch.start_all(cancel.clone(), configs, std::sync::Weak::new())
        .await
        .unwrap();

    let channels = ch.list_channels();
    assert_eq!(channels.len(), 1);
    assert_eq!(channels[0].name, "enabled");

    cancel.cancel();
}

#[tokio::test]
async fn test_skip_existing_channel() {
    let (_pool, store) = create_test_pool().await;
    let cancel = CancellationToken::new();
    let hub = ChannelHub::new(store);

    let config = ChannelConfig {
        name: "only_once".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        ..Default::default()
    };

    hub.start_all(cancel.clone(), vec![config.clone()], std::sync::Weak::new())
        .await
        .unwrap();
    assert_eq!(hub.list_channels().len(), 1);

    // Second attempt should be skipped
    hub.start_all(cancel.clone(), vec![config], std::sync::Weak::new())
        .await
        .unwrap();
    assert_eq!(hub.list_channels().len(), 1);

    cancel.cancel();
}

#[tokio::test]
async fn test_mock_adapter_send_message() {
    let adapter = MockAdapter::new("test");
    let blocks = vec![ContentBlock::Text {
        text: "hello".into(),
    }];
    adapter.send_message("chat1", blocks, None).await.unwrap();
    let out = adapter.outgoing.lock().await;
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].0, "chat1");
}

// ── flush_reply ─────────────────────────────────────────────────────

/// Card-capable mock: records `send_card` payloads and `update_card` patches.
pub struct CardMockAdapter {
    pub cards: tokio::sync::Mutex<Vec<(String, String)>>,
    pub patches: tokio::sync::Mutex<Vec<(String, String)>>,
    pub outgoing: tokio::sync::Mutex<Vec<String>>,
    pub reactions: tokio::sync::Mutex<Vec<(String, String)>>,
    /// When set, `send_card`/`send_message` fail (platform outage).
    pub fail_sends: std::sync::atomic::AtomicBool,
}

impl CardMockAdapter {
    fn new() -> Self {
        Self {
            cards: tokio::sync::Mutex::new(Vec::new()),
            patches: tokio::sync::Mutex::new(Vec::new()),
            outgoing: tokio::sync::Mutex::new(Vec::new()),
            reactions: tokio::sync::Mutex::new(Vec::new()),
            fail_sends: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for CardMockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelEvent>,
        cancel: CancellationToken,
    ) -> std::result::Result<(), crate::channels::ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        _external_chat_id: &str,
        _blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        if self.fail_sends.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::channels::ChannelError::Platform(
                "mock send_message failure".into(),
            ));
        }
        for block in &_blocks {
            if let ContentBlock::Text { text } = block {
                self.outgoing.lock().await.push(text.clone());
            }
        }
        Ok(Some("msg-1".to_string()))
    }

    async fn send_card(
        &self,
        external_chat_id: &str,
        card_json: &str,
        _reply_msg_id: Option<&str>,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        if self.fail_sends.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::channels::ChannelError::Platform(
                "mock send_card failure".into(),
            ));
        }
        self.cards
            .lock()
            .await
            .push((external_chat_id.to_string(), card_json.to_string()));
        Ok(Some("card-msg-1".to_string()))
    }

    async fn update_card(
        &self,
        message_id: &str,
        card_json: &str,
    ) -> std::result::Result<(), crate::channels::ChannelError> {
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
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        self.reactions
            .lock()
            .await
            .push((message_id.to_string(), emoji.to_string()));
        Ok(Some("reaction-1".to_string()))
    }

    fn supports_status_card(&self) -> bool {
        true
    }
}

fn test_routing() -> SessionRouting {
    SessionRouting {
        channel_name: "feishu".to_string(),
        external_chat_id: "chat-1".to_string(),
        reply_msg_id: None,
        mapping_key: "chat-1".to_string(),
        doc_comment: None,
    }
}

fn run_buffer() -> reply::RunReplyBuffer {
    let mut buf = reply::RunReplyBuffer::new();
    buf.record_model_end("Let me check.");
    buf.record_tool_start("t1", "shell", Some(r#"{"command":"cargo test"}"#));
    buf.record_tool_end("t1", 2000, false);
    buf.record_model_end("final answer");
    buf
}

#[tokio::test]
async fn flush_reply_card_platform_sends_single_card_with_panel() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    flush_reply(&adapter, &test_routing(), run_buffer().into_reply(), true).await;

    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].0, "chat-1");
    let card = &cards[0].1;
    assert!(card.contains("collapsible_panel"));
    assert!(card.contains("final answer"));
    assert!(card.contains("Let me check."), "narration joins the panel");
    assert!(card.contains("cargo test"), "tool summary joins the panel");
}

#[tokio::test]
async fn flush_reply_plain_platform_appends_trace_lines() {
    let mock = Arc::new(MockAdapter::new("tg"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    flush_reply(&adapter, &test_routing(), run_buffer().into_reply(), true).await;

    let out = mock.outgoing.lock().await;
    assert_eq!(out.len(), 1);
    let ContentBlock::Text { text } = &out[0].1[0] else {
        panic!("expected text block");
    };
    assert!(text.starts_with("final answer"));
    assert!(text.contains("🐾 0s · 💬 2"));
    assert!(text.contains("cargo test"));
}

#[tokio::test]
async fn flush_reply_tool_trace_disabled_sends_bare_text() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let plain = Arc::new(MockAdapter::new("plain"));
    let plain_adapter: Arc<dyn PlatformAdapter> = plain.clone();

    // Card platform with tool_trace off: falls back to send_message with the
    // bare text (trace dropped entirely).
    let routing = test_routing();
    flush_reply(&adapter, &routing, run_buffer().into_reply(), false).await;
    assert!(mock.cards.lock().await.is_empty(), "no card sent");

    flush_reply(&plain_adapter, &routing, run_buffer().into_reply(), false).await;
    let out = plain.outgoing.lock().await;
    assert_eq!(out.len(), 1);
    let ContentBlock::Text { text } = &out[0].1[0] else {
        panic!("expected text block");
    };
    assert_eq!(text, "final answer");
}

#[tokio::test]
async fn flush_reply_without_text_sends_nothing() {
    let mock = Arc::new(MockAdapter::new("test"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    let mut buf = reply::RunReplyBuffer::new();
    buf.record_tool_start("t1", "read", None);
    buf.record_tool_end("t1", 5, false);

    flush_reply(&adapter, &test_routing(), buf.into_reply(), true).await;
    assert!(mock.outgoing.lock().await.is_empty());
}

#[tokio::test]
async fn test_skip_duplicate_channel() {
    let (_pool, store) = create_test_pool().await;
    let cancel = CancellationToken::new();
    let hub = ChannelHub::new(store);

    // Create a huge number of configs with the same name to trigger skip
    let configs = vec![
        ChannelConfig {
            name: "dup".to_string(),
            enabled: true,
            platform: PlatformConfig::Telegram {
                token: "fake".into(),
            },
            ..Default::default()
        },
        ChannelConfig {
            name: "dup".to_string(),
            enabled: true,
            platform: PlatformConfig::Telegram {
                token: "fake2".into(),
            },
            ..Default::default()
        },
    ];

    // Should succeed but only start one
    hub.start_all(cancel.clone(), configs, std::sync::Weak::new())
        .await
        .unwrap();
    assert_eq!(hub.list_channels().len(), 1);

    cancel.cancel();
}

#[test]
fn test_parse_model_commands() {
    assert!(matches!(
        parse_channel_command(Some("/models")),
        ChannelCommand::ListModels
    ));
    assert!(matches!(
        parse_channel_command(Some("  /model\n")),
        ChannelCommand::CurrentModel
    ));
    assert!(matches!(
        parse_channel_command(Some("/model claude-sonnet")),
        ChannelCommand::SwitchModel(ref key) if key == "claude-sonnet"
    ));
    assert!(matches!(
        parse_channel_command(Some("/models@yomi_bot")),
        ChannelCommand::ListModels
    ));
    assert!(matches!(
        parse_channel_command(Some("/model@yomi_bot nova-2")),
        ChannelCommand::SwitchModel(ref key) if key == "nova-2"
    ));
    assert!(matches!(
        parse_channel_command(Some("/models nova-2")),
        ChannelCommand::SwitchModel(ref key) if key == "nova-2"
    ));
}

#[test]
fn test_parse_invalid_model_command() {
    assert!(matches!(
        parse_channel_command(Some("/model one two")),
        ChannelCommand::InvalidModelCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("please use /model nova-2")),
        ChannelCommand::None
    ));
    assert!(matches!(parse_channel_command(None), ChannelCommand::None));
}

#[test]
fn test_parse_existing_commands_from_raw_text() {
    assert!(matches!(
        parse_channel_command(Some("/clear")),
        ChannelCommand::Clear
    ));
    assert!(matches!(
        parse_channel_command(Some("/stop")),
        ChannelCommand::Stop
    ));
    assert!(matches!(
        parse_channel_command(Some("/stop@yomi_bot")),
        ChannelCommand::Stop
    ));
    assert!(matches!(
        parse_channel_command(Some("/steer@yomi_bot inspect the logs")),
        ChannelCommand::Steer(ref text) if text == "inspect the logs"
    ));
    assert!(matches!(
        parse_channel_command(Some("/steer inspect the logs")),
        ChannelCommand::Steer(ref text) if text == "inspect the logs"
    ));
    assert!(matches!(
        parse_channel_command(Some("/queue@yomi_bot run this next")),
        ChannelCommand::Queue(ref text) if text == "run this next"
    ));
    assert!(matches!(
        parse_channel_command(Some("/queue run this next")),
        ChannelCommand::Queue(ref text) if text == "run this next"
    ));
    assert!(matches!(
        parse_channel_command(Some("/steer")),
        ChannelCommand::InvalidSteerCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/queue")),
        ChannelCommand::InvalidQueueCommand
    ));
}

#[test]
fn test_parse_command_aliases() {
    assert!(matches!(
        parse_channel_command(Some("/h")),
        ChannelCommand::Help
    ));
    assert!(matches!(
        parse_channel_command(Some("/i")),
        ChannelCommand::Info
    ));
    assert!(matches!(
        parse_channel_command(Some("/c")),
        ChannelCommand::Clear
    ));
    assert!(matches!(
        parse_channel_command(Some("/c@yomi_bot")),
        ChannelCommand::Clear
    ));
    assert!(matches!(
        parse_channel_command(Some("/s")),
        ChannelCommand::Stop
    ));
    assert!(matches!(
        parse_channel_command(Some("/m")),
        ChannelCommand::CurrentModel
    ));
    assert!(matches!(
        parse_channel_command(Some("/m sonnet")),
        ChannelCommand::SwitchModel(ref key) if key == "sonnet"
    ));
    assert!(matches!(
        parse_channel_command(Some("/q run this next")),
        ChannelCommand::Queue(ref text) if text == "run this next"
    ));
    assert!(matches!(
        parse_channel_command(Some("/t hi there")),
        ChannelCommand::Thread(ref text) if text == "hi there"
    ));
    assert!(matches!(
        parse_channel_command(Some("/sub")),
        ChannelCommand::Subscribe {
            recursive: false,
            target_chat_id: None
        }
    ));
    assert!(matches!(
        parse_channel_command(Some("/unsub")),
        ChannelCommand::Unsubscribe
    ));
    // Aliases share the canonical command's argument validation.
    assert!(matches!(
        parse_channel_command(Some("/c now")),
        ChannelCommand::None
    ));
    assert!(matches!(
        parse_channel_command(Some("/q")),
        ChannelCommand::InvalidQueueCommand
    ));
    // Every canonical name shows up in `/help`.
    for (name, _) in COMMANDS {
        assert!(HELP_TEXT.contains(name), "help text missing {name}");
    }
}

#[test]
fn test_parse_unknown_command() {
    // Command-shaped tokens matching nothing report as unknown…
    assert!(matches!(
        parse_channel_command(Some("/claer")),
        ChannelCommand::Unknown(ref cmd) if cmd == "/claer"
    ));
    assert!(matches!(
        parse_channel_command(Some("/foo@yomi_bot")),
        ChannelCommand::Unknown(ref cmd) if cmd == "/foo@yomi_bot"
    ));
    // … while paths and prose pass through to the agent as messages.
    assert!(matches!(
        parse_channel_command(Some("/tmp/foo")),
        ChannelCommand::None
    ));
    assert!(matches!(
        parse_channel_command(Some("/tmp/foo bar")),
        ChannelCommand::None
    ));
    assert!(matches!(
        parse_channel_command(Some("hello")),
        ChannelCommand::None
    ));
}

#[tokio::test]
async fn unknown_command_replies_with_error() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        ..Default::default()
    };
    let msg = ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_1".to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some("/claer".to_string()),
        content: vec![],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: false,
        create_time: None,
        doc_comment: None,
    };

    let reply = handle_incoming_message("mock", &config, &store, kernel, msg, &obs, &adapter)
        .await
        .unwrap()
        .unwrap();
    assert!(reply.contains("Unknown command `/claer`"), "{reply}");
    assert!(reply.contains("/help"), "{reply}");
}

#[test]
fn test_parse_help_command() {
    assert!(matches!(
        parse_channel_command(Some("/help")),
        ChannelCommand::Help
    ));
    assert!(matches!(
        parse_channel_command(Some("/help@yomi_bot")),
        ChannelCommand::Help
    ));
    assert!(matches!(
        parse_channel_command(Some("/help extra")),
        ChannelCommand::None
    ));
    assert!(HELP_TEXT.contains("/steer") && HELP_TEXT.contains("/models"));
}

#[test]
fn test_parse_compact_command() {
    assert!(matches!(
        parse_channel_command(Some("/compact")),
        ChannelCommand::Compact
    ));
    assert!(matches!(
        parse_channel_command(Some("/compact@yomi_bot")),
        ChannelCommand::Compact
    ));
    assert!(matches!(
        parse_channel_command(Some("/compact now")),
        ChannelCommand::None
    ));
    // Prefix lookalikes never trigger the command — they report as
    // unknown instead.
    assert!(matches!(
        parse_channel_command(Some("/compaction")),
        ChannelCommand::Unknown(_)
    ));
    assert!(HELP_TEXT.contains("/compact"));
}

#[test]
fn test_parse_restart_command() {
    assert!(matches!(
        parse_channel_command(Some("/restart")),
        ChannelCommand::Restart
    ));
    assert!(matches!(
        parse_channel_command(Some("/restart@yomi_bot")),
        ChannelCommand::Restart
    ));
    assert!(matches!(
        parse_channel_command(Some("/restart now")),
        ChannelCommand::None
    ));
    assert!(HELP_TEXT.contains("/restart"));
}

#[test]
fn test_parse_thread_command() {
    assert!(matches!(
        parse_channel_command(Some("/thread 帮我看看这个问题")),
        ChannelCommand::Thread(text) if text == "帮我看看这个问题"
    ));
    assert!(matches!(
        parse_channel_command(Some("/thread@yomi_bot hi there")),
        ChannelCommand::Thread(text) if text == "hi there"
    ));
    // Text is required — a bare command is a usage error; `/threads`
    // (the reply-in-thread override) parses as its own command.
    assert!(matches!(
        parse_channel_command(Some("/thread")),
        ChannelCommand::InvalidThreadCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/threads hi")),
        ChannelCommand::InvalidThreadsCommand
    ));
    assert!(HELP_TEXT.contains("/thread"));
}

/// `/bind`: no-arg shows the current binding; retargeting is admin-only,
/// adopts unrouted sessions, refuses sessions routed to another chat,
/// and refuses cross-scope binds for doc comments (delivery would target
/// the wrong document).
#[tokio::test]
async fn bind_command_show_adopt_and_guards() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        admin_users: vec!["ou_admin".to_string()],
        ..Default::default()
    };
    let msg = |user: &str, raw: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: user.to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: false,
        create_time: None,
        doc_comment: None,
    };
    let call = |msg: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            msg,
            &obs,
            &adapter,
        )
    };

    // No binding yet: no-arg shows guidance.
    let reply = call(msg("ou_admin", "/bind")).await.unwrap().unwrap();
    assert!(reply.contains("No session here yet"), "{reply}");

    // An unrouted (e.g. GUI/CLI-created) session to adopt.
    let sid = kernel
        .create_session(crate::kernel::CreateSessionInput {
            project_id: None,
            working_dir: None,
            auto_approve_level: Some(crate::permission::Level::Dangerous),
            tool_blocklist: vec![],
            model_key: None,
        })
        .await
        .unwrap();
    let bind_cmd = format!("/bind {}", sid.0);

    // Non-admin cannot bind.
    let reply = call(msg("ou_random", &bind_cmd)).await.unwrap().unwrap();
    assert!(reply.contains("Permission denied"), "{reply}");

    // Admin binds: the scope now maps to the session.
    let reply = call(msg("ou_admin", &bind_cmd)).await.unwrap().unwrap();
    assert!(reply.contains("Bound this conversation"), "{reply}");
    assert_eq!(
        store.find_mapping("mock", "oc_1").await.unwrap(),
        Some(sid.clone())
    );

    // Idempotent re-bind.
    let reply = call(msg("ou_admin", &bind_cmd)).await.unwrap().unwrap();
    assert!(reply.contains("Already bound"), "{reply}");

    // Unknown session id.
    let reply = call(msg("ou_admin", "/bind sess_nope"))
        .await
        .unwrap()
        .unwrap();
    assert!(reply.contains("not found"), "{reply}");

    // No-arg now reports the binding.
    let reply = call(msg("ou_admin", "/bind")).await.unwrap().unwrap();
    assert!(reply.contains(&*sid.0), "{reply}");

    // Cross-chat refusal: the session is routed to oc_1.
    let mut other_chat = msg("ou_admin", &bind_cmd);
    other_chat.external_chat_id = "oc_2".to_string();
    let reply = call(other_chat).await.unwrap().unwrap();
    assert!(reply.contains("refusing to rebind"), "{reply}");

    // Doc-comment scope: a chat-routed session is incompatible (delivery
    // targets the mapping row — sharing would post to the wrong place).
    let mut doc_msg = msg("ou_admin", &bind_cmd);
    doc_msg.external_chat_id = String::new();
    doc_msg.external_message_id = None;
    doc_msg.doc_comment = Some(crate::channels::DocCommentRef {
        file_token: "tok".to_string(),
        file_type: "docx".to_string(),
        comment_id: "c_9".to_string(),
    });
    let reply = call(doc_msg).await.unwrap().unwrap();
    assert!(reply.contains("refusing to rebind"), "{reply}");

    // Malformed: too many args.
    let reply = call(msg("ou_admin", "/bind a b")).await.unwrap().unwrap();
    assert!(reply.contains("Usage"), "{reply}");
}

/// `/bind` retarget semantics: an already-routed session is *moved* (the
/// old scope's mapping row is deleted, so delivery — one row per session —
/// follows the new conversation), and the reply names the displaced
/// session so it can be bound back.
#[tokio::test]
async fn bind_command_move_and_bind_back() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        admin_users: vec!["ou_admin".to_string()],
        ..Default::default()
    };
    let msg = |raw: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_admin".to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: false,
        create_time: None,
        doc_comment: None,
    };
    let call = |msg: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            msg,
            &obs,
            &adapter,
        )
    };
    let new_session = || crate::kernel::CreateSessionInput {
        project_id: None,
        working_dir: None,
        auto_approve_level: Some(crate::permission::Level::Dangerous),
        tool_blocklist: vec![],
        model_key: None,
    };
    let sid1 = kernel.create_session(new_session()).await.unwrap();
    let sid2 = kernel.create_session(new_session()).await.unwrap();

    // Adopt sid1 at chat level — nothing displaced, no previous to name.
    let reply = call(msg(&format!("/bind {}", sid1.0)))
        .await
        .unwrap()
        .unwrap();
    assert!(reply.contains("Bound this conversation"), "{reply}");
    assert!(!reply.contains("Previously bound"), "{reply}");

    // Retarget the scope to sid2: the reply names sid1 for bind-back, and
    // sid1 loses its only mapping row (unrouted again).
    let reply = call(msg(&format!("/bind {}", sid2.0)))
        .await
        .unwrap()
        .unwrap();
    assert!(reply.contains("Previously bound"), "{reply}");
    assert!(reply.contains(&format!("/bind {}", sid1.0)), "{reply}");
    assert_eq!(
        store.find_mapping("mock", "oc_1").await.unwrap(),
        Some(sid2.clone())
    );
    assert!(store
        .find_routing_by_session(&sid1)
        .await
        .unwrap()
        .is_none());

    // Binding back works; the reply now names sid2.
    let reply = call(msg(&format!("/bind {}", sid1.0)))
        .await
        .unwrap()
        .unwrap();
    assert!(reply.contains("Bound this conversation"), "{reply}");
    assert!(reply.contains(&*sid2.0), "{reply}");

    // Move: bind sid1 from a thread of the same chat.
    let mut thread_msg = msg(&format!("/bind {}", sid1.0));
    thread_msg.is_group = true;
    thread_msg.thread_id = Some("t1".to_string());
    thread_msg.external_message_id = Some("m2".to_string());
    let reply = call(thread_msg).await.unwrap().unwrap();
    assert!(reply.contains("Moved"), "{reply}");

    // The old scope's row is gone; delivery routing follows the move.
    assert_eq!(store.find_mapping("mock", "oc_1").await.unwrap(), None);
    assert_eq!(
        store.find_mapping("mock", "t1").await.unwrap(),
        Some(sid1.clone())
    );
    let routing = store.find_routing_by_session(&sid1).await.unwrap().unwrap();
    assert_eq!(routing.mapping_key, "t1");
    assert_eq!(routing.external_chat_id, "oc_1");

    // The old conversation got a farewell naming the moved session.
    let outgoing = mock.outgoing.lock().await;
    assert_eq!(outgoing.len(), 1);
    let (chat, blocks) = &outgoing[0];
    assert_eq!(chat, "oc_1");
    let text = blocks_text(blocks);
    assert!(text.contains("has moved"), "{text}");
    assert!(text.contains(&*sid1.0), "{text}");
    drop(outgoing);
}

// ── Chat-level command scope (reply_in_thread) ──────────────────────

/// Test rig for chat-level command tests: a Feishu-shaped config with
/// reply_in_thread on, plus a `call` driving the handler directly.
struct ChatLevelRig {
    store: Arc<dyn ChannelStore>,
    kernel: Arc<Kernel>,
    mock: Arc<MockAdapter>,
    obs: Arc<ObsTracker>,
    config: ChannelConfig,
    /// Keeps the kernel's data dir alive (dropped last, after the kernel).
    _tmp: tempfile::TempDir,
}

impl ChatLevelRig {
    async fn new() -> Self {
        let (_pool, store) = create_test_pool().await;
        let store: Arc<dyn ChannelStore> = store;
        let tmp = tempfile::TempDir::new().unwrap();
        let mut kconfig = crate::config::Config {
            data_dir: tmp.path().to_path_buf(),
            ..crate::config::Config::default()
        };
        kconfig.finalize();
        let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
        let mock = Arc::new(MockAdapter::new("mock"));
        let obs = Arc::new(ObsTracker::new());
        let config = ChannelConfig {
            name: "mock".to_string(),
            enabled: true,
            platform: PlatformConfig::Feishu {
                app_id: "app".to_string(),
                app_secret: "secret".to_string(),
            },
            admin_users: vec!["user-1".to_string()],
            reply_in_thread: true,
            ..Default::default()
        };
        Self {
            store,
            kernel,
            mock,
            obs,
            config,
            _tmp: tmp,
        }
    }

    async fn call(&self, msg: ChannelMessage) -> Result<Option<String>> {
        let adapter: Arc<dyn PlatformAdapter> = self.mock.clone();
        handle_incoming_message(
            "mock",
            &self.config,
            &self.store,
            Arc::clone(&self.kernel),
            msg,
            &self.obs,
            &adapter,
        )
        .await
    }

    async fn new_session(&self) -> SessionId {
        self.kernel
            .create_session(crate::kernel::CreateSessionInput {
                project_id: None,
                working_dir: None,
                auto_approve_level: Some(crate::permission::Level::Dangerous),
                tool_blocklist: vec![],
                model_key: None,
            })
            .await
            .unwrap()
    }
}

fn chat_level_cmd(raw: &str) -> ChannelMessage {
    let mut msg = channel_message(None, true, true);
    msg.raw_text = Some(raw.to_string());
    msg
}

/// Chat-level command feedback in a `reply_in_thread` group lands in the
/// main flow (no anchor); the same command inside a thread stays
/// anchored to the command message.
#[tokio::test]
async fn help_card_anchor_follows_command_scope() {
    let rig = ChatLevelRig::new().await;
    rig.mock
        .status_card_ok
        .store(true, std::sync::atomic::Ordering::Relaxed);

    rig.call(chat_level_cmd("/help")).await.unwrap();
    let mut threaded = channel_message(Some("t1"), true, true);
    threaded.raw_text = Some("/help".to_string());
    rig.call(threaded).await.unwrap();

    let cards = rig.mock.cards.lock().await;
    assert_eq!(cards.len(), 2);
    assert_eq!(cards[0].2, None, "chat-level feedback must not anchor");
    assert_eq!(
        cards[1].2.as_deref(),
        Some("msg-1"),
        "in-thread feedback anchors to the command message"
    );
}

/// `/clear` at chat level addresses the chat session — honestly
/// reporting when there is none (the message-id key could never match
/// a session, and the old code claimed success for that no-op).
#[tokio::test]
async fn clear_at_chat_level_addresses_the_chat_session() {
    let rig = ChatLevelRig::new().await;

    let reply = rig.call(chat_level_cmd("/clear")).await.unwrap();
    assert_eq!(
        reply.as_deref(),
        Some("No session here yet — nothing to clear.")
    );

    let sid = rig.new_session().await;
    rig.store
        .save_mapping("mock", "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();
    let reply = rig.call(chat_level_cmd("/clear")).await.unwrap();
    assert_eq!(reply.as_deref(), Some("🧹 Context cleared."));
}

/// `/stop` at chat level addresses the chat session (same scope rule).
#[tokio::test]
async fn stop_at_chat_level_addresses_the_chat_session() {
    let rig = ChatLevelRig::new().await;

    let reply = rig.call(chat_level_cmd("/stop")).await.unwrap();
    assert_eq!(reply.as_deref(), Some("No active session to stop."));

    let sid = rig.new_session().await;
    rig.store
        .save_mapping("mock", "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();
    let reply = rig.call(chat_level_cmd("/stop")).await.unwrap();
    assert_eq!(reply.as_deref(), Some("⏹ Stopped."));
}

/// `/compact` at chat level addresses the chat session (same scope
/// rule) — honest when there is none.
#[tokio::test]
async fn compact_at_chat_level_addresses_the_chat_session() {
    let rig = ChatLevelRig::new().await;

    let reply = rig.call(chat_level_cmd("/compact")).await.unwrap();
    assert_eq!(reply.as_deref(), Some("No session to compact."));

    // With a chat session bound the lookup hits — the ack then depends
    // on the status-card/bus path, but it must not be the no-session
    // answer (locks the key switch).
    let sid = rig.new_session().await;
    rig.store
        .save_mapping("mock", "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();
    let reply = rig.call(chat_level_cmd("/compact")).await.unwrap();
    assert_ne!(
        reply.as_deref(),
        Some("No session to compact."),
        "must address the chat session: {reply:?}"
    );
}

/// `/bind` at chat level binds the chat scope — not the command
/// message's own id, a scope no follow-up ever reaches in
/// `reply_in_thread` mode.
#[tokio::test]
async fn bind_at_chat_level_binds_the_chat_scope() {
    let rig = ChatLevelRig::new().await;
    let sid = rig.new_session().await;

    let reply = rig
        .call(chat_level_cmd(&format!("/bind {}", sid.0)))
        .await
        .unwrap()
        .unwrap();
    assert!(reply.contains("Bound this conversation"), "{reply}");
    assert_eq!(
        rig.store.find_mapping("mock", "chat-1").await.unwrap(),
        Some(sid.clone())
    );
    assert_eq!(rig.store.find_mapping("mock", "msg-1").await.unwrap(), None);
}

/// `/status` and `/usage`: admin-gated daemon reports; the admin gets a
/// card, and at chat level it lands in the main flow (scope rule).
#[tokio::test]
async fn status_and_usage_require_admin_and_card_into_main_flow() {
    let rig = ChatLevelRig::new().await;
    rig.mock
        .status_card_ok
        .store(true, std::sync::atomic::Ordering::Relaxed);

    for raw in ["/status", "/usage"] {
        let mut outsider = chat_level_cmd(raw);
        outsider.external_user_id = "ou_random".to_string();
        let reply = rig.call(outsider).await.unwrap();
        assert_eq!(
            reply.as_deref(),
            Some("Permission denied: not in admin_users."),
            "{raw}"
        );
    }
    assert!(rig.mock.cards.lock().await.is_empty());

    let reply = rig.call(chat_level_cmd("/status")).await.unwrap();
    assert!(reply.is_none(), "status answers via card: {reply:?}");
    let reply = rig.call(chat_level_cmd("/usage")).await.unwrap();
    assert!(reply.is_none(), "usage answers via card: {reply:?}");

    let cards = rig.mock.cards.lock().await;
    assert_eq!(cards.len(), 2);
    assert!(cards[0].1.contains("🩺 Runtime"), "card: {}", cards[0].1);
    assert!(
        cards[0].1.contains("**Active runs**"),
        "card: {}",
        cards[0].1
    );
    assert!(cards[1].1.contains("📊 Usage · 7d"), "card: {}", cards[1].1);
    assert!(
        cards[1].1.contains("**Total (7d)**"),
        "card: {}",
        cards[1].1
    );
    assert_eq!(cards[0].2, None, "chat-level feedback must not anchor");
    assert_eq!(cards[1].2, None, "chat-level feedback must not anchor");
}

/// `/restart` (admin-only): the ack goes out inline via the adapter —
/// never through the spawned reply path, which the shutdown could abort —
/// and only then is the restart requested.
#[tokio::test]
async fn test_restart_command_gate_and_trigger() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        require_mention: false,
        admin_users: vec!["ou_admin".to_string()],
        ..Default::default()
    };
    let msg = |user: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: user.to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some("/restart".to_string()),
        content: vec![ContentBlock::Text {
            text: "/restart".to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: false,
        create_time: None,
        doc_comment: None,
    };

    // Non-admin: denied via the normal reply path; nothing sent inline.
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        msg("ou_random"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(
        reply.as_deref(),
        Some("Permission denied: not in admin_users.")
    );
    assert!(mock.outgoing.lock().await.is_empty());

    // Admin on a daemon without lifecycle support: polite refusal.
    assert!(!kernel.can_restart());
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        msg("ou_admin"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(
        reply.as_deref(),
        Some("Restart is not supported by this daemon.")
    );
    assert!(mock.outgoing.lock().await.is_empty());

    // Admin with lifecycle support: inline ack, then the restart request.
    let (restart_tx, mut restart_rx) = mpsc::channel(1);
    *kernel.restart_slot().lock().unwrap() = Some(restart_tx);
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        kernel,
        msg("ou_admin"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(reply, None, "ack sent inline, nothing for the reply path");
    restart_rx.try_recv().expect("restart requested");
    let outgoing = mock.outgoing.lock().await;
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].0, "oc_1");
    let ContentBlock::Text { text } = &outgoing[0].1[0] else {
        panic!("expected text ack");
    };
    assert!(text.contains("Restarting daemon"), "ack: {text}");
}

/// `/thread <text>` (one-shot): the trigger runs with a forced
/// `reply_in_thread` — its session keys by the command's own message id
/// — and follow-ups inside the opened thread adopt that session via
/// the thread root instead of starting a thread-id-keyed one.
#[tokio::test]
async fn test_thread_command_one_shot_and_adoption() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let base = |msg_id: &str, raw: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_1".to_string(),
        external_message_id: Some(msg_id.to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![ContentBlock::Text {
            text: raw.to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: None,
        doc_comment: None,
    };
    let handle = |m: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            m,
            &obs,
            &adapter,
        )
    };

    // Bare command → usage.
    let reply = handle(base("m0", "/thread")).await.unwrap();
    assert!(
        reply
            .as_deref()
            .is_some_and(|r| r.contains("Usage: `/thread <text>`")),
        "{reply:?}"
    );

    // One-shot trigger: the session keys by the command's own message
    // id (the future thread root).
    let reply = handle(base("m1", "/thread 看看这个")).await.unwrap();
    assert_eq!(reply, None);
    let sid = store
        .find_mapping("mock", "m1")
        .await
        .unwrap()
        .expect("thread session under the root key");

    // A follow-up inside the opened thread resolves to the root-keyed
    // session — no thread-keyed row appears at all.
    let mut follow_up = base("m2", "继续");
    follow_up.thread_id = Some("omt_1".to_string());
    follow_up.root_id = Some("m1".to_string());
    follow_up.parent_id = Some("om_bot".to_string());
    let reply = handle(follow_up).await.unwrap();
    assert_eq!(reply, None);
    assert!(
        store.find_mapping("mock", "omt_1").await.unwrap().is_none(),
        "no thread-keyed mapping row (unified root key)"
    );
    assert_eq!(
        store.find_mapping("mock", "m1").await.unwrap(),
        Some(sid),
        "the follow-up kept the /thread session"
    );
}

/// Private chat: in-thread follow-ups carry no `root_id` at all — the
/// thread's root is fetched from the platform once (cached), and the
/// follow-up resolves to the `/thread` session under the canonical
/// root key. No thread-keyed mapping row is ever written.
#[tokio::test]
async fn test_thread_followup_without_root_id_resolves_root() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let base = |msg_id: &str, raw: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_1".to_string(),
        external_message_id: Some(msg_id.to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![ContentBlock::Text {
            text: raw.to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: false,
        create_time: None,
        doc_comment: None,
    };
    let handle = |m: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            m,
            &obs,
            &adapter,
        )
    };

    // One-shot `/thread`: session keyed by the command's message id.
    let reply = handle(base("m1", "/thread 看看这个")).await.unwrap();
    assert_eq!(reply, None);
    let sid = store
        .find_mapping("mock", "m1")
        .await
        .unwrap()
        .expect("thread session under the root key");

    // The platform knows the root; the event doesn't carry it.
    mock.thread_roots
        .lock()
        .await
        .insert("omt_1".to_string(), "m1".to_string());

    let mut follow_up = base("m2", "继续");
    follow_up.thread_id = Some("omt_1".to_string());
    let reply = handle(follow_up).await.unwrap();
    assert_eq!(reply, None);
    assert!(
        store.find_mapping("mock", "omt_1").await.unwrap().is_none(),
        "no thread-keyed mapping row (unified root key)"
    );
    assert_eq!(
        store.find_mapping("mock", "m1").await.unwrap(),
        Some(sid.clone()),
        "the follow-up resolved to the /thread session"
    );
    assert_eq!(
        mock.thread_root_calls.lock().await.len(),
        1,
        "the root lookup runs once per fresh thread"
    );

    // Later messages hit the adapter's cache — no further backing
    // lookup even if the backing source forgets the root.
    mock.thread_roots.lock().await.clear();
    let mut follow_up2 = base("m3", "再继续");
    follow_up2.thread_id = Some("omt_1".to_string());
    let _ = handle(follow_up2).await.unwrap();
    assert_eq!(
        mock.thread_root_calls.lock().await.len(),
        1,
        "the root cache must skip the backing lookup"
    );
    assert!(
        store.find_mapping("mock", "omt_1").await.unwrap().is_none(),
        "still no thread-keyed row"
    );
    assert_eq!(
        store.find_mapping("mock", "m1").await.unwrap(),
        Some(sid),
        "still the /thread session"
    );
}

/// An in-thread trigger whose root has no session starts a fresh
/// thread-keyed session (a human-created thread the bot never joined).
#[tokio::test]
async fn test_thread_followup_without_root_mapping_starts_fresh() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    mock.thread_roots
        .lock()
        .await
        .insert("omt_9".to_string(), "om_stranger".to_string());

    let msg = ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_1".to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some("看看".to_string()),
        content: vec![ContentBlock::Text {
            text: "看看".to_string(),
        }],
        image_keys: vec![],
        thread_id: Some("omt_9".to_string()),
        root_id: None,
        parent_id: None,
        is_group: false,
        create_time: None,
        doc_comment: None,
    };
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        msg,
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(reply, None);
    assert!(
        store.find_mapping("mock", "omt_9").await.unwrap().is_some(),
        "a fresh session keys by the thread id"
    );
    assert!(
        store
            .find_mapping("mock", "om_stranger")
            .await
            .unwrap()
            .is_none(),
        "nothing keys by the unmapped root"
    );
}

/// `/thread` inside an existing thread is an error — the command
/// promises a new thread it can't create. Refusing (instead of
/// degrading to a plain trigger) also removes any temptation to fork
/// a parallel session into the same visible thread.
#[tokio::test]
async fn test_thread_command_inside_existing_thread_errors() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let in_thread = |msg_id: &str, raw: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_1".to_string(),
        external_message_id: Some(msg_id.to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![ContentBlock::Text {
            text: raw.to_string(),
        }],
        image_keys: vec![],
        thread_id: Some("omt_1".to_string()),
        root_id: Some("om_root".to_string()),
        parent_id: Some("om_root".to_string()),
        is_group: true,
        create_time: None,
        doc_comment: None,
    };
    let handle = |m: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            m,
            &obs,
            &adapter,
        )
    };

    // A plain in-thread trigger claims the thread (thread-id key).
    let reply = handle(in_thread("m1", "hi")).await.unwrap();
    assert_eq!(reply, None);
    let sid = store
        .find_mapping("mock", "omt_1")
        .await
        .unwrap()
        .expect("thread session");

    // `/thread` in the same thread: refused, no root-keyed fork, the
    // thread session untouched.
    let reply = handle(in_thread("m2", "/thread 继续")).await.unwrap();
    assert!(
        reply
            .as_deref()
            .is_some_and(|r| r.contains("Already in a thread")),
        "{reply:?}"
    );
    assert!(
        store
            .find_mapping("mock", "om_root")
            .await
            .unwrap()
            .is_none(),
        "in-thread /thread must not fork a root-keyed session"
    );
    assert_eq!(
        store.find_mapping("mock", "omt_1").await.unwrap(),
        Some(sid)
    );
}

/// Poll until the (async) session-title fallback lands, or time out.
async fn wait_for_title(
    kernel: &Arc<crate::kernel::Kernel>,
    sid: &crate::types::SessionId,
) -> String {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let session = kernel.get_session(sid).await.unwrap();
        if let Some(title) = session.title {
            return title;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "title not set in time"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// A channel trigger titles the session from the user's bare text —
/// not from the adapter's metadata header on msg.content, and not
/// from injected chat history merged ahead of it (the bugs behind
/// all-Untitled / garbage-titled channel sessions).
#[tokio::test]
async fn channel_trigger_titles_session_from_user_text_not_history() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    // History noise ("earlier"/"latest") must not leak into the title.
    let mock = Arc::new(HistoryMockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        ChannelMessage {
            external_chat_id: "oc_1".to_string(),
            external_user_id: "ou_1".to_string(),
            external_message_id: Some("trigger".to_string()),
            is_mention: true,
            raw_text: Some("标题应该是这句话".to_string()),
            // Production shape: the agent-facing content carries the
            // adapter's metadata header ahead of the user's text.
            content: vec![ContentBlock::Text {
                text: "[2026-08-04 10:00:00][from_user_id: ou_1][chat_id: oc_1][platform: feishu]\n标题应该是这句话".to_string(),
            }],
            image_keys: vec![],
            thread_id: None,
            root_id: None,
            parent_id: None,
            is_group: true,
            create_time: Some(300),
            doc_comment: None,
        },
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(reply, None);
    let sid = store
        .find_mapping("mock", "oc_1")
        .await
        .unwrap()
        .expect("session created");
    let title = wait_for_title(&kernel, &sid).await;
    assert_eq!(title, "标题应该是这句话");
    kernel.stop().await;
}

/// An image-only first message (no raw text) gets no title input at
/// all — a header-only content block must not become the title.
#[tokio::test]
async fn image_only_trigger_leaves_session_untitled() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        ChannelMessage {
            external_chat_id: "oc_1".to_string(),
            external_user_id: "ou_1".to_string(),
            external_message_id: Some("m1".to_string()),
            is_mention: true,
            raw_text: None,
            content: vec![ContentBlock::Text {
                text:
                    "[2026-08-04 10:00:00][from_user_id: ou_1][chat_id: oc_1][platform: feishu]\n"
                        .to_string(),
            }],
            image_keys: vec!["img_1".to_string()],
            thread_id: None,
            root_id: None,
            parent_id: None,
            is_group: false,
            create_time: None,
            doc_comment: None,
        },
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(reply, None);
    let sid = store
        .find_mapping("mock", "oc_1")
        .await
        .unwrap()
        .expect("session created");

    // Give any (misguided) title task a chance to run; none should fire.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(kernel.get_session(&sid).await.unwrap().title.is_none());
    kernel.stop().await;
}

/// `/thread <text>` titles the session by the payload text, without
/// the command token.
#[tokio::test]
async fn thread_command_titles_session_from_payload_text() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        ChannelMessage {
            external_chat_id: "oc_1".to_string(),
            external_user_id: "ou_1".to_string(),
            external_message_id: Some("m1".to_string()),
            is_mention: true,
            raw_text: Some("/thread 看看这个".to_string()),
            content: vec![ContentBlock::Text {
                text: "/thread 看看这个".to_string(),
            }],
            image_keys: vec![],
            thread_id: None,
            root_id: None,
            parent_id: None,
            is_group: true,
            create_time: None,
            doc_comment: None,
        },
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(reply, None);
    let sid = store
        .find_mapping("mock", "m1")
        .await
        .unwrap()
        .expect("session created");
    let title = wait_for_title(&kernel, &sid).await;
    assert_eq!(title, "看看这个");
    kernel.stop().await;
}

/// `yomi channel new-thread`: posts the anchor, creates a session keyed
/// by it (in-thread follow-ups adopt it), injects the task — and with a
/// `--title`, the task is posted separately as the thread opener.
#[tokio::test]
async fn channel_new_thread_runs_task_in_session_keyed_by_anchor() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    mock.issue_ids
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let hub = ChannelHub::new(Arc::clone(&store));
    hub.instances.insert(
        "mock".to_string(),
        ChannelInstance {
            config: config.clone(),
            status: Arc::new(AtomicU8::new(STATUS_IDLE)),
            adapter: Arc::clone(&adapter),
        },
    );

    // Default: the task text itself is the anchor.
    let out = hub
        .create_thread_in_chat(&kernel, None, "feishu", "oc_1", None, "调研 X")
        .await
        .unwrap();
    assert_eq!(out["root_id"].as_str(), Some("msg-1"));
    assert_eq!(out["thread_url"].as_str(), Some("link://oc_1/msg-1"));
    assert_eq!(last_outgoing_text(&mock).await, "调研 X");
    let sid = out["session_id"].as_str().unwrap().to_string();
    let mapped = store
        .find_mapping("mock", "msg-1")
        .await
        .unwrap()
        .expect("session created");
    assert_eq!(mapped.0.as_str(), sid);
    let title = wait_for_title(&kernel, &SessionId::from(sid.clone())).await;
    assert_eq!(title, "调研 X");

    // With --title: the root carries the title, the task opens the thread.
    let out = hub
        .create_thread_in_chat(&kernel, None, "feishu", "oc_1", Some("📌 X"), "调研 X 详情")
        .await
        .unwrap();
    assert_eq!(out["root_id"].as_str(), Some("msg-2"));
    let outgoing = mock.outgoing.lock().await;
    let texts: Vec<String> = outgoing
        .iter()
        .filter_map(|(_, blocks)| match blocks.first() {
            Some(ContentBlock::Text { text }) => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["调研 X", "📌 X", "调研 X 详情"]);
    drop(outgoing);

    // Resolution errors: unknown channel, unsupported platform.
    assert!(hub
        .create_thread_in_chat(&kernel, Some("nope"), "feishu", "oc_1", None, "x")
        .await
        .is_err());
    hub.instances.insert(
        "tg".to_string(),
        ChannelInstance {
            config: ChannelConfig {
                name: "tg".to_string(),
                platform: PlatformConfig::Telegram {
                    token: "fake".into(),
                },
                ..config.clone()
            },
            status: Arc::new(AtomicU8::new(STATUS_IDLE)),
            adapter: Arc::clone(&adapter),
        },
    );
    let err = hub
        .create_thread_in_chat(&kernel, Some("tg"), "telegram", "chat", None, "x")
        .await;
    assert!(err.is_err(), "telegram has no threads");
    kernel.stop().await;
}

/// `/thread` works in private chats too (Feishu threads exist there):
/// the session keys by the command's message id — never hijacking the
/// chat-level session — and in-thread follow-ups adopt it. Telegram
/// has no threads and is refused.
#[tokio::test]
async fn test_thread_command_private_chat_and_platform_gate() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let msg = |msg_id: &str, raw: &str, is_group: bool| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_1".to_string(),
        external_message_id: Some(msg_id.to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![ContentBlock::Text {
            text: raw.to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group,
        create_time: None,
        doc_comment: None,
    };
    let feishu = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    // Private chat: the one-shot thread keys by the command's message
    // id, leaving the chat-level session key alone.
    let reply = handle_incoming_message(
        "mock",
        &feishu,
        &store,
        Arc::clone(&kernel),
        msg("m1", "/thread hi", false),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(reply, None);
    let sid = store
        .find_mapping("mock", "m1")
        .await
        .unwrap()
        .expect("private /thread session under the message id");
    assert!(store.find_mapping("mock", "oc_1").await.unwrap().is_none());

    // A private in-thread follow-up adopts it.
    let mut follow_up = msg("m2", "继续", false);
    follow_up.thread_id = Some("omt_1".to_string());
    follow_up.root_id = Some("m1".to_string());
    follow_up.parent_id = Some("om_bot".to_string());
    let reply = handle_incoming_message(
        "mock",
        &feishu,
        &store,
        Arc::clone(&kernel),
        follow_up,
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(reply, None);
    assert!(
        store.find_mapping("mock", "omt_1").await.unwrap().is_none(),
        "no thread-keyed mapping row (unified root key)"
    );
    assert_eq!(store.find_mapping("mock", "m1").await.unwrap(), Some(sid));

    // Telegram group: no thread support.
    let telegram = ChannelConfig {
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        ..feishu.clone()
    };
    let reply = handle_incoming_message(
        "mock",
        &telegram,
        &store,
        Arc::clone(&kernel),
        msg("m3", "/thread hi", true),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(
        reply.as_deref(),
        Some("This platform does not support threads.")
    );
    assert!(store.find_mapping("mock", "m3").await.unwrap().is_none());
}

#[test]
fn test_longer_words_are_not_commands() {
    // Exact matching must not hijack longer words ("/clearance" would
    // trigger the destructive /clear otherwise) — they report as unknown
    // commands instead of reaching the agent.
    assert!(matches!(
        parse_channel_command(Some("/clearance")),
        ChannelCommand::Unknown(_)
    ));
    assert!(matches!(
        parse_channel_command(Some("/helpful")),
        ChannelCommand::Unknown(_)
    ));
    assert!(matches!(
        parse_channel_command(Some("/stopping")),
        ChannelCommand::Unknown(_)
    ));
    assert!(matches!(
        parse_channel_command(Some("/information")),
        ChannelCommand::Unknown(_)
    ));
    // … but the @bot suffix still works.
    assert!(matches!(
        parse_channel_command(Some("/clear@yomi_bot")),
        ChannelCommand::Clear
    ));
}

fn model_info(name: &str, model_id: &str, context_window: u32) -> crate::kernel::ModelInfo {
    crate::kernel::ModelInfo {
        name: name.to_string(),
        model_id: model_id.to_string(),
        provider: "anthropic".to_string(),
        context_window,
    }
}

#[test]
fn test_format_model_list_marks_current_model() {
    let models = vec![
        model_info("claude", "claude-sonnet", 200_000),
        model_info("nova", "nova-2", 256_000),
    ];

    let output = format_model_list(&models, "nova");

    assert!(output.contains("`claude` · anthropic · `claude-sonnet` · 200k ctx"));
    assert!(output.contains("`nova` · anthropic · `nova-2` · 256k ctx **← current**"));
    assert!(output.contains("/model <model_key>"));
}

#[test]
fn test_format_current_and_unknown_model() {
    let models = vec![model_info("nova", "nova-2", 256_000)];

    let current = format_current_model(&models, "nova");
    assert!(current.contains("`nova` · anthropic · `nova-2` · 256k ctx"));

    let unknown = format_unknown_model("missing", &models);
    assert!(unknown.contains("Model `missing` was not found"));
    assert!(unknown.contains("Available model keys: `nova`"));
}

fn channel_message(
    thread_id: Option<&str>,
    is_group: bool,
    has_message_id: bool,
) -> ChannelMessage {
    ChannelMessage {
        external_chat_id: "chat-1".to_string(),
        external_user_id: "user-1".to_string(),
        external_message_id: has_message_id.then(|| "msg-1".to_string()),
        is_mention: true,
        raw_text: None,
        content: vec![],
        image_keys: vec![],
        thread_id: thread_id.map(str::to_string),
        root_id: None,
        parent_id: None,
        is_group,
        create_time: None,
        doc_comment: None,
    }
}

#[test]
fn reply_anchor_keeps_in_thread_replies_anchored() {
    let msg = channel_message(Some("thread-1"), true, true);
    // Regardless of the config, in-thread messages anchor to the trigger.
    assert_eq!(reply_anchor(&msg, false).as_deref(), Some("msg-1"));
    assert_eq!(reply_anchor(&msg, true).as_deref(), Some("msg-1"));
}

#[test]
fn reply_anchor_respects_reply_in_thread_config() {
    let group_msg = channel_message(None, true, true);
    assert_eq!(reply_anchor(&group_msg, false), None);
    assert_eq!(reply_anchor(&group_msg, true).as_deref(), Some("msg-1"));
}

#[test]
fn reply_anchor_never_anchors_private_chats() {
    let private_msg = channel_message(None, false, true);
    assert_eq!(reply_anchor(&private_msg, false), None);
    assert_eq!(reply_anchor(&private_msg, true), None);
}

#[test]
fn reply_anchor_requires_message_id() {
    let msg = channel_message(None, true, false);
    assert_eq!(reply_anchor(&msg, true), None);
}

#[test]
fn command_reply_anchor_chat_level_feedback_stays_in_main_flow() {
    // A top-level group message in reply_in_thread mode: command
    // feedback — including unknown/malformed commands — addresses the
    // chat as a whole and does not open a one-reply thread.
    let msg = channel_message(None, true, true);
    assert_eq!(
        command_reply_anchor(&msg, true, &ChannelCommand::Info),
        None
    );
    assert_eq!(
        command_reply_anchor(&msg, true, &ChannelCommand::Unknown("/infp".to_string())),
        None
    );
    assert_eq!(
        command_reply_anchor(&msg, true, &ChannelCommand::InvalidThreadsCommand),
        None
    );
}

#[test]
fn command_reply_anchor_chat_level_triggers_open_a_thread() {
    // Run triggers are the only chat-level messages whose reply anchors
    // — the anchor is what opens the conversation's thread.
    let msg = channel_message(None, true, true);
    for cmd in [
        ChannelCommand::None,
        ChannelCommand::Steer("x".to_string()),
        ChannelCommand::Queue("x".to_string()),
        ChannelCommand::Thread("x".to_string()),
    ] {
        assert_eq!(
            command_reply_anchor(&msg, true, &cmd).as_deref(),
            Some("msg-1")
        );
    }
}

#[test]
fn command_reply_anchor_elsewhere_matches_reply_anchor() {
    // In-thread commands stay anchored (the reply stays in the thread).
    let threaded = channel_message(Some("thread-1"), true, true);
    assert_eq!(
        command_reply_anchor(&threaded, true, &ChannelCommand::Info).as_deref(),
        Some("msg-1")
    );
    // reply_in_thread off: group feedback was never anchored.
    let group = channel_message(None, true, true);
    assert_eq!(
        command_reply_anchor(&group, false, &ChannelCommand::Info),
        None
    );
    // Private chats never anchor.
    let private = channel_message(None, false, true);
    assert_eq!(
        command_reply_anchor(&private, true, &ChannelCommand::Info),
        None
    );
}

#[test]
fn command_session_key_chat_level_addresses_the_chat() {
    // reply_in_thread keys top-level messages by their own id — a scope
    // no follow-up ever reaches; session commands there address the chat.
    let msg = channel_message(None, true, true);
    assert_eq!(command_session_key(&msg, true, "chat-1", "msg-1"), "chat-1");
}

#[test]
fn command_session_key_elsewhere_addresses_the_conversation() {
    let mut threaded = channel_message(Some("thread-1"), true, true);
    threaded.root_id = Some("root-1".to_string());
    assert_eq!(
        command_session_key(&threaded, true, "chat-1", "root-1"),
        "root-1"
    );
    // reply_in_thread off: unchanged (the mapping key already is the chat).
    let top = channel_message(None, true, true);
    assert_eq!(
        command_session_key(&top, false, "chat-1", "chat-1"),
        "chat-1"
    );
    let private = channel_message(None, false, true);
    assert_eq!(
        command_session_key(&private, true, "chat-1", "chat-1"),
        "chat-1"
    );
}

#[test]
fn status_usage_command_parse() {
    assert!(matches!(
        parse_channel_command(Some("/status")),
        ChannelCommand::Status
    ));
    assert!(matches!(
        parse_channel_command(Some("/usage")),
        ChannelCommand::Usage(7)
    ));
    assert!(matches!(
        parse_channel_command(Some("/u 30")),
        ChannelCommand::Usage(30)
    ));
    assert!(matches!(
        parse_channel_command(Some("/usage@yomi_bot 90")),
        ChannelCommand::Usage(90)
    ));
    for raw in ["/usage 0", "/usage 91", "/usage abc", "/usage 7 extra"] {
        assert!(
            matches!(
                parse_channel_command(Some(raw)),
                ChannelCommand::InvalidUsageCommand
            ),
            "{raw}"
        );
    }
    // No-arg commands with extra args are not commands (existing
    // convention: they pass through as message text).
    assert!(matches!(
        parse_channel_command(Some("/status now")),
        ChannelCommand::None
    ));
}

#[test]
fn format_runtime_status_lines() {
    let boot = chrono::Utc::now() - chrono::Duration::hours(3);
    let body = format_runtime_status(
        boot,
        2,
        3,
        1,
        Some(6),
        &[ChannelInfo {
            name: "feishu".to_string(),
            status: ChannelStatus::Connecting,
        }],
    );
    assert!(body.contains("up 3h"), "{body}");
    assert!(body.contains("**Active runs**: 2"), "{body}");
    assert!(body.contains("**Background shells**: 3"), "{body}");
    assert!(body.contains("**Running subagents**: 1"), "{body}");
    assert!(body.contains("**Cron jobs**: 6 active"), "{body}");
    assert!(body.contains("feishu (up)"), "{body}");

    // A cleanly-exited receiver renders as stopped.
    let body = format_runtime_status(
        boot,
        0,
        0,
        0,
        None,
        &[ChannelInfo {
            name: "tg".to_string(),
            status: ChannelStatus::Idle,
        }],
    );
    assert!(body.contains("tg (stopped)"), "{body}");

    // No cron store / no channels: those lines are omitted entirely.
    let body = format_runtime_status(boot, 0, 0, 0, None, &[]);
    assert!(!body.contains("Cron jobs"), "{body}");
    assert!(!body.contains("Channels"), "{body}");
}

#[test]
fn format_usage_sections() {
    let summary = crate::storage::usage::UsageSummary {
        prompt_tokens: 8_000_000,
        completion_tokens: 4_400_000,
        cached_tokens: 3_100_000,
        request_count: 482,
    };
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    let day = |date: &str, total: u64, req: u64| crate::storage::usage::DailyUsage {
        date: date.to_string(),
        prompt_tokens: total / 2,
        completion_tokens: total / 2,
        cached_tokens: 0,
        request_count: req,
        models: vec![],
    };
    let daily = vec![day("2026-08-24", 2_100_000, 88), day(&today, 1_800_000, 67)];
    let models = vec![crate::storage::usage::ModelUsage {
        model: "k3-hs".to_string(),
        provider: "openai".to_string(),
        prompt_tokens: 6_000_000,
        completion_tokens: 2_200_000,
        cached_tokens: 0,
        request_count: 301,
    }];
    let body = format_usage(7, &summary, &daily, &models);
    assert!(
        body.contains("**Total (7d)**: 12.4m tok (3.1m cached) · 482 req"),
        "{body}"
    );
    assert!(body.contains("**Today**: 1.8m tok · 67 req"), "{body}");
    assert!(body.contains("`k3-hs` · 8.2m tok · 301 req"), "{body}");
    assert!(body.contains("08-24 · 2.1m tok · 88 req"), "{body}");

    // The latest daily row is not today → no Today line.
    let body = format_usage(7, &summary, &daily[..1], &models);
    assert!(!body.contains("**Today**"), "{body}");
}

#[tokio::test]
async fn test_is_channel_session() {
    let (_pool, store) = create_test_pool().await;
    let hub = ChannelHub::new(store.clone());

    let sid = SessionId::new();
    assert!(!hub.is_channel_session(&sid).await);

    store
        .save_mapping("tg_bot", "12345", &sid, "chat123", None)
        .await
        .unwrap();
    assert!(hub.is_channel_session(&sid).await);

    // Unrelated session remains non-channel.
    assert!(!hub.is_channel_session(&SessionId::new()).await);
}

#[test]
fn mapping_key_reply_in_thread_top_level_message_starts_new_session() {
    // Top-level group message: no root_id/thread_id yet (the thread is only
    // opened by the bot's reply), so it keys by its own message id.
    let msg = channel_message(None, true, true);
    assert_eq!(session_mapping_key(&msg, "chat-1", true), "msg-1");
}

#[test]
fn mapping_key_reply_in_thread_follow_up_joins_root_session() {
    // In-thread message: Feishu sets root_id to the thread's root message,
    // so the follow-up joins the session started by that message.
    let mut msg = channel_message(Some("thread-1"), true, true);
    msg.root_id = Some("msg-root".to_string());
    assert_eq!(session_mapping_key(&msg, "chat-1", true), "msg-root");
}

#[test]
fn mapping_key_reply_in_thread_legacy_thread_falls_back_to_thread_id() {
    // Thread message without root_id (older data / unusual shapes) still
    // keys by thread_id.
    let msg = channel_message(Some("thread-1"), true, true);
    assert_eq!(session_mapping_key(&msg, "chat-1", true), "thread-1");
}

#[test]
fn mapping_key_reply_in_thread_private_chat_stays_chat_scoped() {
    // Private chats never key by message, even for quote-replies (root_id).
    let mut msg = channel_message(None, false, true);
    msg.root_id = Some("msg-root".to_string());
    assert_eq!(session_mapping_key(&msg, "chat-1", true), "chat-1");
}

#[test]
fn mapping_key_reply_in_thread_quote_reply_starts_own_session() {
    // Plain quote-reply (root_id set, but NOT inside any thread): keys by
    // its own message id so it starts a fresh session — the bot's
    // reply_in_thread answer opens a new thread anchored at this message,
    // whose follow-ups then carry root_id = this message's id.
    let mut msg = channel_message(None, true, true);
    msg.root_id = Some("old-msg".to_string());
    assert_eq!(session_mapping_key(&msg, "chat-1", true), "msg-1");
}

#[test]
fn mapping_key_without_reply_in_thread_unchanged() {
    // Quote-reply in a group with reply_in_thread off: root_id is ignored.
    let mut msg = channel_message(None, true, true);
    msg.root_id = Some("msg-root".to_string());
    assert_eq!(session_mapping_key(&msg, "chat-1", false), "chat-1");

    // Thread messages still key by thread_id as before.
    let msg = channel_message(Some("thread-1"), true, true);
    assert_eq!(session_mapping_key(&msg, "chat-1", false), "thread-1");
}

fn doc_comment_msg() -> ChannelMessage {
    let mut msg = channel_message(None, false, false);
    msg.external_chat_id = String::new();
    msg.doc_comment = Some(crate::channels::DocCommentRef {
        file_token: "tok123".to_string(),
        file_type: "docx".to_string(),
        comment_id: "c_1".to_string(),
    });
    msg
}

#[test]
fn mapping_key_doc_comment_keys_by_comment_thread() {
    let msg = doc_comment_msg();
    // One session per comment thread — regardless of reply_in_thread.
    assert_eq!(session_mapping_key(&msg, "", false), "doc:docx:tok123:c_1");
    assert_eq!(session_mapping_key(&msg, "", true), "doc:docx:tok123:c_1");

    // Whole-document comments (sentinel id, set in comment.rs) share one
    // session per document.
    let mut msg = doc_comment_msg();
    msg.doc_comment.as_mut().unwrap().comment_id = crate::channels::WHOLE_COMMENT_ID.to_string();
    assert_eq!(
        session_mapping_key(&msg, "", false),
        "doc:docx:tok123:whole"
    );
}

#[test]
fn doc_comment_messages_parse_commands() {
    // Commands work in doc comments; the reply routes into the comment
    // thread (see send_command_reply), not any chat.
    let mut msg = doc_comment_msg();
    msg.raw_text = Some("/clear".to_string());
    assert!(matches!(
        parse_channel_command(msg.raw_text.as_deref()),
        ChannelCommand::Clear
    ));
    // Plain comment text is not a command.
    msg.raw_text = Some("这句话怎么样".to_string());
    assert!(matches!(
        parse_channel_command(msg.raw_text.as_deref()),
        ChannelCommand::None
    ));
}

#[tokio::test]
async fn command_reply_for_doc_comment_goes_to_comment_thread() {
    let mock = Arc::new(MockAdapter::new("feishu"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let msg = doc_comment_msg();

    send_command_reply(&adapter, &msg, None, "🧹 Context cleared.".to_string())
        .await
        .unwrap();

    let replies = mock.comment_replies.lock().await;
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].0, "c_1");
    assert_eq!(replies[0].1, "🧹 Context cleared.");
    assert!(mock.outgoing.lock().await.is_empty(), "no chat message");
}

#[tokio::test]
async fn command_reply_for_chat_message_goes_to_chat() {
    let mock = Arc::new(MockAdapter::new("feishu"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let msg = channel_message(None, false, true);

    send_command_reply(&adapter, &msg, None, "🧹 Context cleared.".to_string())
        .await
        .unwrap();

    let outgoing = mock.outgoing.lock().await;
    assert_eq!(outgoing.len(), 1);
    assert_eq!(outgoing[0].0, "chat-1");
    assert!(mock.comment_replies.lock().await.is_empty());
}

#[tokio::test]
async fn deliver_reply_doc_comment_goes_to_comment_thread() {
    let mock = Arc::new(MockAdapter::new("feishu"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();
    let routing = SessionRouting {
        channel_name: "feishu".to_string(),
        external_chat_id: String::new(),
        reply_msg_id: None,
        mapping_key: "doc:docx:tok123:c_1".to_string(),
        doc_comment: crate::channels::parse_doc_comment_mapping_key("doc:docx:tok123:c_1"),
    };

    let delivered = deliver_reply(
        &obs,
        &adapter,
        &routing,
        Some(run_buffer().into_reply()),
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        &std::sync::Weak::new(),
    )
    .await;

    assert_eq!(delivered.as_deref(), Some("reply-1"));
    let replies = mock.comment_replies.lock().await;
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0].0, "c_1");
    assert!(replies[0].1.contains("final answer"));
    // Nothing went to any chat surface.
    assert!(mock.outgoing.lock().await.is_empty());
}

#[tokio::test]
async fn deliver_reply_doc_comment_chunks_long_text() {
    let mock = Arc::new(MockAdapter::new("feishu"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();
    let routing = SessionRouting {
        channel_name: "feishu".to_string(),
        external_chat_id: String::new(),
        reply_msg_id: None,
        mapping_key: "doc:docx:tok123:c_1".to_string(),
        doc_comment: crate::channels::parse_doc_comment_mapping_key("doc:docx:tok123:c_1"),
    };
    let mut buf = reply::RunReplyBuffer::new();
    buf.record_model_end(&"字".repeat(4500));

    deliver_reply(
        &obs,
        &adapter,
        &routing,
        Some(buf.into_reply()),
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        &std::sync::Weak::new(),
    )
    .await;

    let replies = mock.comment_replies.lock().await;
    assert_eq!(replies.len(), 2);
    assert_eq!(replies[0].1.chars().count(), 4000);
    assert_eq!(replies[1].1.chars().count(), 500);
}

#[test]
fn chat_level_message_only_for_top_level_group_in_thread_mode() {
    // Top-level group message in reply_in_thread mode → chat-level.
    let msg = channel_message(None, true, true);
    assert!(is_chat_level_message(&msg, true));

    // In-thread message → thread session.
    let mut msg = channel_message(Some("thread-1"), true, true);
    assert!(!is_chat_level_message(&msg, true));
    // Quote-reply (root_id set, no thread) → its own new session, not
    // chat-level.
    msg.root_id = Some("msg-root".to_string());
    assert!(!is_chat_level_message(&msg, true));

    // Private chat → never chat-level.
    let msg = channel_message(None, false, true);
    assert!(!is_chat_level_message(&msg, true));

    // reply_in_thread off → never chat-level.
    let msg = channel_message(None, true, true);
    assert!(!is_chat_level_message(&msg, false));
}

#[test]
fn test_parse_info_command() {
    assert!(matches!(
        parse_channel_command(Some("/info")),
        ChannelCommand::Info
    ));
    assert!(matches!(
        parse_channel_command(Some("/info@yomi_bot")),
        ChannelCommand::Info
    ));
    assert!(matches!(
        parse_channel_command(Some("/info extra")),
        ChannelCommand::None
    ));
}

#[test]
fn test_parse_approval_commands() {
    assert!(matches!(
        parse_channel_command(Some("/permits")),
        ChannelCommand::Permits
    ));
    assert!(matches!(
        parse_channel_command(Some("/permits@yomi_bot")),
        ChannelCommand::Permits
    ));
    assert!(matches!(
        parse_channel_command(Some("/approve 3")),
        ChannelCommand::Approve { id: 3, perm: None }
    ));
    assert!(matches!(
        parse_channel_command(Some("/approve 3 edit")),
        ChannelCommand::Approve {
            id: 3,
            perm: Some(_)
        }
    ));
    assert!(matches!(
        parse_channel_command(Some("/deny 3")),
        ChannelCommand::Deny { id: 3 }
    ));

    // Malformed: missing/invalid ids, extra arguments.
    assert!(matches!(
        parse_channel_command(Some("/approve")),
        ChannelCommand::InvalidApprovalCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/approve abc")),
        ChannelCommand::InvalidApprovalCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/approve 3 edit extra")),
        ChannelCommand::InvalidApprovalCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/deny")),
        ChannelCommand::InvalidApprovalCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/deny 3 4")),
        ChannelCommand::InvalidApprovalCommand
    ));
    // Prefix lookalikes report as unknown commands.
    assert!(matches!(
        parse_channel_command(Some("/approved 3")),
        ChannelCommand::Unknown(_)
    ));
}

#[test]
fn test_format_session_info() {
    let now = chrono::Utc::now();
    let session = crate::types::SessionResponse {
        id: SessionId::new(),
        phase: "idle".to_string(),
        title: None,
        parent_id: None,
        project_id: None,
        working_dir: None,
        message_count: 7,
        created_at: now - chrono::Duration::hours(3),
        updated_at: now - chrono::Duration::minutes(5),
        auto_approve_level: Some("dangerous".to_string()),
        model_key: None,
        template: None,
    };
    let models = vec![model_info("nova", "nova-2", 256_000)];

    let out = format_session_info(&session, "nova", &models, 0, &[], Some(45_056));
    assert!(out.contains(&format!("- **ID**: `{}`", session.id.0)));
    assert!(out.contains("- **Model**: `nova` · anthropic · `nova-2` · 256k ctx (default)"));
    assert!(out.contains("- **Context**: 45.1k/256.0k (18%)"));
    assert!(out.contains("- **Status**: idle"));
    assert!(out.contains("- **Created**: 3h ago · **Active**: 5m ago"));
    assert!(out.contains("- **Permission**: dangerous"));
    assert!(out.contains("- **Subagents**: 0 running"));
    assert!(out.contains("- **Background shells**: 0"));

    // Persisted model key drops the (default) marker; shells are listed.
    let session = crate::types::SessionResponse {
        model_key: Some("nova".to_string()),
        ..session
    };
    let shells = vec![crate::agent::BackgroundShellTask {
        task_id: "sh-1".to_string(),
        session_id: session.id.clone(),
        pid: 42,
        command: "cargo test".to_string(),
        output_path: "/tmp/sh-1.log".to_string(),
        started_at: now - chrono::Duration::minutes(9),
    }];
    let out = format_session_info(&session, "nova", &models, 2, &shells, None);
    assert!(out.contains("- **Model**: `nova` · anthropic · `nova-2` · 256k ctx\n"));
    assert!(out.contains("- **Context**: —"));
    assert!(out.contains("- **Subagents**: 2 running"));
    assert!(out.contains("- **Background shells**: 1\n  - `cargo test` · pid 42 · 9m ago"));
}

#[tokio::test]
async fn session_context_tokens_picks_latest_usage() {
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
    let sid = SessionId::new();

    // Nothing recorded → unknown (`—` on the card).
    assert_eq!(kernel.get_session_context_tokens(&sid).await, None);

    let usage = |prompt_tokens: u32, completion_tokens: u32, total_tokens: u32| {
        crate::types::MessageTokenUsage {
            prompt_tokens,
            completion_tokens,
            total_tokens,
        }
    };
    let store = kernel.message_store().await;
    store
        .append(
            &sid.0,
            &[
                crate::types::Message::user("hi"),
                crate::types::Message::assistant("a").with_token_usage(usage(100, 20, 120)),
                crate::types::Message::assistant("b").with_token_usage(usage(200, 30, 230)),
                crate::types::Message::user("again"),
            ],
        )
        .await
        .unwrap();
    assert_eq!(kernel.get_session_context_tokens(&sid).await, Some(230));
}

#[tokio::test]
async fn permits_denies_non_admin_without_card() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "app".to_string(),
            app_secret: "secret".to_string(),
        },
        require_mention: true,
        admin_users: vec!["ou_admin".to_string()],
        ..Default::default()
    };
    let mut msg = channel_message(None, true, true);
    msg.external_user_id = "ou_random".to_string();
    msg.raw_text = Some("/permits".to_string());
    let reply = handle_incoming_message("mock", &config, &store, kernel, msg, &obs, &adapter)
        .await
        .unwrap();
    assert_eq!(
        reply.as_deref(),
        Some("Permission denied: not in admin_users.")
    );
    assert!(mock.cards.lock().await.is_empty());
}

// ── deliver_reply ───────────────────────────────────────────────────

#[tokio::test]
async fn info_reply_card_on_capable_platforms_text_elsewhere() {
    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let msg = channel_message(None, true, true);

    // Text fallback (default): the title becomes a bold first line.
    send_info_reply(
        &adapter,
        &msg,
        None,
        "ℹ️ Session info",
        "- **A**: 1".to_string(),
    )
    .await
    .unwrap();
    let text = last_outgoing_text(&mock).await;
    assert!(
        text.starts_with("**ℹ️ Session info**\n\n- **A**: 1"),
        "text fallback: {text}"
    );
    assert!(mock.cards.lock().await.is_empty());

    // Card path: the header carries the title, the body the markdown.
    mock.status_card_ok
        .store(true, std::sync::atomic::Ordering::Relaxed);
    send_info_reply(
        &adapter,
        &msg,
        None,
        "ℹ️ Session info",
        "- **A**: 1".to_string(),
    )
    .await
    .unwrap();
    let cards = mock.cards.lock().await;
    let Some((_, card, _)) = cards.last() else {
        panic!("no card sent")
    };
    assert!(card.contains("\"template\":\"blue\""), "card: {card}");
    assert!(card.contains("ℹ️ Session info"), "card: {card}");
    assert!(card.contains("- **A**: 1"), "card: {card}");
}

use crate::event::{AgentEvent, AgentStatus, StopReason, ToolEvent};

fn running_event() -> crate::event::Event {
    crate::event::Event::Agent(AgentEvent::Lifecycle {
        state: AgentStatus::Running,
    })
}

fn tool_start_event() -> crate::event::Event {
    crate::event::Event::Tool(ToolEvent::Start {
        message_id: crate::types::MessageId::new(),
        tool_id: "t1".to_string(),
        tool_name: "bash".to_string(),
        arguments: None,
    })
}

fn completed() -> StopReason {
    StopReason::Completed {
        finish_reason: None,
    }
}

#[tokio::test]
async fn deliver_reply_morphs_status_card_when_no_mid_run_posts() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    // Drive obs to materialize the status card.
    obs.handle_event(&adapter, &sid, "chat-1", None, &running_event())
        .await;
    obs.handle_event(&adapter, &sid, "chat-1", None, &tool_start_event())
        .await;
    assert_eq!(mock.cards.lock().await.len(), 1, "materialized");

    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(run_buffer().into_reply()),
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        // Attachment delivery is out of scope for these tests; a dangling
        // Weak keeps the workspace lookup inert.
        &std::sync::Weak::new(),
    )
    .await;

    // The status card is PATCHed into the final reply — no new message.
    assert_eq!(mock.cards.lock().await.len(), 1, "no extra card");
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("final answer"));
    assert!(patches[0].1.contains("collapsible_panel"));
}

#[tokio::test]
async fn deliver_reply_freezes_card_and_flushes_new_message_on_mid_run_posts() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    obs.handle_event(&adapter, &sid, "chat-1", None, &running_event())
        .await;
    obs.handle_event(&adapter, &sid, "chat-1", None, &tool_start_event())
        .await;
    // A mid-run platform message (receipts are recorded only while running).
    obs.record_receipt(&sid, "m1".to_string());
    obs.record_receipt(&sid, "m2".to_string());

    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(run_buffer().into_reply()),
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        // Attachment delivery is out of scope for these tests; a dangling
        // Weak keeps the workspace lookup inert.
        &std::sync::Weak::new(),
    )
    .await;

    // The old card freezes in place as a terminal receipt — stats only,
    // no trace panel (the reply message carries it now) …
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("✅ **Done**"), "frozen terminal card");
    assert!(
        !patches[0].1.contains("collapsible_panel"),
        "stats-only receipt: the trace rides the reply message"
    );
    drop(patches);
    // … and the reply lands at the bottom as a NEW card message carrying
    // the run trace.
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 2, "materialize + reply card");
    assert!(cards[1].1.contains("final answer"));
    assert!(
        cards[1].1.contains("collapsible_panel") && cards[1].1.contains("🐾"),
        "reply card carries the trace panel"
    );
    assert!(mock.outgoing.lock().await.is_empty(), "no bare-text flush");
}

#[tokio::test]
async fn deliver_reply_mid_run_without_text_keeps_trace_on_frozen_card() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    obs.handle_event(&adapter, &sid, "chat-1", None, &running_event())
        .await;
    obs.handle_event(&adapter, &sid, "chat-1", None, &tool_start_event())
        .await;
    obs.record_receipt(&sid, "m1".to_string());

    // A run that produced tool calls but no assistant text: nothing can
    // be flushed, so the frozen card keeps the trace panel itself.
    let mut buf = reply::RunReplyBuffer::new();
    buf.record_tool_start("t1", "shell", Some(r#"{"command":"cargo test"}"#));
    buf.record_tool_end("t1", 2000, false);

    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(buf.into_reply()),
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        &std::sync::Weak::new(),
    )
    .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("✅ **Done**"), "frozen terminal card");
    assert!(
        patches[0].1.contains("collapsible_panel") && patches[0].1.contains("🐾"),
        "no reply to carry the trace — the card keeps it"
    );
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "materialize only; no text to flush");
    assert!(mock.outgoing.lock().await.is_empty());
}

#[tokio::test]
async fn deliver_reply_mid_run_split_disabled_morphs_card() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    obs.handle_event(&adapter, &sid, "chat-1", None, &running_event())
        .await;
    obs.handle_event(&adapter, &sid, "chat-1", None, &tool_start_event())
        .await;
    obs.record_receipt(&sid, "m1".to_string());

    // mid_run_split = false: always morph, one message per run.
    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(run_buffer().into_reply()),
        true,
        true,
        false,
        &sid,
        SettleKind::Stopped(&completed()),
        &std::sync::Weak::new(),
    )
    .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1, "morphed in place");
    assert!(patches[0].1.contains("final answer"));
    assert!(patches[0].1.contains("collapsible_panel"));
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "materialize only; no new message");
    assert!(mock.outgoing.lock().await.is_empty());
}

#[tokio::test]
async fn deliver_reply_mid_run_split_disabled_reacts_on_morph() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    obs.record_user_msg(&sid, "user-msg-1".to_string());
    obs.handle_event(&adapter, &sid, "chat-1", None, &running_event())
        .await;
    obs.handle_event(&adapter, &sid, "chat-1", None, &tool_start_event())
        .await;
    obs.record_receipt(&sid, "m1".to_string());

    // mid_run_split = false: the morph is silent (in-place PATCH above
    // the user's mid-run posts), so the receipts must not suppress the
    // settle reaction — it is the only completion signal.
    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(run_buffer().into_reply()),
        true,
        true,
        false,
        &sid,
        SettleKind::Stopped(&completed()),
        &std::sync::Weak::new(),
    )
    .await;

    let reactions = mock.reactions.lock().await;
    assert_eq!(
        reactions.as_slice(),
        &[("user-msg-1".to_string(), "DONE".to_string())]
    );
}

#[tokio::test]
async fn deliver_reply_mid_run_trace_disabled_keeps_panel_on_card() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    obs.handle_event(&adapter, &sid, "chat-1", None, &running_event())
        .await;
    obs.handle_event(&adapter, &sid, "chat-1", None, &tool_start_event())
        .await;
    obs.record_receipt(&sid, "m1".to_string());

    // tool_trace = false: the flush is bare text without the trace, so
    // the frozen card keeps the trace panel itself.
    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(run_buffer().into_reply()),
        false,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        &std::sync::Weak::new(),
    )
    .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("✅ **Done**"));
    assert!(
        patches[0].1.contains("collapsible_panel"),
        "trace stays on the card: {patches:?}"
    );
    drop(patches);
    let outgoing = mock.outgoing.lock().await;
    assert_eq!(outgoing.len(), 1, "bare-text flush");
    assert!(outgoing[0].contains("final answer"));
    assert!(!outgoing[0].contains("🐾"));
}

#[tokio::test]
async fn deliver_reply_mid_run_without_reply_keeps_trace_on_card() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    obs.handle_event(&adapter, &sid, "chat-1", None, &running_event())
        .await;
    obs.handle_event(&adapter, &sid, "chat-1", None, &tool_start_event())
        .await;
    obs.record_receipt(&sid, "m1".to_string());

    // No reply at all (crash / lost events): nothing to flush, the
    // frozen card keeps the trace panel.
    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        None,
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        &std::sync::Weak::new(),
    )
    .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("collapsible_panel"));
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "materialize only");
    assert!(mock.outgoing.lock().await.is_empty());
}

#[tokio::test]
async fn deliver_reply_mid_run_flush_failure_keeps_trace_on_card() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    obs.handle_event(&adapter, &sid, "chat-1", None, &running_event())
        .await;
    obs.handle_event(&adapter, &sid, "chat-1", None, &tool_start_event())
        .await;
    obs.record_receipt(&sid, "m1".to_string());

    // Platform outage after materialization: the reply-card send and its
    // plain-text fallback both fail — the frozen card must keep the
    // trace panel (the reply never carried it).
    mock.fail_sends
        .store(true, std::sync::atomic::Ordering::Relaxed);
    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(run_buffer().into_reply()),
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        &std::sync::Weak::new(),
    )
    .await;

    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1, "frozen in place");
    assert!(patches[0].1.contains("✅ **Done**"));
    assert!(
        patches[0].1.contains("collapsible_panel") && patches[0].1.contains("🐾"),
        "flush failed — trace stays on the card: {patches:?}"
    );
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "materialize only; flush failed");
    assert!(mock.outgoing.lock().await.is_empty());
}

#[tokio::test]
async fn deliver_reply_plain_platform_flushes_without_morph() {
    let mock = Arc::new(MockAdapter::new("tg"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(run_buffer().into_reply()),
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        // Attachment delivery is out of scope for these tests; a dangling
        // Weak keeps the workspace lookup inert.
        &std::sync::Weak::new(),
    )
    .await;

    let out = mock.outgoing.lock().await;
    assert_eq!(out.len(), 1);
    let ContentBlock::Text { text } = &out[0].1[0] else {
        panic!("expected text block");
    };
    assert!(text.contains("final answer"));
    assert!(text.contains("🐾 0s · 💬 2"));
}

#[tokio::test]
async fn deliver_reply_falls_back_to_flush_when_obs_state_missing() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    // No `Running` was ever fed (event lost): the morph has nothing to
    // settle — the reply must still be delivered as a plain flush.
    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(run_buffer().into_reply()),
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        // Attachment delivery is out of scope for these tests; a dangling
        // Weak keeps the workspace lookup inert.
        &std::sync::Weak::new(),
    )
    .await;

    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "fallback flush via send_card");
    assert!(cards[0].1.contains("final answer"));
}

// ── History context injection ───────────────────────────────────────

#[test]
fn assemble_history_formats_chronological_capped_lines() {
    let messages = [
        HistoryMessage {
            message_id: "m1".into(),
            create_time: 1_700_000_000,
            sender_id: "ou_alice".into(),
            text: "  hello world  ".into(),
            image_keys: vec![],
            parent_id: None,
        },
        HistoryMessage {
            message_id: "m2".into(),
            create_time: 1_700_000_060,
            sender_id: "ou_bob".into(),
            text: "x".repeat(2500),
            image_keys: vec![],
            parent_id: None,
        },
    ];
    let refs: Vec<&HistoryMessage> = messages.iter().collect();
    let quotes =
        std::collections::HashMap::from([("m1".to_string(), "ou_x: 前文摘要".to_string())]);
    let out = assemble_history(&refs, &quotes);
    assert!(out.starts_with("<recent_chat_history>\n"));
    assert!(out.ends_with("</recent_chat_history>"));
    assert!(out.contains("ou_alice: hello world"), "trimmed: {out}");
    assert!(out.contains("hello world ↩ ou_x: 前文摘要"), "quote: {out}");
    let bob_line = out.lines().find(|l| l.contains("ou_bob")).unwrap();
    assert!(bob_line.ends_with('…'), "capped: {bob_line}");
    assert!(bob_line.chars().count() <= 2000 + 40, "line: {bob_line}");
    assert!(!bob_line.contains('↩'), "unquoted line has no snippet");
}

#[derive(Default)]
struct HistoryMockAdapter {
    calls: tokio::sync::Mutex<Vec<(Option<i64>, usize)>>,
    fail: std::sync::atomic::AtomicBool,
    empty: std::sync::atomic::AtomicBool,
    with_root: std::sync::atomic::AtomicBool,
    with_images: std::sync::atomic::AtomicBool,
    /// When set, history includes command messages (control-plane noise).
    with_commands: std::sync::atomic::AtomicBool,
    /// When set, history message m0 quote-replies the thread root.
    quote_m0: std::sync::atomic::AtomicBool,
    /// When set, `fetch_message` fails.
    fetch_fail: std::sync::atomic::AtomicBool,
    quoted: tokio::sync::Mutex<Option<HistoryMessage>>,
    fetch_calls: tokio::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl PlatformAdapter for HistoryMockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelEvent>,
        cancel: CancellationToken,
    ) -> std::result::Result<(), crate::channels::ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        _external_chat_id: &str,
        _blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        Ok(None)
    }

    async fn fetch_message(
        &self,
        message_id: &str,
    ) -> std::result::Result<Option<HistoryMessage>, crate::channels::ChannelError> {
        self.fetch_calls.lock().await.push(message_id.to_string());
        if self.fetch_fail.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::channels::ChannelError::Platform(
                "mock fetch failure".into(),
            ));
        }
        // Echo the requested id like the real API does.
        Ok(self.quoted.lock().await.clone().map(|mut m| {
            m.message_id = message_id.to_string();
            m
        }))
    }

    async fn fetch_history(
        &self,
        _container: &HistoryContainer,
        since_ts: Option<i64>,
        limit: usize,
    ) -> std::result::Result<Vec<HistoryMessage>, crate::channels::ChannelError> {
        if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::channels::ChannelError::Platform(
                "mock fetch failure".into(),
            ));
        }
        if self.empty.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        self.calls.lock().await.push((since_ts, limit));
        let mut messages = Vec::new();
        if self.with_root.load(std::sync::atomic::Ordering::Relaxed) {
            messages.push(HistoryMessage {
                message_id: "root-msg".into(),
                create_time: 50,
                sender_id: "ou_a".into(),
                text: "thread root".into(),
                image_keys: vec![],
                parent_id: None,
            });
        }
        messages.extend([
            HistoryMessage {
                message_id: "m0".into(),
                create_time: 100,
                sender_id: "ou_a".into(),
                text: "earlier".into(),
                image_keys: vec![],
                parent_id: self
                    .quote_m0
                    .load(std::sync::atomic::Ordering::Relaxed)
                    .then(|| "root-msg".to_string()),
            },
            HistoryMessage {
                message_id: "m1".into(),
                create_time: 200,
                sender_id: "ou_a".into(),
                text: "latest".into(),
                image_keys: if self.with_images.load(std::sync::atomic::Ordering::Relaxed) {
                    vec!["img_x".into()]
                } else {
                    vec![]
                },
                parent_id: None,
            },
        ]);
        if self
            .with_commands
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            messages.extend([
                HistoryMessage {
                    message_id: "cmd1".into(),
                    create_time: 250,
                    sender_id: "ou_a".into(),
                    text: "/info".into(),
                    image_keys: vec![],
                    parent_id: None,
                },
                HistoryMessage {
                    message_id: "cmd2".into(),
                    create_time: 260,
                    sender_id: "ou_a".into(),
                    text: "@_user_1 /clear@yomi_bot".into(),
                    image_keys: vec![],
                    parent_id: None,
                },
            ]);
        }
        messages.push(HistoryMessage {
            message_id: "trigger".into(),
            create_time: 300,
            sender_id: "ou_b".into(),
            text: "trigger msg".into(),
            image_keys: vec![],
            parent_id: None,
        });
        Ok(messages)
    }

    async fn download_message_image(
        &self,
        message_id: &str,
        image_key: &str,
    ) -> std::result::Result<ContentBlock, crate::channels::ChannelError> {
        Ok(ContentBlock::ImageUrl {
            image_url: format!("data:image/png;base64,mock-{message_id}-{image_key}").into(),
        })
    }
}

fn group_msg(thread_id: Option<String>) -> ChannelMessage {
    ChannelMessage {
        external_chat_id: "oc_1".into(),
        external_user_id: "ou_b".into(),
        external_message_id: Some("trigger".into()),
        is_mention: true,
        raw_text: None,
        content: vec![],
        image_keys: vec![],
        thread_id,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: None,
        doc_comment: None,
    }
}

/// Concatenate the text blocks of an assembled history prefix.
fn blocks_text(blocks: &[ContentBlock]) -> String {
    blocks.iter().filter_map(ContentBlock::as_text).collect()
}

fn quoted_history_msg() -> HistoryMessage {
    HistoryMessage {
        message_id: "om_q".into(),
        create_time: 1_700_000_000_000,
        sender_id: "ou_x".into(),
        text: "被引用的内容".into(),
        image_keys: vec![],
        parent_id: None,
    }
}

#[tokio::test]
async fn test_quoted_prefix_rules() {
    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    *mock.quoted.lock().await = Some(quoted_history_msg());

    let base = group_msg(None);

    // No quote → nothing, and no fetch attempted.
    assert!(maybe_quoted_prefix(&adapter, &base, RootDelivery::Pending)
        .await
        .is_none());
    assert!(mock.quoted_calls.lock().await.is_empty());

    // Routine thread reply to the root in a reused session → skipped
    // (the root was already consumed into the session).
    let mut thread_reply = base.clone();
    thread_reply.thread_id = Some("omt_1".into());
    thread_reply.root_id = Some("om_root".into());
    thread_reply.parent_id = Some("om_root".into());
    assert!(
        maybe_quoted_prefix(&adapter, &thread_reply, RootDelivery::Consumed)
            .await
            .is_none()
    );
    assert!(mock.quoted_calls.lock().await.is_empty());

    // Same reply on a FRESH session (human-created thread): the root is
    // exactly the missing context → injected like any other quote, and
    // reported as root-delivering.
    mock.quoted_map.lock().await.insert(
        "om_root".into(),
        HistoryMessage {
            message_id: "om_root".into(),
            create_time: 1_700_000_000_000,
            sender_id: "ou_x".into(),
            text: "话题根消息".into(),
            image_keys: vec![],
            parent_id: None,
        },
    );
    let (blocks, in_chain) = maybe_quoted_prefix(&adapter, &thread_reply, RootDelivery::Pending)
        .await
        .expect("fresh thread root injected");
    assert!(blocks_text(&blocks).contains("<quoted_message>"));
    assert!(in_chain, "the chain link IS the root");

    // Top-level quote → injected (a fresh session: the quote IS the context).
    let mut top_quote = base.clone();
    top_quote.parent_id = Some("om_q".into());
    top_quote.root_id = Some("om_q".into());
    let (blocks, _) = maybe_quoted_prefix(&adapter, &top_quote, RootDelivery::Pending)
        .await
        .expect("quoted block");
    let text = blocks_text(&blocks);
    assert!(text.contains("<quoted_message>"), "{text}");
    assert!(text.contains("ou_x: 被引用的内容"), "{text}");

    // Mid-thread quote → injected.
    let mut mid_quote = base;
    mid_quote.thread_id = Some("omt_1".into());
    mid_quote.root_id = Some("om_root".into());
    mid_quote.parent_id = Some("om_q".into());
    assert!(
        maybe_quoted_prefix(&adapter, &mid_quote, RootDelivery::Consumed)
            .await
            .is_some()
    );

    assert_eq!(
        mock.quoted_calls.lock().await.as_slice(),
        ["om_root", "om_q", "om_q"]
    );
}

#[tokio::test]
async fn test_quoted_prefix_includes_images() {
    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    *mock.image_download_ok.lock().await = true;
    *mock.quoted.lock().await = Some(HistoryMessage {
        message_id: "om_img".into(),
        create_time: 1_700_000_000_000,
        sender_id: "ou_x".into(),
        text: "[image]".into(),
        image_keys: vec!["img_1".into()],
        parent_id: None,
    });

    let mut msg = group_msg(None);
    msg.parent_id = Some("om_img".into());
    let (blocks, _) = maybe_quoted_prefix(&adapter, &msg, RootDelivery::Pending)
        .await
        .expect("quoted block");
    assert!(
        blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ImageUrl { .. })),
        "quoted image should be downloaded: {blocks:?}"
    );
}

#[tokio::test]
async fn test_quoted_prefix_walks_quote_chain() {
    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut map = mock.quoted_map.lock().await;
    map.insert(
        "om_r".into(),
        HistoryMessage {
            message_id: "om_r".into(),
            create_time: 1_700_000_060_000,
            sender_id: "ou_b".into(),
            text: "引用回复：这根消息说了啥".into(),
            image_keys: vec![],
            parent_id: Some("om_m0".into()),
        },
    );
    map.insert(
        "om_m0".into(),
        HistoryMessage {
            message_id: "om_m0".into(),
            create_time: 1_700_000_000_000,
            sender_id: "ou_a".into(),
            text: "原始消息".into(),
            image_keys: vec![],
            parent_id: None,
        },
    );
    drop(map);

    // A human thread whose root is itself a quote-reply: the fresh
    // session gets the whole chain, ancestors first.
    let mut msg = group_msg(Some("omt_1".into()));
    msg.root_id = Some("om_r".into());
    msg.parent_id = Some("om_r".into());
    let (blocks, in_chain) = maybe_quoted_prefix(&adapter, &msg, RootDelivery::Pending)
        .await
        .expect("quoted chain");
    let text = blocks_text(&blocks);
    let ancestor = text.find("ou_a: 原始消息").expect("ancestor: {text}");
    let quoted = text.find("ou_b: 引用回复").expect("quoted: {text}");
    assert!(ancestor < quoted, "chronological: {text}");
    assert!(in_chain, "chain reached the root: {text}");
    assert_eq!(mock.quoted_calls.lock().await.as_slice(), ["om_r", "om_m0"]);
}

#[tokio::test]
async fn test_quoted_prefix_chain_capped_and_partial() {
    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    // Four-deep chain → capped at QUOTE_CHAIN_MAX fetches.
    {
        let mut map = mock.quoted_map.lock().await;
        for (id, parent) in [("m1", "m2"), ("m2", "m3"), ("m3", "m4"), ("m4", "m5")] {
            map.insert(
                id.into(),
                HistoryMessage {
                    message_id: id.into(),
                    create_time: 1,
                    sender_id: "ou_a".into(),
                    text: id.into(),
                    image_keys: vec![],
                    parent_id: Some(parent.into()),
                },
            );
        }
    }
    let mut msg = group_msg(None);
    msg.parent_id = Some("m1".into());
    let (blocks, _) = maybe_quoted_prefix(&adapter, &msg, RootDelivery::Pending)
        .await
        .expect("chain");
    let text = blocks_text(&blocks);
    assert!(text.contains("m3"), "three links assembled: {text}");
    assert!(!text.contains("m4"), "capped before the fourth: {text}");
    assert_eq!(mock.quoted_calls.lock().await.len(), QUOTE_CHAIN_MAX);

    // A mid-chain miss keeps the prefix assembled so far.
    let mock2 = Arc::new(MockAdapter::new("mock2"));
    let adapter2: Arc<dyn PlatformAdapter> = mock2.clone();
    mock2.quoted_map.lock().await.insert(
        "only".into(),
        HistoryMessage {
            message_id: "only".into(),
            create_time: 1,
            sender_id: "ou_a".into(),
            text: "第一层".into(),
            image_keys: vec![],
            parent_id: Some("missing".into()),
        },
    );
    let mut msg2 = group_msg(None);
    msg2.parent_id = Some("only".into());
    let (blocks, _) = maybe_quoted_prefix(&adapter2, &msg2, RootDelivery::Pending)
        .await
        .expect("partial chain");
    assert!(blocks_text(&blocks).contains("第一层"));
    assert_eq!(
        mock2.quoted_calls.lock().await.as_slice(),
        ["only", "missing"]
    );
}

#[tokio::test]
async fn context_prefix_orders_history_before_quoted() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    *mock.quoted.lock().await = Some(HistoryMessage {
        message_id: "om_q".into(),
        create_time: 50,
        sender_id: "ou_x".into(),
        text: "被引用的内容".into(),
        image_keys: vec![],
        parent_id: None,
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    // In-thread trigger quoting a mid-thread message (not the root), so
    // both blocks are produced: history as background, quoted adjacent
    // to the trigger.
    let mut msg = group_msg(Some("omt_1".into()));
    msg.root_id = Some("om_root".into());
    msg.parent_id = Some("om_q".into());
    let blocks = context_prefix(&adapter, &config, &store, "feishu", &msg, false).await;
    let text = blocks_text(&blocks);
    let history = text.find("<recent_chat_history>").expect("history: {text}");
    let quoted = text.find("<quoted_message>").expect("quoted: {text}");
    assert!(history < quoted, "history first, quoted last: {text}");
}

#[tokio::test]
async fn context_prefix_fresh_thread_root_exactly_once() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.with_root
        .store(true, std::sync::atomic::Ordering::Relaxed);
    *mock.quoted.lock().await = Some(HistoryMessage {
        message_id: "root-msg".into(),
        create_time: 50,
        sender_id: "ou_a".into(),
        text: "thread root".into(),
        image_keys: vec![],
        parent_id: None,
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    // Fresh in-thread trigger replying to the root: the quoted block
    // delivers it (Pending → ByQuote), history must not repeat it.
    let mut msg = group_msg(Some("omt_1".into()));
    msg.root_id = Some("root-msg".into());
    msg.parent_id = Some("root-msg".into());
    let blocks = context_prefix(&adapter, &config, &store, "feishu", &msg, false).await;
    let text = blocks_text(&blocks);
    assert_eq!(
        text.matches("thread root").count(),
        1,
        "root exactly once: {text}"
    );
    assert!(text.contains("<quoted_message>"), "via quoted: {text}");
}

#[tokio::test]
async fn clear_does_not_rewind_history_cursor() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let clear = |create_time: i64| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_b".to_string(),
        external_message_id: Some("msg-1".to_string()),
        is_mention: true,
        raw_text: Some("/clear".to_string()),
        content: vec![ContentBlock::Text {
            text: "/clear".to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: Some(create_time),
        doc_comment: None,
    };

    // Cursor already ahead (later activity was consumed): a
    // late-processed /clear must not rewind it. (The /clear arm itself
    // touches no cursor — the loop-level advance does, monotonically.)
    store
        .set_history_cursor("mock", "oc_1", 1000)
        .await
        .unwrap();
    let msg = clear(500);
    handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        msg.clone(),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    advance_history_cursor(&config, &store, "mock", &msg).await;
    assert_eq!(
        store.get_history_cursor("mock", "oc_1").await.unwrap(),
        Some(1000),
        "no rewind"
    );

    // Cursor behind: /clear advances to the command's own timestamp.
    let msg = clear(2000);
    handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        msg.clone(),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    advance_history_cursor(&config, &store, "mock", &msg).await;
    assert_eq!(
        store.get_history_cursor("mock", "oc_1").await.unwrap(),
        Some(2000),
        "advanced"
    );
}

/// A refused `/thread` ran nothing: it must not advance the history
/// cursor — otherwise the next real trigger silently loses the window
/// the refusal skipped.
#[tokio::test]
async fn refused_thread_command_does_not_advance_history_cursor() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let feishu = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        ..Default::default()
    };
    let msg = |thread_id: Option<&str>, ts: i64| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_1".to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some("/thread hi".to_string()),
        content: vec![ContentBlock::Text {
            text: "/thread hi".to_string(),
        }],
        image_keys: vec![],
        thread_id: thread_id.map(str::to_string),
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: Some(ts),
        doc_comment: None,
    };

    // Refused in-thread: no cursor on the thread container.
    advance_history_cursor(&feishu, &store, "mock", &msg(Some("omt_1"), 1000)).await;
    assert_eq!(
        store.get_history_cursor("mock", "omt_1").await.unwrap(),
        None
    );

    // Refused on Telegram: no cursor on the chat container.
    let telegram = ChannelConfig {
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        ..feishu.clone()
    };
    advance_history_cursor(&telegram, &store, "mock", &msg(None, 1000)).await;
    assert_eq!(
        store.get_history_cursor("mock", "oc_1").await.unwrap(),
        None
    );

    // A runnable /thread consumes as before: the chat cursor advances.
    advance_history_cursor(&feishu, &store, "mock", &msg(None, 2000)).await;
    assert_eq!(
        store.get_history_cursor("mock", "oc_1").await.unwrap(),
        Some(2000)
    );
}

/// Model/info commands in a fresh thread must not claim it: thread
/// mappings are conversation-only, so the first real trigger still
/// treats the thread as fresh (and inherits the chat-level model).
#[tokio::test]
async fn model_commands_do_not_claim_fresh_thread() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        models: vec![
            crate::provider::ModelConfig {
                name: "m1".into(),
                ..Default::default()
            },
            crate::provider::ModelConfig {
                name: "m2".into(),
                ..Default::default()
            },
        ],
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram {
            token: "fake".into(),
        },
        require_mention: false,
        reply_in_thread: true,
        ..Default::default()
    };
    let msg = |raw_text: Option<&str>| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_b".to_string(),
        external_message_id: Some("msg-1".to_string()),
        is_mention: true,
        raw_text: raw_text.map(str::to_string),
        content: vec![ContentBlock::Text {
            text: raw_text.unwrap_or("你好").to_string(),
        }],
        image_keys: vec![],
        thread_id: Some("omt_1".to_string()),
        root_id: Some("om_root".to_string()),
        parent_id: Some("om_root".to_string()),
        is_group: true,
        create_time: None,
        doc_comment: None,
    };
    let thread_claimed = || async {
        store
            .find_mapping("mock", "om_root")
            .await
            .unwrap()
            .is_some()
    };

    // /models: answered from the resolved model, no mapping created.
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        msg(Some("/models")),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(reply.is_none(), "info replies go through the adapter");
    let text = last_outgoing_text(&mock).await;
    assert!(text.contains("`m1`"), "models list: {text}");
    assert!(!thread_claimed().await, "/models must not claim the thread");

    // /info: degrades to the resolved model, no mapping created.
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        msg(Some("/info")),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(
        reply
            .as_deref()
            .is_some_and(|r| r.contains("No session yet")),
        "{reply:?}"
    );
    assert!(!thread_claimed().await, "/info must not claim the thread");

    // /model m2: no thread session to switch → falls back to the chat
    // session; the thread stays unclaimed.
    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        msg(Some("/model m2")),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(
        reply.as_deref().is_some_and(|r| r.contains("all threads")),
        "{reply:?}"
    );
    assert!(!thread_claimed().await, "/model must not claim the thread");
    assert!(
        store.find_mapping("mock", "oc_1").await.unwrap().is_some(),
        "the choice lands on the chat session"
    );

    // The first real trigger now claims the thread — and inherits m2.
    handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        msg(None),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    let sid = store
        .find_mapping("mock", "om_root")
        .await
        .unwrap()
        .expect("thread session created by the real trigger");
    assert_eq!(kernel.get_session_model(&sid).await, "m2", "inherited");
}

#[test]
fn consumes_history_gate() {
    // Run triggers consume context; /clear deliberately discards it —
    // both may advance the cursor.
    assert!(consumes_history(&ChannelCommand::None));
    assert!(consumes_history(&ChannelCommand::Steer("x".into())));
    assert!(consumes_history(&ChannelCommand::Queue("x".into())));
    assert!(consumes_history(&ChannelCommand::Thread("x".into())));
    assert!(consumes_history(&ChannelCommand::Clear));
    // Read-only/other commands never read history → never advance.
    for cmd in [
        ChannelCommand::Stop,
        ChannelCommand::ListModels,
        ChannelCommand::CurrentModel,
        ChannelCommand::SwitchModel("k".into()),
        ChannelCommand::Info,
        ChannelCommand::Help,
        ChannelCommand::Restart,
    ] {
        assert!(!consumes_history(&cmd));
    }
}

#[tokio::test]
async fn history_prefix_backstops_root_outside_page() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    // No with_root: the fetched page does NOT include the root.
    *mock.quoted.lock().await = Some(HistoryMessage {
        message_id: "root-msg".into(),
        create_time: 50,
        sender_id: "ou_a".into(),
        text: "thread root".into(),
        image_keys: vec![],
        parent_id: None,
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    let mut msg = group_msg(Some("omt_1".into()));
    msg.root_id = Some("root-msg".into());
    // The trigger replies to a mid-thread message, so the quoted path
    // does not carry the root; the backstop must.
    msg.parent_id = Some("m0".into());

    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Pending,
    )
    .await
    .expect("history");
    let prefix = blocks_text(&prefix);
    assert!(prefix.contains("thread root"), "backstopped root: {prefix}");
    assert_eq!(
        mock.fetch_calls.lock().await.as_slice(),
        ["root-msg"],
        "backstop fetched the root once"
    );

    // Consumed state → no backstop fetch.
    let (_pool2, store2) = create_test_pool().await;
    let store2: Arc<dyn ChannelStore> = store2;
    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store2,
        "feishu",
        &msg,
        RootDelivery::Consumed,
    )
    .await
    .expect("history");
    assert!(!blocks_text(&prefix).contains("thread root"));
    assert_eq!(
        mock.fetch_calls.lock().await.len(),
        1,
        "no backstop fetch when consumed"
    );
}

#[tokio::test]
async fn resolve_history_quotes_in_page_fetch_dedup_and_cap() {
    let mock = Arc::new(HistoryMockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    *mock.quoted.lock().await = Some(HistoryMessage {
        message_id: "x".into(),
        create_time: 1,
        sender_id: "ou_x".into(),
        text: "页外引用".into(),
        image_keys: vec![],
        parent_id: None,
    });
    let hmsg = |id: &str, text: &str, parent: Option<&str>| HistoryMessage {
        message_id: id.into(),
        create_time: 1,
        sender_id: "ou_a".into(),
        text: text.into(),
        image_keys: vec![],
        parent_id: parent.map(str::to_string),
    };
    let page = vec![
        hmsg("p1", "页内引用", None),
        hmsg("m1", "回复 p1", Some("p1")),
        hmsg("m2", "回复 x1", Some("x1")),
        hmsg("m3", "回复 x2", Some("x2")),
        hmsg("m4", "回复 x3", Some("x3")),
        hmsg("m5", "也回复 x1", Some("x1")),
        hmsg("m6", "回复 x4", Some("x4")),
    ];
    let history: Vec<&HistoryMessage> = page.iter().collect();
    let quotes = resolve_history_quotes(&adapter, &page, &history).await;

    // In-page parent: free.
    assert_eq!(quotes["m1"], "ou_a: 页内引用");
    // Out-of-page parents: fetched once each, deduped by parent, and
    // capped at HISTORY_QUOTE_FETCH_MAX distinct parents (x4 skipped).
    assert_eq!(quotes["m2"], "ou_x: 页外引用");
    assert_eq!(quotes["m5"], "ou_x: 页外引用");
    assert!(!quotes.contains_key("m6"), "cap: {quotes:?}");
    // m1 (in-page) + 3 distinct fetched parents (m2/m3/m4) + m5 reusing x1.
    assert_eq!(quotes.len(), 2 + HISTORY_QUOTE_FETCH_MAX);
    assert_eq!(mock.fetch_calls.lock().await.len(), HISTORY_QUOTE_FETCH_MAX);
}

#[tokio::test]
async fn history_prefix_inlines_quote_snippet_from_page() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.with_root
        .store(true, std::sync::atomic::Ordering::Relaxed);
    mock.quote_m0
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    let mut msg = group_msg(Some("omt_1".into()));
    msg.root_id = Some("root-msg".into());

    // The root is dropped (consumed), but m0's quote of it is resolved
    // from the fetched page for free — no fetch_message call.
    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Consumed,
    )
    .await
    .expect("history");
    let prefix = blocks_text(&prefix);
    assert!(
        prefix.contains("earlier ↩ ou_a: thread root"),
        "inline quote: {prefix}"
    );
    assert!(mock.fetch_calls.lock().await.is_empty(), "in-page is free");
}

#[tokio::test]
async fn resolve_history_quotes_failure_cached_and_counts_toward_cap() {
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.fetch_fail
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let hmsg = |id: &str, parent: &str| HistoryMessage {
        message_id: id.into(),
        create_time: 1,
        sender_id: "ou_a".into(),
        text: id.into(),
        image_keys: vec![],
        parent_id: Some(parent.into()),
    };
    let page = vec![
        hmsg("m1", "x1"),
        hmsg("m2", "x1"),
        hmsg("m3", "x2"),
        hmsg("m4", "x3"),
        hmsg("m5", "x4"),
    ];
    let history: Vec<&HistoryMessage> = page.iter().collect();
    let quotes = resolve_history_quotes(&adapter, &page, &history).await;

    assert!(quotes.is_empty(), "all fetches failed: {quotes:?}");
    // Each distinct parent tried exactly once (x1's failure cached for
    // m2), still capped at HISTORY_QUOTE_FETCH_MAX distinct parents.
    assert_eq!(mock.fetch_calls.lock().await.len(), HISTORY_QUOTE_FETCH_MAX);
}

#[tokio::test]
async fn history_quote_of_backstopped_root_resolves_free() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    // The root is NOT in the fetched page — the backstop fetches it once.
    *mock.quoted.lock().await = Some(HistoryMessage {
        message_id: "root-msg".into(),
        create_time: 50,
        sender_id: "ou_a".into(),
        text: "thread root".into(),
        image_keys: vec![],
        parent_id: None,
    });
    mock.quote_m0
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    let mut msg = group_msg(Some("omt_1".into()));
    msg.root_id = Some("root-msg".into());
    msg.parent_id = Some("m0".into());

    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Pending,
    )
    .await
    .expect("history");
    let prefix = blocks_text(&prefix);
    assert!(
        prefix.contains("earlier ↩ ou_a: thread root"),
        "quote of backstopped root: {prefix}"
    );
    assert_eq!(
        mock.fetch_calls.lock().await.as_slice(),
        ["root-msg"],
        "backstop's fetch is reused — no double fetch"
    );
}

#[tokio::test]
async fn history_prefix_assembles_drops_trigger_and_advances_cursor() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    let msg = group_msg(None);

    let blocks = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Pending,
    )
    .await;
    let blocks = blocks.expect("history prefix");
    let prefix = blocks_text(&blocks);
    assert!(prefix.contains("earlier"));
    assert!(prefix.contains("latest"));
    assert!(!prefix.contains("trigger msg"), "trigger dropped: {prefix}");

    // Cursor advanced to the newest fetched message (the trigger's ts).
    let cursor = store.get_history_cursor("feishu", "oc_1").await.unwrap();
    assert_eq!(cursor, Some(300));

    // Second call passes the stored cursor through to the adapter.
    let _ = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Pending,
    )
    .await;
    let calls = mock.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], (None, 20));
    assert_eq!(calls[1], (Some(300), 20));
}

#[test]
fn test_is_command_text() {
    // Bare commands, with @bot suffix, with leading mention(s), and
    // arg-less command tokens are all control-plane.
    assert!(is_command_text("/info"));
    assert!(is_command_text("/c"));
    assert!(is_command_text("/q run this next"));
    assert!(is_command_text("/clear@yomi_bot"));
    assert!(is_command_text("@_user_1 /info"));
    assert!(is_command_text("@_user_1 @_user_2 /clear"));
    assert!(is_command_text("  /steer"));
    // Longer words, unknown commands, mid-sentence commands, plain
    // mentions, and empty / mention-only texts stay.
    assert!(!is_command_text("/clearance 大甩卖"));
    assert!(!is_command_text("/claer"));
    assert!(!is_command_text("/tmp/foo"));
    assert!(!is_command_text("记得跑一下 /info"));
    assert!(!is_command_text("@alice 你好"));
    assert!(!is_command_text(""));
    assert!(!is_command_text("@"));
}

#[tokio::test]
async fn history_prefix_drops_command_messages() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.with_commands
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    let msg = group_msg(None);

    let blocks = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Pending,
    )
    .await
    .expect("history prefix");
    let prefix = blocks_text(&blocks);
    assert!(prefix.contains("earlier"));
    assert!(prefix.contains("latest"));
    assert!(!prefix.contains("/info"), "bare command dropped: {prefix}");
    assert!(
        !prefix.contains("/clear@yomi_bot"),
        "mentioned command dropped: {prefix}"
    );

    // Cursor still advances past the dropped commands — no refetch.
    let cursor = store.get_history_cursor("feishu", "oc_1").await.unwrap();
    assert_eq!(cursor, Some(300));
}

#[tokio::test]
async fn history_prefix_keeps_command_shaped_root() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.with_images
        .store(true, std::sync::atomic::Ordering::Relaxed);
    // No with_root: the root comes from the backstop — with command-
    // shaped text. It is EXEMPT from the command filter: history[0] must
    // stay the root or the image priority slicing below misfires.
    *mock.quoted.lock().await = Some(HistoryMessage {
        message_id: "root-msg".into(),
        create_time: 50,
        sender_id: "ou_a".into(),
        text: "/info".into(),
        image_keys: vec!["img_root".into()],
        parent_id: None,
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    let mut msg = group_msg(Some("omt_1".into()));
    msg.root_id = Some("root-msg".into());
    msg.parent_id = Some("m0".into());

    let blocks = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Pending,
    )
    .await
    .expect("history");
    let text = blocks_text(&blocks);
    assert!(text.contains("/info"), "command-shaped root kept: {text}");
    // The root's image keeps its last-position (survive-the-cap)
    // priority — proof the positional invariant held.
    let images: Vec<&str> = blocks
        .iter()
        .filter_map(|b| match b {
            ContentBlock::ImageUrl { image_url } => Some(image_url.url.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(images.len(), 2, "{images:?}");
    assert!(images[0].contains("mock-m1-img_x"), "{images:?}");
    assert!(images[1].contains("mock-root-msg-img_root"), "{images:?}");
}

#[tokio::test]
async fn history_prefix_skips_private_chats() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(HistoryMockAdapter::default());
    let config = ChannelConfig::default();
    let mut msg = group_msg(None);
    msg.is_group = false;

    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Pending,
    )
    .await;
    assert!(prefix.is_none());
}

#[tokio::test]
async fn history_prefix_uses_thread_container_when_present() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(HistoryMockAdapter::default());
    let config = ChannelConfig::default();
    let msg = group_msg(Some("omt_1".into()));

    let _ = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Pending,
    )
    .await;
    // Cursor is keyed by the thread id, not the chat id.
    let cursor = store.get_history_cursor("feishu", "omt_1").await.unwrap();
    assert_eq!(cursor, Some(300));
    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn history_prefix_degrades_to_none_on_fetch_error() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.fail.store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();

    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &group_msg(None),
        RootDelivery::Pending,
    )
    .await;
    assert!(prefix.is_none(), "fetch failure degrades to no context");
}

#[tokio::test]
async fn history_prefix_disabled_by_zero_config() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig {
        history_context: 0,
        ..ChannelConfig::default()
    };

    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &group_msg(None),
        RootDelivery::Pending,
    )
    .await;
    assert!(prefix.is_none());
    assert!(mock.calls.lock().await.is_empty(), "no fetch issued");
}

#[tokio::test]
async fn history_prefix_empty_fetch_keeps_cursor_unset() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.empty.store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();

    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &group_msg(None),
        RootDelivery::Pending,
    )
    .await;
    assert!(prefix.is_none());
    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        None,
        "empty fetch must not advance the cursor"
    );
}

#[tokio::test]
async fn history_prefix_skips_channel_level_when_reply_in_thread() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig {
        reply_in_thread: true,
        ..ChannelConfig::default()
    };

    // Channel-level trigger with reply_in_thread: no history (a fresh
    // thread starts; cross-topic chatter is noise there).
    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &group_msg(None),
        RootDelivery::Pending,
    )
    .await;
    assert!(prefix.is_none());
    assert!(mock.calls.lock().await.is_empty(), "no fetch issued");

    // Inside an existing thread, that thread's history still applies.
    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &group_msg(Some("omt_1".into())),
        RootDelivery::Pending,
    )
    .await;
    assert!(prefix.is_some(), "thread history still injected");
}

#[tokio::test]
async fn advance_cursor_is_monotonic_and_gated() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let config = ChannelConfig::default();
    let mut msg = group_msg(None);

    // Advances to the message ts.
    msg.create_time = Some(1000);
    advance_history_cursor(&config, &store, "feishu", &msg).await;
    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        Some(1000)
    );

    // Older messages never rewind.
    msg.create_time = Some(500);
    advance_history_cursor(&config, &store, "feishu", &msg).await;
    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        Some(1000)
    );

    // Newer advances.
    msg.create_time = Some(2000);
    advance_history_cursor(&config, &store, "feishu", &msg).await;
    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        Some(2000)
    );

    // Private chats are skipped.
    msg.is_group = false;
    msg.create_time = Some(3000);
    advance_history_cursor(&config, &store, "feishu", &msg).await;
    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        Some(2000)
    );

    // Disabled by config.
    msg.is_group = true;
    let config_off = ChannelConfig {
        history_context: 0,
        ..ChannelConfig::default()
    };
    advance_history_cursor(&config_off, &store, "feishu", &msg).await;
    assert_eq!(
        store.get_history_cursor("feishu", "oc_1").await.unwrap(),
        Some(2000)
    );
}

#[tokio::test]
async fn history_prefix_thread_root_drop_rules() {
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.with_root
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    let mut msg = group_msg(Some("omt_1".into()));
    msg.root_id = Some("root-msg".into());

    // Fresh human thread → the root is kept, even with a chat cursor far
    // newer than it (the old heuristic's misfire condition).
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    store
        .set_history_cursor("feishu", "oc_1", 100)
        .await
        .unwrap();
    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &msg,
        RootDelivery::Pending,
    )
    .await
    .expect("history");
    let prefix = blocks_text(&prefix);
    assert!(
        prefix.contains("thread root"),
        "fresh thread root kept: {prefix}"
    );
    assert!(prefix.contains("earlier"), "other entries kept: {prefix}");

    // Reused session → the root was already consumed → dropped.
    let (_pool2, store2) = create_test_pool().await;
    let store2: Arc<dyn ChannelStore> = store2;
    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store2,
        "feishu",
        &msg,
        RootDelivery::Consumed,
    )
    .await
    .expect("history");
    let prefix = blocks_text(&prefix);
    assert!(
        !prefix.contains("thread root"),
        "consumed root dropped: {prefix}"
    );
    assert!(prefix.contains("earlier"), "other entries kept: {prefix}");

    // Fresh session, but the quoted block just delivered the root →
    // dropped here so it isn't injected twice.
    let (_pool3, store3) = create_test_pool().await;
    let store3: Arc<dyn ChannelStore> = store3;
    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store3,
        "feishu",
        &msg,
        RootDelivery::ByQuote,
    )
    .await
    .expect("history");
    let prefix = blocks_text(&prefix);
    assert!(
        !prefix.contains("thread root"),
        "quoted root deduped: {prefix}"
    );
}

#[tokio::test]
async fn history_prefix_attaches_history_images() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.with_images
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();

    let blocks = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &group_msg(None),
        RootDelivery::Pending,
    )
    .await
    .expect("history blocks");

    // Text block first, then the downloaded image of history message m1.
    assert!(matches!(blocks[0], ContentBlock::Text { .. }));
    let images: Vec<_> = blocks
        .iter()
        .filter(|b| matches!(b, ContentBlock::ImageUrl { .. }))
        .collect();
    assert_eq!(images.len(), 1, "one history image attached: {blocks:?}");
    let ContentBlock::ImageUrl { image_url } = images[0] else {
        unreachable!()
    };
    assert!(
        image_url.url.contains("mock-m1-img_x"),
        "url: {image_url:?}"
    );
}

// ── Deferred image download (post-gate) ─────────────────────────────

#[tokio::test]
async fn append_message_images_downloads_and_appends() {
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(HistoryMockAdapter::default());
    let mut content = vec![ContentBlock::Text {
        text: "body".into(),
    }];

    append_message_images(&adapter, "trigger", &["img_x".to_string()], &mut content).await;

    assert_eq!(content.len(), 2);
    let ContentBlock::ImageUrl { image_url } = &content[1] else {
        panic!("expected image block: {content:?}");
    };
    assert!(
        image_url.url.contains("mock-trigger-img_x"),
        "url: {image_url:?}"
    );
}

#[tokio::test]
async fn append_message_images_failure_degrades_to_placeholder() {
    // MockAdapter does not implement download_message_image → default Err.
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(MockAdapter::new("fs"));
    let mut content = Vec::new();

    append_message_images(&adapter, "trigger", &["img_x".to_string()], &mut content).await;

    assert_eq!(content.len(), 1);
    let ContentBlock::Text { text } = &content[0] else {
        panic!("expected placeholder: {content:?}");
    };
    assert!(text.contains("[Failed to download image:"), "{text}");
}

#[tokio::test]
async fn append_message_images_no_keys_untouched() {
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(MockAdapter::new("fs"));
    let mut content = vec![ContentBlock::Text {
        text: "body".into(),
    }];

    append_message_images(&adapter, "trigger", &[], &mut content).await;

    assert_eq!(content.len(), 1);
}

#[tokio::test]
async fn append_message_images_caps_an_image_dump() {
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(HistoryMockAdapter::default());
    let mut content = Vec::new();
    let keys: Vec<String> = (0..8).map(|i| format!("img_{i}")).collect();

    append_message_images(&adapter, "trigger", &keys, &mut content).await;

    // IMAGE_DOWNLOAD_MAX images + one note for the rest.
    assert_eq!(content.len(), IMAGE_DOWNLOAD_MAX + 1, "{content:?}");
    let ContentBlock::Text { text } = &content[IMAGE_DOWNLOAD_MAX] else {
        panic!("expected omission note: {content:?}");
    };
    assert!(text.contains("3 more image(s) omitted"), "{text}");
}

// ── Message gate reactions ──────────────────────────────────────────

fn gate_config(platform: PlatformConfig) -> ChannelConfig {
    ChannelConfig {
        name: "gate".to_string(),
        enabled: true,
        platform,
        allowed_users: vec!["user-1".to_string()],
        ..Default::default()
    }
}

fn feishu_gate_config() -> ChannelConfig {
    gate_config(PlatformConfig::Feishu {
        app_id: "app".to_string(),
        app_secret: "secret".to_string(),
    })
}

/// A throwaway store for gate tests (mention overrides default to none).
async fn gate_store() -> Arc<dyn ChannelStore> {
    let (_pool, store) = create_test_pool().await;
    store
}

/// Gate the message and fire its reaction inline, so tests can assert on
/// the mock adapter's reactions (production fires it off the gate loop).
async fn gate_with_reaction(
    adapter: &Arc<dyn PlatformAdapter>,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    msg: &ChannelMessage,
) -> Gate {
    let (gate, reaction) = gate_message(config, store, msg).await;
    send_gate_reaction(adapter, config, msg, reaction).await;
    gate
}

#[tokio::test]
async fn gate_accepts_allowed_mention_with_ack_reaction() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let msg = channel_message(None, true, true);

    assert_eq!(
        gate_with_reaction(&adapter, &feishu_gate_config(), &gate_store().await, &msg).await,
        Gate::Allow
    );
    let reactions = mock.reactions.lock().await.clone();
    assert_eq!(reactions, [("msg-1".to_string(), "OneSecond".to_string())]);
}

#[tokio::test]
async fn gate_acks_queue_command_with_queue_reaction() {
    // /q（与 /queue）排队消息用专属 ack："收到，排队候着"——OneSecond
    // 承诺马上处理，队列不做这个承诺。无效 /q（无文本）没落队，仍
    // 用普通 ack。
    for (raw, want) in [
        ("/q 排在后面", "Get"),
        ("/queue 也排在后面", "Get"),
        ("/q", "OneSecond"),
        ("/steer 插一句", "OneSecond"),
        ("普通消息", "OneSecond"),
    ] {
        let mock = Arc::new(MockAdapter::new("fs"));
        let adapter: Arc<dyn PlatformAdapter> = mock.clone();
        let mut msg = channel_message(None, true, true);
        msg.raw_text = Some(raw.to_string());

        assert_eq!(
            gate_with_reaction(&adapter, &feishu_gate_config(), &gate_store().await, &msg).await,
            Gate::Allow
        );
        let reactions = mock.reactions.lock().await.clone();
        assert_eq!(
            reactions,
            [("msg-1".to_string(), want.to_string())],
            "raw: {raw}"
        );
    }
}

#[tokio::test]
async fn gate_denies_unlisted_user_with_denied_reaction() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut msg = channel_message(None, true, true);
    msg.external_user_id = "stranger".to_string();

    assert_eq!(
        gate_with_reaction(&adapter, &feishu_gate_config(), &gate_store().await, &msg).await,
        Gate::Denied
    );
    let reactions = mock.reactions.lock().await.clone();
    assert_eq!(reactions, [("msg-1".to_string(), "THANKS".to_string())]);
}

#[tokio::test]
async fn gate_stays_silent_for_unlisted_user_without_mention() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut msg = channel_message(None, true, true);
    msg.external_user_id = "stranger".to_string();
    msg.is_mention = false;

    assert_eq!(
        gate_with_reaction(&adapter, &feishu_gate_config(), &gate_store().await, &msg).await,
        Gate::Denied
    );
    assert!(mock.reactions.lock().await.is_empty());
}

#[tokio::test]
async fn gate_marks_allowed_mention_miss_not_addressed() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut msg = channel_message(None, true, true);
    msg.is_mention = false;

    assert_eq!(
        gate_with_reaction(&adapter, &feishu_gate_config(), &gate_store().await, &msg).await,
        Gate::NotAddressed
    );
    assert!(mock.reactions.lock().await.is_empty());
}

#[tokio::test]
async fn gate_reacts_to_unlisted_user_when_mentions_not_required() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig {
        require_mention: false,
        ..feishu_gate_config()
    };
    let mut msg = channel_message(None, true, true);
    msg.external_user_id = "stranger".to_string();
    msg.is_mention = false;

    assert_eq!(
        gate_with_reaction(&adapter, &config, &gate_store().await, &msg).await,
        Gate::Denied
    );
    let reactions = mock.reactions.lock().await.clone();
    assert_eq!(reactions, [("msg-1".to_string(), "THANKS".to_string())]);
}

#[tokio::test]
async fn gate_stays_silent_for_blocked_user() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig {
        blocked_users: vec!["user-1".to_string()],
        ..feishu_gate_config()
    };
    let msg = channel_message(None, true, true);

    assert_eq!(
        gate_with_reaction(&adapter, &config, &gate_store().await, &msg).await,
        Gate::Denied
    );
    assert!(mock.reactions.lock().await.is_empty());
}

#[tokio::test]
async fn gate_telegram_acks_with_eyes_and_denies_with_folded_hands() {
    let mock = Arc::new(MockAdapter::new("tg"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = gate_config(PlatformConfig::Telegram {
        token: "fake".to_string(),
    });

    let msg = channel_message(None, true, true);
    assert_eq!(
        gate_with_reaction(&adapter, &config, &gate_store().await, &msg).await,
        Gate::Allow
    );

    let mut denied = channel_message(None, true, true);
    denied.external_user_id = "stranger".to_string();
    assert_eq!(
        gate_with_reaction(&adapter, &config, &gate_store().await, &denied).await,
        Gate::Denied
    );

    let reactions = mock.reactions.lock().await.clone();
    assert_eq!(
        reactions,
        [
            ("msg-1".to_string(), "👀".to_string()),
            ("msg-1".to_string(), "🙏".to_string()),
        ]
    );
}

#[tokio::test]
async fn gate_acks_every_allowed_message_when_mentions_not_required() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig {
        require_mention: false,
        ..feishu_gate_config()
    };
    let mut msg = channel_message(None, true, true);
    msg.is_mention = false;

    assert_eq!(
        gate_with_reaction(&adapter, &config, &gate_store().await, &msg).await,
        Gate::Allow
    );
    let reactions = mock.reactions.lock().await.clone();
    assert_eq!(reactions, [("msg-1".to_string(), "OneSecond".to_string())]);
}

#[tokio::test]
async fn gate_skips_reaction_without_message_id() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut msg = channel_message(None, true, false);
    msg.external_user_id = "stranger".to_string();

    assert_eq!(
        gate_with_reaction(&adapter, &feishu_gate_config(), &gate_store().await, &msg).await,
        Gate::Denied
    );
    assert!(mock.reactions.lock().await.is_empty());
}

// ── Mention overrides (/mention) ────────────────────────────────────

#[tokio::test]
async fn gate_chat_override_off_allows_non_mention() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let store = gate_store().await;
    // Config requires mentions; the chat override turns it off.
    store
        .set_mention_override("gate", "chat-1", false)
        .await
        .unwrap();
    let mut msg = channel_message(None, true, true);
    msg.is_mention = false;

    assert_eq!(
        gate_with_reaction(&adapter, &feishu_gate_config(), &store, &msg).await,
        Gate::Allow
    );
}

#[tokio::test]
async fn gate_chat_override_on_tightens_permissive_config() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let store = gate_store().await;
    store
        .set_mention_override("gate", "chat-1", true)
        .await
        .unwrap();
    let config = ChannelConfig {
        require_mention: false,
        ..feishu_gate_config()
    };
    let mut msg = channel_message(None, true, true);
    msg.is_mention = false;

    assert_eq!(
        gate_with_reaction(&adapter, &config, &store, &msg).await,
        Gate::NotAddressed
    );
    // …while a mention still passes.
    let mention = channel_message(None, true, true);
    assert_eq!(
        gate_with_reaction(&adapter, &config, &store, &mention).await,
        Gate::Allow
    );
}

#[tokio::test]
async fn gate_thread_override_wins_then_falls_back_to_chat() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let store = gate_store().await;
    // Chat requires mentions (override); the thread opts out.
    store
        .set_mention_override("gate", "chat-1", true)
        .await
        .unwrap();
    store
        .set_mention_override("gate", "thread-1", false)
        .await
        .unwrap();
    let mut msg = channel_message(Some("thread-1"), true, true);
    msg.is_mention = false;

    assert_eq!(
        gate_with_reaction(&adapter, &feishu_gate_config(), &store, &msg).await,
        Gate::Allow
    );

    // Thread override cleared: the chat override applies.
    store
        .clear_mention_override("gate", "thread-1")
        .await
        .unwrap();
    assert_eq!(
        gate_with_reaction(&adapter, &feishu_gate_config(), &store, &msg).await,
        Gate::NotAddressed
    );
}

#[test]
fn mention_command_parse() {
    assert!(matches!(
        parse_channel_command(Some("/mention")),
        ChannelCommand::Mention(None)
    ));
    assert!(matches!(
        parse_channel_command(Some("/mention on")),
        ChannelCommand::Mention(Some(OverrideMode::On))
    ));
    assert!(matches!(
        parse_channel_command(Some("/mention off")),
        ChannelCommand::Mention(Some(OverrideMode::Off))
    ));
    assert!(matches!(
        parse_channel_command(Some("/mention reset")),
        ChannelCommand::Mention(Some(OverrideMode::Reset))
    ));
    assert!(matches!(
        parse_channel_command(Some("/mention loudly")),
        ChannelCommand::InvalidMentionCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/mention off now")),
        ChannelCommand::InvalidMentionCommand
    ));
    // `/mentions` reports as an unknown command.
    assert!(matches!(
        parse_channel_command(Some("/mentions")),
        ChannelCommand::Unknown(_)
    ));
    assert!(HELP_TEXT.contains("/mention"));
}

#[test]
fn threads_command_parse() {
    assert!(matches!(
        parse_channel_command(Some("/threads")),
        ChannelCommand::Threads(None)
    ));
    assert!(matches!(
        parse_channel_command(Some("/threads on")),
        ChannelCommand::Threads(Some(OverrideMode::On))
    ));
    assert!(matches!(
        parse_channel_command(Some("/threads off")),
        ChannelCommand::Threads(Some(OverrideMode::Off))
    ));
    assert!(matches!(
        parse_channel_command(Some("/threads reset")),
        ChannelCommand::Threads(Some(OverrideMode::Reset))
    ));
    assert!(matches!(
        parse_channel_command(Some("/threads loudly")),
        ChannelCommand::InvalidThreadsCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/threads off now")),
        ChannelCommand::InvalidThreadsCommand
    ));
    assert!(HELP_TEXT.contains("/threads"));
}

/// The chat override wins over the channel config; other chats and a
/// cleared override fall back to it.
#[tokio::test]
async fn rit_override_resolution() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let config = ChannelConfig {
        name: "mock".to_string(),
        ..Default::default()
    };
    assert!(!config.reply_in_thread);
    assert!(!resolve_reply_in_thread(&store, &config, "oc_1").await);

    store.set_rit_override("mock", "oc_1", true).await.unwrap();
    assert!(resolve_reply_in_thread(&store, &config, "oc_1").await);
    // Other chats are unaffected.
    assert!(!resolve_reply_in_thread(&store, &config, "oc_2").await);

    store.clear_rit_override("mock", "oc_1").await.unwrap();
    assert!(!resolve_reply_in_thread(&store, &config, "oc_1").await);
}

/// `/threads` end to end: admin gate, chat scoping, query/reset texts,
/// DM refusal — and the override flips the session mapping mode.
#[tokio::test]
async fn threads_command_query_set_reset() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "app".to_string(),
            app_secret: "secret".to_string(),
        },
        require_mention: true,
        admin_users: vec!["ou_admin".to_string()],
        ..Default::default()
    };
    let msg = |user: &str, text: &str, group: bool| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: user.to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some(text.to_string()),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: group,
        create_time: Some(1000),
        doc_comment: None,
    };
    let handle = |m: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            m,
            &obs,
            &adapter,
        )
    };

    // Non-admin mutation: denied, nothing persisted.
    let reply = handle(msg("ou_random", "/threads on", true)).await.unwrap();
    assert_eq!(
        reply.as_deref(),
        Some("Permission denied: not in admin_users.")
    );
    assert_eq!(store.get_rit_override("mock", "oc_1").await.unwrap(), None);

    // Admin sets the chat override on.
    let reply = handle(msg("ou_admin", "/threads on", true)).await.unwrap();
    let text = reply.unwrap();
    assert!(text.contains("`on`"), "ack: {text}");
    assert_eq!(
        store.get_rit_override("mock", "oc_1").await.unwrap(),
        Some(true)
    );
    // …and the mapping mode follows: a top-level group message now keys
    // by its own message id instead of the chat id.
    assert!(resolve_reply_in_thread(&store, &config, "oc_1").await);

    // Query reports the override and its source (sent via the adapter —
    // info replies return `None`).
    let reply = handle(msg("ou_random", "/threads", true)).await.unwrap();
    assert!(reply.is_none(), "info replies go through the adapter");
    let text = last_outgoing_text(&mock).await;
    assert!(text.contains("`on`"), "query: {text}");
    assert!(text.contains("chat override"), "source: {text}");

    // A thread message still queries/mutates the chat scope.
    let mut thread_msg = msg("ou_random", "/threads", true);
    thread_msg.thread_id = Some("omt_1".to_string());
    let reply = handle(thread_msg).await.unwrap();
    assert!(reply.is_none());
    assert!(last_outgoing_text(&mock).await.contains("chat override"));

    // Reset: back to the channel default.
    let reply = handle(msg("ou_admin", "/threads reset", true))
        .await
        .unwrap();
    let text = reply.unwrap();
    assert!(text.contains("channel default"), "reset ack: {text}");
    assert_eq!(store.get_rit_override("mock", "oc_1").await.unwrap(), None);
    assert!(!resolve_reply_in_thread(&store, &config, "oc_1").await);

    // DMs need no override and persist nothing.
    let reply = handle(msg("ou_admin", "/threads on", false)).await.unwrap();
    assert!(reply.unwrap().contains("DM"));
    assert_eq!(store.get_rit_override("mock", "oc_1").await.unwrap(), None);
}

/// `/mention` end to end: admin gate, container scoping, query/reset
/// texts, DM no-op — and the command never advances the history cursor.
#[tokio::test]
async fn mention_command_query_set_reset() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "app".to_string(),
            app_secret: "secret".to_string(),
        },
        require_mention: true,
        admin_users: vec!["ou_admin".to_string()],
        ..Default::default()
    };
    let msg = |user: &str, text: &str, thread: Option<&str>, group: bool| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: user.to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some(text.to_string()),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        image_keys: vec![],
        thread_id: thread.map(str::to_string),
        root_id: None,
        parent_id: None,
        is_group: group,
        create_time: Some(1000),
        doc_comment: None,
    };
    let handle = |m: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            m,
            &obs,
            &adapter,
        )
    };

    // Non-admin mutation: denied, nothing persisted.
    let reply = handle(msg("ou_random", "/mention off", None, true))
        .await
        .unwrap();
    assert_eq!(
        reply.as_deref(),
        Some("Permission denied: not in admin_users.")
    );
    assert_eq!(
        store.get_mention_override("mock", "oc_1").await.unwrap(),
        None
    );

    // Admin sets the chat override off.
    let reply = handle(msg("ou_admin", "/mention off", None, true))
        .await
        .unwrap();
    let text = reply.unwrap();
    assert!(text.contains("`off`"), "ack: {text}");
    assert!(text.contains("chat"), "scope: {text}");
    assert_eq!(
        store.get_mention_override("mock", "oc_1").await.unwrap(),
        Some(false)
    );

    // Query reports the override and its source (sent via the adapter —
    // info replies return `None`).
    let reply = handle(msg("ou_random", "/mention", None, true))
        .await
        .unwrap();
    assert!(reply.is_none(), "info replies go through the adapter");
    let text = last_outgoing_text(&mock).await;
    assert!(text.contains("`off`"), "query: {text}");
    assert!(text.contains("chat override"), "source: {text}");

    // A thread message falls back to the chat override in the query…
    let reply = handle(msg("ou_random", "/mention", Some("omt_1"), true))
        .await
        .unwrap();
    assert!(reply.is_none());
    let text = last_outgoing_text(&mock).await;
    assert!(text.contains("chat override"), "thread fallback: {text}");

    // …but mutates its own thread container.
    let reply = handle(msg("ou_admin", "/mention on", Some("omt_1"), true))
        .await
        .unwrap();
    assert!(reply.unwrap().contains("thread"));
    assert_eq!(
        store.get_mention_override("mock", "omt_1").await.unwrap(),
        Some(true)
    );

    // Reset the thread override: back to the chat override.
    let reply = handle(msg("ou_admin", "/mention reset", Some("omt_1"), true))
        .await
        .unwrap();
    let text = reply.unwrap();
    assert!(text.contains("chat override"), "reset ack: {text}");
    assert_eq!(
        store.get_mention_override("mock", "omt_1").await.unwrap(),
        None
    );

    // DMs need no override and persist nothing.
    let reply = handle(msg("ou_admin", "/mention off", None, false))
        .await
        .unwrap();
    assert!(reply.unwrap().contains("DM"));

    // The command is control-plane: no history-cursor advance.
    let (_pool2, store2) = create_test_pool().await;
    let store2: Arc<dyn ChannelStore> = store2;
    let mut cursor_msg = channel_message(None, true, true);
    cursor_msg.raw_text = Some("/mention off".to_string());
    cursor_msg.create_time = Some(1000);
    advance_history_cursor(&config, &store2, "mock", &cursor_msg).await;
    assert_eq!(
        store2.get_history_cursor("mock", "chat-1").await.unwrap(),
        None
    );
}

// ── Passive mid-run receipts (non-addressed messages) ───────────────

/// A mention-missed message in a running session's conversation counts
/// as a mid-run post, so the run's reply sinks below it.
#[tokio::test]
async fn passive_receipt_records_for_running_session() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let obs = ObsTracker::new();
    let config = feishu_gate_config();
    let sid = SessionId::new();
    store
        .save_mapping(&config.name, "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();

    let mut msg = channel_message(None, true, true);
    msg.is_mention = false;
    record_passive_receipt(&config.name, &config, &store, &obs, &msg, |_| true).await;

    assert!(obs.has_mid_run_posts(&sid));
}

/// Idle session: a non-addressed message is not a mid-run post.
#[tokio::test]
async fn passive_receipt_skips_idle_session() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let obs = ObsTracker::new();
    let config = feishu_gate_config();
    let sid = SessionId::new();
    store
        .save_mapping(&config.name, "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();

    let mut msg = channel_message(None, true, true);
    msg.is_mention = false;
    record_passive_receipt(&config.name, &config, &store, &obs, &msg, |_| false).await;

    assert!(!obs.has_mid_run_posts(&sid));
}

/// No session mapped for the message's conversation — nothing to record.
#[tokio::test]
async fn passive_receipt_skips_unmapped_conversation() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let obs = ObsTracker::new();
    let config = feishu_gate_config();
    let sid = SessionId::new();

    let mut msg = channel_message(None, true, true);
    msg.is_mention = false;
    record_passive_receipt(&config.name, &config, &store, &obs, &msg, |_| true).await;

    assert!(!obs.has_mid_run_posts(&sid));
}

/// Commands are not conversation — skipped like addressed commands.
#[tokio::test]
async fn passive_receipt_skips_commands() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let obs = ObsTracker::new();
    let config = feishu_gate_config();
    let sid = SessionId::new();
    store
        .save_mapping(&config.name, "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();

    let mut msg = channel_message(None, true, true);
    msg.is_mention = false;
    msg.raw_text = Some("/stop".to_string());
    record_passive_receipt(&config.name, &config, &store, &obs, &msg, |_| true).await;

    assert!(!obs.has_mid_run_posts(&sid));
}

/// In-thread message (reply_in_thread): the receipt lands on the
/// thread's session via the root key.
#[tokio::test]
async fn passive_receipt_records_in_thread() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let obs = ObsTracker::new();
    let config = ChannelConfig {
        reply_in_thread: true,
        ..feishu_gate_config()
    };
    let sid = SessionId::new();
    store
        .save_mapping(&config.name, "root-1", &sid, "chat-1", Some("root-1"))
        .await
        .unwrap();

    let mut msg = channel_message(Some("t1"), true, true);
    msg.is_mention = false;
    msg.root_id = Some("root-1".to_string());
    record_passive_receipt(&config.name, &config, &store, &obs, &msg, |_| true).await;

    assert!(obs.has_mid_run_posts(&sid));
}

/// Observability off: no receipts at all.
#[tokio::test]
async fn passive_receipt_skips_when_observability_off() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let obs = ObsTracker::new();
    let config = ChannelConfig {
        observability: false,
        ..feishu_gate_config()
    };
    let sid = SessionId::new();
    store
        .save_mapping(&config.name, "chat-1", &sid, "chat-1", None)
        .await
        .unwrap();

    let mut msg = channel_message(None, true, true);
    msg.is_mention = false;
    record_passive_receipt(&config.name, &config, &store, &obs, &msg, |_| true).await;

    assert!(!obs.has_mid_run_posts(&sid));
}

// ── Live Feishu E2E (manual) ─────────────────────────────────────────

/// Live-e2e env vars: feishu credentials plus a real thread —
/// `YOMI_E2E_ROOT` its root message, `YOMI_E2E_TRIGGER` an in-thread
/// @bot message replying to that root. `None` (with a skip note) when
/// unset.
///
/// ```sh
/// YOMI_E2E_FEISHU_APP_ID=… YOMI_E2E_FEISHU_APP_SECRET=… \
/// YOMI_E2E_THREAD=omt_… YOMI_E2E_ROOT=om_… YOMI_E2E_TRIGGER=om_… \
/// cargo test -p kernel e2e_feishu -- --ignored --nocapture
/// ```
fn e2e_vars() -> Option<[String; 5]> {
    let keys = [
        "YOMI_E2E_FEISHU_APP_ID",
        "YOMI_E2E_FEISHU_APP_SECRET",
        "YOMI_E2E_THREAD",
        "YOMI_E2E_ROOT",
        "YOMI_E2E_TRIGGER",
    ];
    if keys.iter().any(|k| std::env::var(k).is_err()) {
        eprintln!("YOMI_E2E_* env vars not set; skipping live e2e");
        return None;
    }
    let v: Vec<String> = keys.iter().map(|k| std::env::var(k).unwrap()).collect();
    Some(v.try_into().expect("5 env vars"))
}

/// Run `context_prefix` for a fresh-session, in-thread trigger replying
/// to the root, against the real Feishu adapter.
async fn e2e_setup(
    app_id: String,
    app_secret: String,
    thread_id: String,
    root_id: String,
    trigger_id: String,
) -> (Arc<dyn PlatformAdapter>, Vec<ContentBlock>) {
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(crate::channels::feishu::FeishuAdapter::new(
        app_id, app_secret,
    ));
    let mut msg = group_msg(Some(thread_id));
    msg.root_id = Some(root_id.clone());
    msg.parent_id = Some(root_id);
    msg.external_message_id = Some(trigger_id);

    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let config = ChannelConfig {
        reply_in_thread: true,
        ..ChannelConfig::default()
    };
    let blocks = context_prefix(&adapter, &config, &store, "feishu", &msg, false).await;
    (adapter, blocks)
}

/// End-to-end against the real Feishu API: a trigger inside a
/// human-created thread must deliver the thread root — via the quoted
/// block on a fresh session, image included — exactly once. Point the
/// env vars at a thread whose root is an image message (exercises the
/// download path too).
#[tokio::test]
#[ignore = "hits the real Feishu API; env-var driven, run manually"]
async fn e2e_feishu_fresh_thread_root_delivered_once() {
    let Some([app_id, app_secret, thread_id, root_id, trigger_id]) = e2e_vars() else {
        return;
    };
    let (adapter, blocks) = e2e_setup(
        app_id,
        app_secret,
        thread_id.clone(),
        root_id.clone(),
        trigger_id,
    )
    .await;

    // Platform behavior the fix relies on: the thread container listing
    // includes the root message.
    let items = adapter
        .fetch_history(
            &crate::channels::HistoryContainer::Thread(thread_id),
            None,
            20,
        )
        .await
        .expect("fetch history");
    assert!(
        items.iter().any(|m| m.message_id == root_id),
        "thread listing must include the root"
    );

    // Delivered exactly once: as the quoted message (fresh thread), with
    // the root's image; the history block stays empty here (the only
    // other thread messages are the trigger and our own card).
    let text = blocks_text(&blocks);
    assert!(text.contains("<quoted_message>"), "root quoted: {text}");
    let images = blocks
        .iter()
        .filter(|b| matches!(b, ContentBlock::ImageUrl { .. }))
        .count();
    assert_eq!(images, 1, "root image downloaded exactly once: {blocks:?}");
}

/// Same, against a thread whose root is itself a quote-reply: the whole
/// quote chain must arrive — the ancestor (an image message, exercising
/// the download path) first, then the root's text. Point the env vars
/// at such a thread (root = a text quote-reply of an image message).
#[tokio::test]
#[ignore = "hits the real Feishu API; env-var driven, run manually"]
async fn e2e_feishu_quoted_root_chain() {
    let Some([app_id, app_secret, thread_id, root_id, trigger_id]) = e2e_vars() else {
        return;
    };
    let (adapter, blocks) =
        e2e_setup(app_id, app_secret, thread_id, root_id.clone(), trigger_id).await;
    let text = blocks_text(&blocks);
    assert!(text.contains("<quoted_message>"), "root quoted: {text}");
    // The chain's two links, oldest first: the ancestor (an image) then
    // the root's own text (derived live, not hardcoded).
    let root_text = adapter
        .fetch_message(&root_id)
        .await
        .expect("fetch root")
        .expect("root exists")
        .text;
    let snippet: String = root_text.trim().chars().take(8).collect();
    let ancestor = text.find("[image]").expect("ancestor line: {text}");
    let root_pos = text.find(&snippet).expect("root line: {text}");
    assert!(ancestor < root_pos, "chronological: {text}");
    let images = blocks
        .iter()
        .filter(|b| matches!(b, ContentBlock::ImageUrl { .. }))
        .count();
    assert_eq!(images, 1, "ancestor image downloaded once: {blocks:?}");
}

// ── /subscribe & /unsubscribe ─────────────────────────────────────

#[test]
fn test_parse_subscribe_commands() {
    assert!(matches!(
        parse_channel_command(Some("/subscribe")),
        ChannelCommand::Subscribe {
            recursive: false,
            target_chat_id: None
        }
    ));
    assert!(matches!(
        parse_channel_command(Some("/subscribe -r")),
        ChannelCommand::Subscribe {
            recursive: true,
            target_chat_id: None
        }
    ));
    assert!(matches!(
        parse_channel_command(Some("/subscribe --recursive")),
        ChannelCommand::Subscribe {
            recursive: true,
            target_chat_id: None
        }
    ));
    assert!(matches!(
        parse_channel_command(Some("/subscribe oc_abc -r")),
        ChannelCommand::Subscribe {
            recursive: true,
            target_chat_id: Some(ref t)
        } if t == "oc_abc"
    ));
    assert!(matches!(
        parse_channel_command(Some("/subscribe@yomi_bot oc_abc")),
        ChannelCommand::Subscribe {
            recursive: false,
            target_chat_id: Some(ref t)
        } if t == "oc_abc"
    ));
    assert!(matches!(
        parse_channel_command(Some("/subscribe foo")),
        ChannelCommand::InvalidSubscribeCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/subscribe oc_a oc_b")),
        ChannelCommand::InvalidSubscribeCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/unsubscribe")),
        ChannelCommand::Unsubscribe
    ));
    assert!(matches!(
        parse_channel_command(Some("/unsubscribe now")),
        ChannelCommand::InvalidSubscribeCommand
    ));
    // Prefix lookalikes report as unknown commands.
    assert!(matches!(
        parse_channel_command(Some("/subscribed")),
        ChannelCommand::Unknown(_)
    ));
    assert!(HELP_TEXT.contains("/subscribe"));
    assert!(HELP_TEXT.contains("/unsubscribe"));
}

/// `/subscribe` scope resolution: chat level binds the chat id (never the
/// per-message RIT key), in-thread binds the thread's mapping key;
/// recursion is chat-level only. `/unsubscribe` removes by scope+user.
#[tokio::test]
async fn test_subscribe_command_scopes() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "feishu".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "a".into(),
            app_secret: "s".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let msg = |raw: &str, thread: Option<(&str, &str)>| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_a".to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![],
        image_keys: vec![],
        thread_id: thread.map(|(t, _)| t.to_string()),
        root_id: thread.map(|(_, r)| r.to_string()),
        parent_id: None,
        is_group: true,
        create_time: None,
        doc_comment: None,
    };

    // Chat level: binds the chat id; recursive recorded.
    let reply = handle_incoming_message(
        "feishu",
        &config,
        &store,
        Arc::clone(&kernel),
        msg("/subscribe -r", None),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(
        reply
            .as_deref()
            .unwrap()
            .contains("including all its threads"),
        "{reply:?}"
    );
    let subs = store
        .list_matching_run_subscriptions("feishu", "oc_1", "oc_1")
        .await
        .unwrap();
    assert_eq!(subs.len(), 1);
    assert!(subs[0].recursive);
    assert_eq!(subs[0].scope_key, "oc_1");

    // In-thread: recursion refused; plain subscribe binds the thread key.
    let reply = handle_incoming_message(
        "feishu",
        &config,
        &store,
        Arc::clone(&kernel),
        msg("/subscribe -r", Some(("omt_1", "om_root"))),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(
        reply
            .as_deref()
            .unwrap()
            .contains("only meaningful at chat level"),
        "{reply:?}"
    );
    let reply = handle_incoming_message(
        "feishu",
        &config,
        &store,
        Arc::clone(&kernel),
        msg("/subscribe", Some(("omt_1", "om_root"))),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(
        reply.as_deref().unwrap().contains("this thread"),
        "{reply:?}"
    );
    // The thread run matches the thread sub AND the recursive chat sub.
    let subs = store
        .list_matching_run_subscriptions("feishu", "omt_1", "oc_1")
        .await
        .unwrap();
    assert_eq!(subs.len(), 2);
    let thread_sub = subs
        .iter()
        .find(|s| s.scope_key == "omt_1")
        .expect("thread subscription present");
    assert!(!thread_sub.recursive);

    // A target chat is persisted.
    let reply = handle_incoming_message(
        "feishu",
        &config,
        &store,
        Arc::clone(&kernel),
        msg("/subscribe oc_2", None),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(reply.as_deref().unwrap().contains("oc_2"), "{reply:?}");
    let subs = store
        .list_matching_run_subscriptions("feishu", "oc_1", "oc_1")
        .await
        .unwrap();
    assert_eq!(subs[0].target_chat_id.as_deref(), Some("oc_2"));

    // Unsubscribe removes by scope+user.
    let reply = handle_incoming_message(
        "feishu",
        &config,
        &store,
        Arc::clone(&kernel),
        msg("/unsubscribe", None),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(reply.as_deref(), Some("✅ Unsubscribed."));
    let reply = handle_incoming_message(
        "feishu",
        &config,
        &store,
        Arc::clone(&kernel),
        msg("/unsubscribe", None),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert_eq!(reply.as_deref(), Some("You have no subscription here."));
}

/// Non-Feishu platforms refuse subscriptions (no message links / DMs).
#[tokio::test]
async fn test_subscribe_refused_on_telegram() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "tg".to_string(),
        enabled: true,
        platform: PlatformConfig::Telegram { token: "t".into() },
        require_mention: false,
        ..Default::default()
    };
    let msg = ChannelMessage {
        external_chat_id: "chat1".to_string(),
        external_user_id: "u1".to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some("/subscribe".to_string()),
        content: vec![],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: false,
        create_time: None,
        doc_comment: None,
    };
    let reply = handle_incoming_message("tg", &config, &store, kernel, msg, &obs, &adapter)
        .await
        .unwrap();
    assert_eq!(
        reply.as_deref(),
        Some("This platform does not support subscriptions yet.")
    );
}

/// In `reply_in_thread` group chats a non-recursive chat subscription can
/// never match a run (every trigger opens a new thread) — the ack says so.
#[tokio::test]
async fn test_subscribe_ack_warns_in_rit_groups() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "feishu".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "a".into(),
            app_secret: "s".into(),
        },
        require_mention: false,
        reply_in_thread: true,
        ..Default::default()
    };
    let msg = |raw: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_a".to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: None,
        doc_comment: None,
    };

    let reply = handle_incoming_message(
        "feishu",
        &config,
        &store,
        Arc::clone(&kernel),
        msg("/subscribe"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(
        reply.as_deref().unwrap().contains("does NOT cover"),
        "{reply:?}"
    );

    // Recursive gets no warning — it covers everything.
    let reply = handle_incoming_message(
        "feishu",
        &config,
        &store,
        kernel,
        msg("/subscribe -r"),
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(
        !reply.as_deref().unwrap().contains("does NOT cover"),
        "{reply:?}"
    );
}

/// Adapter capturing subscription notifications (DMs and chat cards).
#[derive(Default)]
struct NotifyMockAdapter {
    dms: tokio::sync::Mutex<Vec<(String, String)>>,
    cards: tokio::sync::Mutex<Vec<(String, String)>>,
    /// Returned by `fetch_message` (the thread root / trigger message).
    message: tokio::sync::Mutex<Option<HistoryMessage>>,
    /// Message ids `fetch_message` was called with, in order.
    fetch_calls: tokio::sync::Mutex<Vec<String>>,
    fetch_fail: std::sync::atomic::AtomicBool,
    /// Returned by `fetch_user_name` (None = no contact permission).
    user_name: Option<String>,
}

#[async_trait::async_trait]
impl PlatformAdapter for NotifyMockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelEvent>,
        cancel: CancellationToken,
    ) -> std::result::Result<(), crate::channels::ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        _external_chat_id: &str,
        _blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        Ok(None)
    }

    async fn send_card(
        &self,
        external_chat_id: &str,
        card_json: &str,
        _reply_msg_id: Option<&str>,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        self.cards
            .lock()
            .await
            .push((external_chat_id.to_string(), card_json.to_string()));
        Ok(Some("card1".to_string()))
    }

    async fn send_direct_card(
        &self,
        user_id: &str,
        card_json: &str,
    ) -> std::result::Result<Option<String>, crate::channels::ChannelError> {
        self.dms
            .lock()
            .await
            .push((user_id.to_string(), card_json.to_string()));
        Ok(Some("dm1".to_string()))
    }

    async fn message_link(&self, _chat_id: &str, message_id: &str) -> Option<String> {
        Some(format!("link://{message_id}"))
    }

    async fn fetch_chat_name(&self, _chat_id: &str) -> Option<String> {
        Some("测试群".to_string())
    }

    async fn fetch_message(
        &self,
        message_id: &str,
    ) -> std::result::Result<Option<HistoryMessage>, crate::channels::ChannelError> {
        self.fetch_calls.lock().await.push(message_id.to_string());
        if self.fetch_fail.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(crate::channels::ChannelError::Platform(
                "mock fetch failure".into(),
            ));
        }
        Ok(self.message.lock().await.clone())
    }

    async fn fetch_user_name(&self, _open_id: &str) -> Option<String> {
        self.user_name.clone()
    }
}

/// Run-completion notification: DMs per subscriber (deduplicated across
/// overlapping subscriptions), one mentioned card per target chat; exact +
/// recursive matching; nothing without a reply id.
#[tokio::test]
async fn test_notify_run_subscribers() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    store
        .save_run_subscription("feishu", "omt_1", "oc_1", false, "ou_dm", None)
        .await
        .unwrap();
    store
        .save_run_subscription("feishu", "oc_1", "oc_1", true, "ou_rec", None)
        .await
        .unwrap();
    store
        .save_run_subscription("feishu", "omt_1", "oc_1", false, "ou_grp", Some("oc_2"))
        .await
        .unwrap();
    // Overlapping subscriptions (exact thread + recursive chat) for one
    // user — must still produce a single DM.
    store
        .save_run_subscription("feishu", "omt_1", "oc_1", false, "ou_dup", None)
        .await
        .unwrap();
    store
        .save_run_subscription("feishu", "oc_1", "oc_1", true, "ou_dup", None)
        .await
        .unwrap();

    let mock = Arc::new(NotifyMockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let routing = SessionRouting {
        channel_name: "feishu".to_string(),
        external_chat_id: "oc_1".to_string(),
        reply_msg_id: None,
        mapping_key: "omt_1".to_string(),
        doc_comment: None,
    };

    // No delivered reply → nothing to link to, no notification.
    notify_run_subscribers(
        &store,
        &adapter,
        &routing,
        None,
        RunEndStatus::Completed,
        &SessionId::new(),
        &std::sync::Weak::new(),
        &ObsTracker::new(),
    )
    .await;
    assert!(mock.dms.lock().await.is_empty());
    assert!(mock.cards.lock().await.is_empty());

    // Cancelled runs are never notified (the initiator stopped it).
    notify_run_subscribers(
        &store,
        &adapter,
        &routing,
        Some("om_reply"),
        RunEndStatus::Cancelled,
        &SessionId::new(),
        &std::sync::Weak::new(),
        &ObsTracker::new(),
    )
    .await;
    assert!(mock.dms.lock().await.is_empty());
    assert!(mock.cards.lock().await.is_empty());

    notify_run_subscribers(
        &store,
        &adapter,
        &routing,
        Some("om_reply"),
        RunEndStatus::Completed,
        &SessionId::new(),
        &std::sync::Weak::new(),
        &ObsTracker::new(),
    )
    .await;
    let dms = mock.dms.lock().await;
    let mut dm_users: Vec<_> = dms.iter().map(|(u, _)| u.as_str()).collect();
    dm_users.sort_unstable();
    assert_eq!(dm_users, ["ou_dm", "ou_dup", "ou_rec"]);
    let dm_card = &dms[0].1;
    assert!(dm_card.contains("link://om_reply"), "{dm_card}");
    assert!(dm_card.contains("finished"), "{dm_card}");
    assert!(dm_card.contains("测试群"), "{dm_card}");
    assert!(dm_card.contains("card_link"), "{dm_card}");
    drop(dms);
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].0, "oc_2");
    assert!(cards[0].1.contains("<at id=ou_grp></at>"), "{}", cards[0].1);
    drop(cards);

    // Failed runs say so.
    notify_run_subscribers(
        &store,
        &adapter,
        &routing,
        Some("om_x"),
        RunEndStatus::Failed,
        &SessionId::new(),
        &std::sync::Weak::new(),
        &ObsTracker::new(),
    )
    .await;
    let dms = mock.dms.lock().await;
    let failed_card = &dms.last().unwrap().1;
    assert!(failed_card.contains("failed"), "{failed_card}");
    assert!(failed_card.contains("❌"), "{failed_card}");
}

fn notify_trigger_message() -> HistoryMessage {
    HistoryMessage {
        message_id: "omt_1".into(),
        create_time: 1,
        sender_id: "ou_author".into(),
        text: "@_user_1 帮我看下\n这个 run   怎么样".into(),
        image_keys: vec![],
        parent_id: None,
    }
}

async fn notify_quote_setup() -> (
    Arc<dyn ChannelStore>,
    Arc<NotifyMockAdapter>,
    SessionRouting,
) {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    store
        .save_run_subscription("feishu", "omt_1", "oc_1", false, "ou_dm", None)
        .await
        .unwrap();
    let mock = Arc::new(NotifyMockAdapter::default());
    let routing = SessionRouting {
        channel_name: "feishu".to_string(),
        external_chat_id: "oc_1".to_string(),
        reply_msg_id: None,
        mapping_key: "omt_1".to_string(),
        doc_comment: None,
    };
    (store, mock, routing)
}

/// The notify card quotes the thread root/trigger message, attributed
/// when the author's name resolves; mentions stripped, whitespace flat.
#[tokio::test]
async fn notify_card_quotes_trigger_message_with_author() {
    let (store, _unused, routing) = notify_quote_setup().await;
    let mut mock = NotifyMockAdapter {
        user_name: Some("李华儒".to_string()),
        ..Default::default()
    };
    *mock.message.get_mut() = Some(notify_trigger_message());
    let mock = Arc::new(mock);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    notify_run_subscribers(
        &store,
        &adapter,
        &routing,
        Some("om_r"),
        RunEndStatus::Completed,
        &SessionId::new(),
        &std::sync::Weak::new(),
        &ObsTracker::new(),
    )
    .await;
    let dms = mock.dms.lock().await;
    let card = &dms[0].1;
    assert!(
        card.contains("> 李华儒：帮我看下 这个 run 怎么样"),
        "{card}"
    );
    assert!(!card.contains("@_user_1"), "{card}");
}

/// Without contact permission the quote line carries no author prefix.
#[tokio::test]
async fn notify_card_quote_omits_author_when_name_unresolved() {
    let (store, mock, routing) = notify_quote_setup().await;
    *mock.message.lock().await = Some(notify_trigger_message());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    notify_run_subscribers(
        &store,
        &adapter,
        &routing,
        Some("om_r"),
        RunEndStatus::Completed,
        &SessionId::new(),
        &std::sync::Weak::new(),
        &ObsTracker::new(),
    )
    .await;
    let dms = mock.dms.lock().await;
    let card = &dms[0].1;
    assert!(card.contains("> 帮我看下 这个 run 怎么样"), "{card}");
    assert!(!card.contains("：帮我看下"), "{card}");
}

/// Trigger fetch failing (and no session title available — dead kernel
/// weak ref) degrades to the one-line card, notification still sent.
#[tokio::test]
async fn notify_card_without_quote_when_context_unavailable() {
    let (store, mock, routing) = notify_quote_setup().await;
    mock.fetch_fail
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();

    notify_run_subscribers(
        &store,
        &adapter,
        &routing,
        Some("om_r"),
        RunEndStatus::Completed,
        &SessionId::new(),
        &std::sync::Weak::new(),
        &ObsTracker::new(),
    )
    .await;
    let dms = mock.dms.lock().await;
    assert_eq!(dms.len(), 1);
    assert!(!dms[0].1.contains("> "), "no quote line: {}", dms[0].1);
}

/// Chat-level sessions key their mapping on the chat id — not fetchable
/// as a message, so no quote (the title fallback needs a live kernel).
#[tokio::test]
async fn notify_card_chat_level_session_skips_message_fetch() {
    let (store, mock, mut routing) = notify_quote_setup().await;
    // Chat-level subscription matching the chat-level routing below.
    store
        .save_run_subscription("feishu", "oc_1", "oc_1", false, "ou_chat", None)
        .await
        .unwrap();
    *mock.message.lock().await = Some(notify_trigger_message());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    routing.mapping_key = "oc_1".to_string();

    notify_run_subscribers(
        &store,
        &adapter,
        &routing,
        Some("om_r"),
        RunEndStatus::Completed,
        &SessionId::new(),
        &std::sync::Weak::new(),
        &ObsTracker::new(),
    )
    .await;
    let dms = mock.dms.lock().await;
    assert!(!dms[0].1.contains("> "), "no quote line: {}", dms[0].1);
}

/// The quote prefers the session's latest user message — the very
/// message the settle ✅ lands on — over the thread root.
#[tokio::test]
async fn notify_card_quotes_reaction_target_over_root() {
    let (store, mock, routing) = notify_quote_setup().await;
    *mock.message.lock().await = Some(notify_trigger_message());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = ObsTracker::new();
    let sid = SessionId::new();
    obs.record_user_msg(&sid, "om_exact".to_string());

    notify_run_subscribers(
        &store,
        &adapter,
        &routing,
        Some("om_r"),
        RunEndStatus::Completed,
        &sid,
        &std::sync::Weak::new(),
        &obs,
    )
    .await;
    assert_eq!(
        mock.fetch_calls.lock().await.as_slice(),
        ["om_exact"],
        "fetched the reaction target, not the root omt_1"
    );
    let dms = mock.dms.lock().await;
    assert!(
        dms[0].1.contains("> 帮我看下 这个 run 怎么样"),
        "{}",
        dms[0].1
    );
}

#[test]
fn notify_quote_snippet_normalizes_and_caps() {
    // Leading mentions stripped, whitespace flattened.
    assert_eq!(
        notify_quote_snippet("  @_user_1 @_user_22 你好\n 世界 "),
        "你好 世界"
    );
    // Capped at 50 content chars plus an ellipsis.
    let long = "字".repeat(60);
    let out = notify_quote_snippet(&long);
    assert_eq!(out.chars().count(), 51);
    assert!(out.ends_with('…'));
    // Non-message text passes through; empty stays empty.
    assert_eq!(notify_quote_snippet("[图片]"), "[图片]");
    assert_eq!(notify_quote_snippet("  "), "");
}

#[test]
fn sessions_command_parse() {
    assert!(matches!(
        parse_channel_command(Some("/sessions")),
        ChannelCommand::Sessions(0)
    ));
    assert!(matches!(
        parse_channel_command(Some("/sessions 20")),
        ChannelCommand::Sessions(20)
    ));
    assert!(matches!(
        parse_channel_command(Some("/sessions x")),
        ChannelCommand::InvalidSessionsCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/sessions 1 2")),
        ChannelCommand::InvalidSessionsCommand
    ));
    assert!(HELP_TEXT.contains("/sessions"));
}

/// `/sessions` (admin): lists the channel's recent sessions with jump
/// links, pages via offset; non-admins are refused.
#[tokio::test]
async fn sessions_command_lists_recent_with_links_and_pages() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        admin_users: vec!["ou_admin".to_string()],
        ..Default::default()
    };
    let base = |msg_id: &str, user: &str, raw: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: user.to_string(),
        external_message_id: Some(msg_id.to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![ContentBlock::Text {
            text: raw.to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: None,
        doc_comment: None,
    };
    let handle = |m: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            m,
            &obs,
            &adapter,
        )
    };

    // Two sessions: a chat-level one and a `/thread` one (titled).
    assert_eq!(handle(base("m1", "ou_1", "你好")).await.unwrap(), None);
    assert_eq!(
        handle(base("m2", "ou_1", "/thread 话题讨论"))
            .await
            .unwrap(),
        None
    );
    let sid = store
        .find_mapping("mock", "m2")
        .await
        .unwrap()
        .expect("thread session");
    assert_eq!(wait_for_title(&kernel, &sid).await, "话题讨论");

    // Admin: both sessions listed, newest page header, jump links.
    let reply = handle(base("m3", "ou_admin", "/sessions"))
        .await
        .unwrap()
        .expect("a list reply");
    assert!(reply.contains("Recent sessions (1–2)"), "{reply}");
    assert!(reply.contains("话题讨论"), "{reply}");
    assert!(reply.contains("link://thread/"), "{reply}");
    assert!(!reply.contains("💤"), "{reply}");

    // Offset pages: one entry on page 2, then exhaustion.
    let reply = handle(base("m4", "ou_admin", "/sessions 1"))
        .await
        .unwrap()
        .expect("page 2");
    assert!(reply.contains("(2–2)"), "{reply}");
    assert!(!reply.contains("Next page"), "{reply}");
    let reply = handle(base("m5", "ou_admin", "/sessions 9"))
        .await
        .unwrap()
        .expect("beyond the end");
    assert!(reply.contains("No more sessions"), "{reply}");

    // Non-admin is refused.
    let reply = handle(base("m6", "ou_1", "/sessions"))
        .await
        .unwrap()
        .expect("refusal");
    assert!(reply.contains("admin_users"), "{reply}");
}

/// `/sessions` on card-capable platforms: a fancy card (header, column
/// rows, links), no text reply.
#[tokio::test]
async fn sessions_command_card_rendering() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let mock = Arc::new(MockAdapter::new("mock"));
    mock.status_card_ok
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        admin_users: vec!["ou_admin".to_string()],
        ..Default::default()
    };
    let base = |msg_id: &str, user: &str, raw: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: user.to_string(),
        external_message_id: Some(msg_id.to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![ContentBlock::Text {
            text: raw.to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: None,
        doc_comment: None,
    };
    let handle = |m: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            m,
            &obs,
            &adapter,
        )
    };

    assert_eq!(
        handle(base("m1", "ou_1", "/thread 话题讨论"))
            .await
            .unwrap(),
        None
    );
    let sid = store
        .find_mapping("mock", "m1")
        .await
        .unwrap()
        .expect("session created");
    assert_eq!(wait_for_title(&kernel, &sid).await, "话题讨论");

    let reply = handle(base("m2", "ou_admin", "/sessions")).await.unwrap();
    assert_eq!(reply, None, "card path sends no text reply");

    let cards = mock.cards.lock().await;
    let (chat, card, anchor) = cards.last().expect("a card was sent");
    assert_eq!(chat, "oc_1");
    assert_eq!(anchor, &None, "rit=off: unanchored, same as the text path");
    assert!(card.contains("📋 Recent sessions (1–1)"), "{card}");
    assert!(card.contains("🧵"), "{card}");
    assert!(card.contains("[**话题讨论**](link://thread/"), "{card}");
}

/// `/sessions` recency dividers: rows group under 6h ago / 1d ago /
/// 1w ago labels (card + text alike); the current bucket gets no label.
#[test]
fn sessions_render_groups_by_time_bucket() {
    let entry = |marker: &'static str, title: &str, bucket: usize| SessionEntry {
        marker,
        title: title.to_string(),
        bucket,
        link: Some("link://x".to_string()),
    };
    let entries = vec![
        entry("⚡", "a", 0),
        entry("🧵", "b", 1),
        entry("🧵", "c", 2),
        entry("💬", "d", 3),
    ];

    let card = sessions_card(0, &entries, true);
    for label in ["6h ago", "1d ago", "1w ago", "Next ▶"] {
        assert!(card.contains(label), "{label} in {card}");
    }
    // Next-page rides a small bordered callback button, not a text hint.
    assert!(
        card.contains(r#"{"action":"pg_sessions","offset":10}"#),
        "pg button in {card}"
    );
    let text = sessions_text(0, &entries, true);
    for label in ["── 6h ago ──", "── 1d ago ──", "── 1w ago ──"] {
        assert!(text.contains(label), "{label} in {text}");
    }

    // All-fresh page: no divider labels anywhere.
    let fresh = sessions_card(0, &[entry("⚡", "a", 0), entry("🧵", "b", 0)], false);
    for label in ["6h ago", "1d ago", "1w ago"] {
        assert!(!fresh.contains(label), "{label} unexpected in {fresh}");
    }
}

/// Bucket boundaries: <6h → 0, <24h → 1, <7d → 2, older → 3.
#[test]
fn session_time_bucket_boundaries() {
    let now = chrono::Utc::now();
    let at = |secs_ago: i64| now - chrono::Duration::seconds(secs_ago);
    assert_eq!(session_time_bucket(at(0), now), 0);
    assert_eq!(session_time_bucket(at(6 * 3600 - 1), now), 0);
    assert_eq!(session_time_bucket(at(6 * 3600), now), 1);
    assert_eq!(session_time_bucket(at(24 * 3600 - 1), now), 1);
    assert_eq!(session_time_bucket(at(24 * 3600), now), 2);
    assert_eq!(session_time_bucket(at(7 * 24 * 3600 - 1), now), 2);
    assert_eq!(session_time_bucket(at(7 * 24 * 3600), now), 3);
}

/// Title sanitization for `/sessions`: lark_md metacharacters are
/// full-width'd (a crafted title must not break the card markup or
/// inject a foreign link), empties fall back, 30-char cap applies.
#[test]
fn sanitize_session_title_neutralizes_markup() {
    // The review's injection example: no clickable remnants.
    assert_eq!(
        sanitize_session_title("x](https://evil.com/#)[y"),
        "x］（https://evil.com/#）［y"
    );
    assert_eq!(
        sanitize_session_title("**bold** `code` ~~strike~~"),
        "＊＊bold＊＊ ｀code｀ ～～strike～～"
    );
    assert_eq!(sanitize_session_title("<a href>"), "＜a href＞");
    assert_eq!(sanitize_session_title("   "), "(untitled)");

    let long = "长".repeat(40);
    let out = sanitize_session_title(&long);
    assert_eq!(out.chars().count(), 31);
    assert!(out.ends_with('…'));
}

/// User activity touches the session: an incoming trigger refreshes
/// `updated_at`, so session lists order by real recency.
#[tokio::test]
async fn trigger_touches_session_recency() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
    // The conductor loop consumes the input bus (and touches the session
    // per dispatched input) — it only runs after `start`.
    kernel.start();

    let mock = Arc::new(MockAdapter::new("mock"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "fake".into(),
            app_secret: "fake".into(),
        },
        require_mention: false,
        ..Default::default()
    };
    let base = |msg_id: &str, raw: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: "ou_1".to_string(),
        external_message_id: Some(msg_id.to_string()),
        is_mention: true,
        raw_text: Some(raw.to_string()),
        content: vec![ContentBlock::Text {
            text: raw.to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: None,
        doc_comment: None,
    };
    let handle = |m: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            m,
            &obs,
            &adapter,
        )
    };

    assert_eq!(handle(base("m1", "你好")).await.unwrap(), None);
    let sid = store
        .find_mapping("mock", "oc_1")
        .await
        .unwrap()
        .expect("session created");
    let t0 = kernel.get_session(&sid).await.unwrap().updated_at;

    // A second trigger (crossing a second boundary) must refresh
    // updated_at. The touch lands in the conductor's spawned task, so
    // poll briefly instead of reading immediately.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    assert_eq!(handle(base("m2", "在吗")).await.unwrap(), None);
    let mut t1 = t0;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        t1 = kernel.get_session(&sid).await.unwrap().updated_at;
        if t1 != t0 {
            break;
        }
    }
    assert!(t1 > t0, "updated_at not refreshed: {t0} -> {t1}");
    kernel.stop().await;
}

#[tokio::test]
async fn deliver_reply_with_mention_flushes_new_message_without_mid_run_posts() {
    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(crate::channels::obs::ObsTracker::new());
    let sid = SessionId::new();

    obs.handle_event(&adapter, &sid, "chat-1", None, &running_event())
        .await;
    obs.handle_event(&adapter, &sid, "chat-1", None, &tool_start_event())
        .await;
    // 无 mid-run posts，但回复含 <@USER_ID> —— 必须沉底发新消息才会通知。

    let mut buf = run_buffer();
    buf.record_model_end("cc <@ou_abc> 看一下");
    deliver_reply(
        &obs,
        &adapter,
        &test_routing(),
        Some(buf.into_reply()),
        true,
        true,
        true,
        &sid,
        SettleKind::Stopped(&completed()),
        &std::sync::Weak::new(),
    )
    .await;

    // 状态卡冻结为一行终态凭据（无 morph）……
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1, "frozen in place, no morph");
    assert!(patches[0].1.contains("✅ **Done**"));
    assert!(
        !patches[0].1.contains("cc <@"),
        "status card must not morph into the reply"
    );
    drop(patches);
    // ……回复全文沉底发新消息，<@id> 重写为 <at id=>
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 2, "materialize + reply card");
    assert!(cards[1].1.contains("cc <at id=ou_abc></at> 看一下"));
}

// ── /mailbox ─────────────────────────────────────────────────────────

/// `/mailbox` 全链路：admin 门槛、无会话提示、pending 卡片（按钮值）、
/// retract/clear、按钮回调原地刷新、空队列文本。黑洞 listener 占住
/// agent，保证 pending 不被抢先消费（确定性）。
#[tokio::test]
async fn mailbox_command_show_retract_clear_and_card_actions() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = listener.accept().await {
            held.push(s);
        }
    });
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        models: vec![crate::provider::ModelConfig {
            name: "blackhole".into(),
            endpoint: format!("http://{addr}"),
            ..Default::default()
        }],
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
    kernel.start();

    let mock = Arc::new(CardMockAdapter::new());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "app".into(),
            app_secret: "secret".into(),
        },
        require_mention: true,
        admin_users: vec!["ou_admin".to_string()],
        ..Default::default()
    };
    let msg = |user: &str, text: &str| ChannelMessage {
        external_chat_id: "oc_1".to_string(),
        external_user_id: user.to_string(),
        external_message_id: Some("m1".to_string()),
        is_mention: true,
        raw_text: Some(text.to_string()),
        content: vec![ContentBlock::Text {
            text: text.to_string(),
        }],
        image_keys: vec![],
        thread_id: None,
        root_id: None,
        parent_id: None,
        is_group: true,
        create_time: Some(1000),
        doc_comment: None,
    };
    let handle = |m: ChannelMessage| {
        handle_incoming_message(
            "mock",
            &config,
            &store,
            Arc::clone(&kernel),
            m,
            &obs,
            &adapter,
        )
    };

    // 非 admin：拒绝。
    let reply = handle(msg("ou_random", "/mailbox")).await.unwrap();
    assert!(reply.as_deref().unwrap().contains("Permission denied"));

    // 无会话：提示。
    let reply = handle(msg("ou_admin", "/mailbox")).await.unwrap();
    assert!(reply.unwrap().contains("No session yet"));

    // 建会话并占住 agent（首个模型请求挂起）。
    let (sid, _) = get_or_create_session("mock", &store, &kernel, "oc_1", "oc_1", None)
        .await
        .unwrap();
    let text = |t: &str| {
        vec![ContentBlock::Text {
            text: t.to_string(),
        }]
    };
    kernel.send_message(&sid, text("blocker")).await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let phase = kernel.get_session(&sid).await.unwrap().phase;
        if phase == "streaming" {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "agent not blocked");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // 入队：queue 两条 + steer 一条。
    kernel.send_message(&sid, text("task A")).await.unwrap();
    kernel.send_message(&sid, text("task B")).await.unwrap();
    kernel.send_steer(&sid, text("note C")).await;
    loop {
        let snap = kernel.mailbox_snapshot(&sid).await;
        if snap.queue.len() == 2 && snap.steer.len() == 1 {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "items never landed");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    // /mailbox → Pending (3) 卡片，含预览与 mb_ 按钮值。
    let reply = handle(msg("ou_admin", "/mailbox")).await.unwrap();
    assert!(reply.is_none(), "card path returns None");
    let card = {
        let cards = mock.cards.lock().await;
        let Some((_, card)) = cards.last() else {
            panic!("no card sent")
        };
        card.clone()
    };
    assert!(card.contains("⏳ Pending (3)"), "{card}");
    assert!(card.contains("task A") && card.contains("note C"), "{card}");
    assert!(
        card.contains("mb_retract") && card.contains("mb_clear"),
        "{card}"
    );

    // 按钮回调 mb_retract：删除并原地刷新（update_card）。
    let item_id = kernel.mailbox_snapshot(&sid).await.queue[0].id.clone();
    crate::channels::mailbox::handle_card_action(
        "mock",
        &config,
        &kernel,
        &adapter,
        crate::channels::CardAction {
            operator_open_id: "ou_admin".to_string(),
            chat_id: Some("oc_1".to_string()),
            message_id: Some("om_card".to_string()),
            value: serde_json::json!({"action": "mb_retract", "sid": sid.0, "item": item_id.as_str()}),
        },
    )
    .await;
    {
        let snap = kernel.mailbox_snapshot(&sid).await;
        assert!(snap.queue.iter().all(|i| i.id != item_id), "button retract");
        let patches = mock.patches.lock().await;
        assert!(
            patches
                .iter()
                .any(|(mid, card)| mid == "om_card" && card.contains("Pending (2)")),
            "in-place card refresh: {patches:?}"
        );
    }

    // 🔄 刷新按钮：无变更也原地刷新一次（PATCH 到按钮所在卡片）。
    crate::channels::mailbox::handle_card_action(
        "mock",
        &config,
        &kernel,
        &adapter,
        crate::channels::CardAction {
            operator_open_id: "ou_admin".to_string(),
            chat_id: Some("oc_1".to_string()),
            message_id: Some("om_card".to_string()),
            value: serde_json::json!({"action": "mb_refresh", "sid": sid.0}),
        },
    )
    .await;
    {
        let patches = mock.patches.lock().await;
        assert!(
            patches.iter().filter(|(mid, _)| mid == "om_card").count() >= 2,
            "refresh button re-patches the card: {patches:?}"
        );
    }

    // /mailbox retract 1 → 撤掉剩下的 queue 条目（此时 merged=[note C, task B]，#1=note C）。
    let reply = handle(msg("ou_admin", "/mailbox retract 1")).await.unwrap();
    let text = reply.unwrap();
    assert!(text.contains("Retracted #1"), "{text}");
    let snap = kernel.mailbox_snapshot(&sid).await;
    assert!(snap.steer.is_empty(), "retract #1 hits the steer head");
    assert_eq!(snap.queue.len(), 1);

    // /mailbox clear → 全清；再查 → 空文本。
    let reply = handle(msg("ou_admin", "/mailbox clear")).await.unwrap();
    assert!(reply.unwrap().contains("Cleared 1"));
    let reply = handle(msg("ou_admin", "/mailbox")).await.unwrap();
    assert!(reply.unwrap().contains("Mailbox is empty"));

    // 解析：/mb 别名、坏用法。
    assert!(matches!(
        parse_channel_command(Some("/mb")),
        ChannelCommand::Mailbox(crate::channels::mailbox::MailboxSub::Show)
    ));
    assert!(matches!(
        parse_channel_command(Some("/mailbox clear steer")),
        ChannelCommand::Mailbox(crate::channels::mailbox::MailboxSub::Clear(
            crate::comms::MailboxScope::Steer
        ))
    ));
    assert!(matches!(
        parse_channel_command(Some("/mailbox retract 2")),
        ChannelCommand::Mailbox(crate::channels::mailbox::MailboxSub::Retract(2))
    ));
    assert!(matches!(
        parse_channel_command(Some("/mailbox retract 0")),
        ChannelCommand::InvalidMailboxCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/mailbox clear foo")),
        ChannelCommand::InvalidMailboxCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/mailbox retract x")),
        ChannelCommand::InvalidMailboxCommand
    ));
    kernel.stop().await;
}

/// `/q` 带图：图片经延迟下载进入排队消息（与 /steer 同路径），不被丢弃。
#[tokio::test]
async fn queue_command_carries_images() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((s, _)) = listener.accept().await {
            held.push(s);
        }
    });
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        models: vec![crate::provider::ModelConfig {
            name: "blackhole".into(),
            endpoint: format!("http://{addr}"),
            ..Default::default()
        }],
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
    kernel.start();

    let mock = Arc::new(MockAdapter::new("mock"));
    *mock.image_download_ok.lock().await = true;
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let obs = Arc::new(ObsTracker::new());
    let config = ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "app".into(),
            app_secret: "secret".into(),
        },
        require_mention: true,
        ..Default::default()
    };
    let (sid, _) = get_or_create_session("mock", &store, &kernel, "oc_1", "oc_1", None)
        .await
        .unwrap();
    let text = |t: &str| {
        vec![ContentBlock::Text {
            text: t.to_string(),
        }]
    };
    kernel.send_message(&sid, text("blocker")).await.unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if kernel.get_session(&sid).await.unwrap().phase == "streaming" {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "agent not blocked");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    let reply = handle_incoming_message(
        "mock",
        &config,
        &store,
        Arc::clone(&kernel),
        ChannelMessage {
            external_chat_id: "oc_1".to_string(),
            external_user_id: "ou_1".to_string(),
            external_message_id: Some("m1".to_string()),
            is_mention: true,
            raw_text: Some("/q 看图说话".to_string()),
            content: text("/q 看图说话"),
            image_keys: vec!["img_1".to_string()],
            thread_id: None,
            root_id: None,
            parent_id: None,
            is_group: true,
            create_time: Some(1000),
            doc_comment: None,
        },
        &obs,
        &adapter,
    )
    .await
    .unwrap();
    assert!(reply.is_none(), "/q sends no reply");

    let snap = loop {
        let snap = kernel.mailbox_snapshot(&sid).await;
        if snap.queue.len() == 1 {
            break snap;
        }
        assert!(std::time::Instant::now() < deadline, "queue never landed");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    let item = &snap.queue[0];
    assert!(
        item.preview.contains("看图说话"),
        "preview: {}",
        item.preview
    );
    assert!(
        item.blocks_len >= 2,
        "image block must ride along: blocks_len={}",
        item.blocks_len
    );
    kernel.stop().await;
}

// ── suggest_command / /shell ─────────────────────────────────────────

#[test]
fn unknown_command_suggests_close_match() {
    assert_eq!(suggest_command("malbox"), Some("/mailbox"));
    assert_eq!(suggest_command("mailbx"), Some("/mailbox"));
    assert_eq!(suggest_command("inf"), Some("/info"));
    assert_eq!(
        suggest_command("maliobx"),
        Some("/mailbox"),
        "相邻交换算 1 步，真实 typo 应命中"
    );
    assert_eq!(suggest_command("xyzzy"), None);
}

#[test]
fn bg_command_parse() {
    assert!(matches!(
        parse_channel_command(Some("/bg")),
        ChannelCommand::BackgroundTasks { all: false }
    ));
    assert!(matches!(
        parse_channel_command(Some("/bg --all")),
        ChannelCommand::BackgroundTasks { all: true }
    ));
    assert!(matches!(
        parse_channel_command(Some("/bg -a")),
        ChannelCommand::BackgroundTasks { all: true }
    ));
    // `/shell` kept as an alias for the same panel.
    assert!(matches!(
        parse_channel_command(Some("/shell")),
        ChannelCommand::BackgroundTasks { all: false }
    ));
}

/// 卡片回调/RPC 路径与 dispatch 循环并发同 key：get_or_create_session 的
/// 键锁保证只有一个创建者（reviewer 实锤：cfg_model/ChannelNewThread
/// 在循环外并发可达）。
#[tokio::test]
async fn get_or_create_session_concurrent_same_key_single_creator() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();

    let call = || {
        let store = Arc::clone(&store);
        let kernel = Arc::clone(&kernel);
        async move {
            crate::channels::hub_routing::get_or_create_session(
                "feishu", &store, &kernel, "oc_x", "omt_1", None,
            )
            .await
            .unwrap()
        }
    };
    let (r1, r2, r3) = tokio::join!(call(), call(), call());
    assert_eq!(r1.0, r2.0);
    assert_eq!(r2.0, r3.0);
    let fresh = [r1.1, r2.1, r3.1].iter().filter(|reused| !**reused).count();
    assert_eq!(fresh, 1, "exactly one caller must be the creator");
}
