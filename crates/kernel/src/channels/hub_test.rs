use super::*;

use crate::channels::PlatformAdapter;
use crate::types::ContentBlock;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::channels::store::SqliteChannelStore;
use crate::channels::PlatformConfig;
use crate::storage::migrations::run_migrations;
use crate::storage::{SessionStore, SqliteSessionStore};
use sqlx::sqlite::SqlitePoolOptions;

pub struct MockAdapter {
    pub outgoing: tokio::sync::Mutex<Vec<(String, Vec<ContentBlock>)>>,
}

impl MockAdapter {
    pub fn new(_name: impl Into<String>) -> Self {
        Self {
            outgoing: tokio::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelMessage>,
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
        Ok(None)
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
        .create(&id, None, None, None, None, model_key)
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
            history_context: 0,
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
            history_context: 0,
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
}

impl CardMockAdapter {
    fn new() -> Self {
        Self {
            cards: tokio::sync::Mutex::new(Vec::new()),
            patches: tokio::sync::Mutex::new(Vec::new()),
            outgoing: tokio::sync::Mutex::new(Vec::new()),
        }
    }
}

#[async_trait::async_trait]
impl PlatformAdapter for CardMockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelMessage>,
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

    fn supports_status_card(&self) -> bool {
        true
    }
}

fn test_routing() -> SessionRouting {
    SessionRouting {
        channel_name: "feishu".to_string(),
        external_chat_id: "chat-1".to_string(),
        reply_msg_id: None,
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
    assert!(text.contains("Trace · 2 steps · 1 tools"));
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
        parse_channel_command(Some("/model@yomi_bot kimi-k2")),
        ChannelCommand::SwitchModel(ref key) if key == "kimi-k2"
    ));
    assert!(matches!(
        parse_channel_command(Some("/models kimi-k2")),
        ChannelCommand::SwitchModel(ref key) if key == "kimi-k2"
    ));
}

#[test]
fn test_parse_invalid_model_command() {
    assert!(matches!(
        parse_channel_command(Some("/model one two")),
        ChannelCommand::InvalidModelCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("please use /model kimi-k2")),
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
        ChannelCommand::None
    ));
    assert!(matches!(
        parse_channel_command(Some("/queue")),
        ChannelCommand::None
    ));
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
fn test_longer_words_are_not_commands() {
    // Prefix matching must not hijack longer words ("/clearance" would
    // trigger the destructive /clear otherwise).
    assert!(matches!(
        parse_channel_command(Some("/clearance")),
        ChannelCommand::None
    ));
    assert!(matches!(
        parse_channel_command(Some("/helpful")),
        ChannelCommand::None
    ));
    assert!(matches!(
        parse_channel_command(Some("/stopping")),
        ChannelCommand::None
    ));
    assert!(matches!(
        parse_channel_command(Some("/information")),
        ChannelCommand::None
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
        model_info("kimi", "kimi-k2", 256_000),
    ];

    let output = format_model_list(&models, "kimi");

    assert!(output.contains("`claude` · anthropic · `claude-sonnet` · 200k ctx"));
    assert!(output.contains("`kimi` · anthropic · `kimi-k2` · 256k ctx **← current**"));
    assert!(output.contains("/model <model_key>"));
}

#[test]
fn test_format_current_and_unknown_model() {
    let models = vec![model_info("kimi", "kimi-k2", 256_000)];

    let current = format_current_model(&models, "kimi");
    assert!(current.contains("Current model: `kimi`"));
    assert!(current.contains("`kimi-k2`"));

    let unknown = format_unknown_model("missing", &models);
    assert!(unknown.contains("Model `missing` was not found"));
    assert!(unknown.contains("Available model keys: `kimi`"));
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
        thread_id: thread_id.map(str::to_string),
        root_id: None,
        is_group,
        create_time: None,
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
fn mapping_key_without_reply_in_thread_unchanged() {
    // Quote-reply in a group with reply_in_thread off: root_id is ignored.
    let mut msg = channel_message(None, true, true);
    msg.root_id = Some("msg-root".to_string());
    assert_eq!(session_mapping_key(&msg, "chat-1", false), "chat-1");

    // Thread messages still key by thread_id as before.
    let msg = channel_message(Some("thread-1"), true, true);
    assert_eq!(session_mapping_key(&msg, "chat-1", false), "thread-1");
}

#[test]
fn chat_level_message_only_for_top_level_group_in_thread_mode() {
    // Top-level group message in reply_in_thread mode → chat-level.
    let msg = channel_message(None, true, true);
    assert!(is_chat_level_message(&msg, true));

    // In-thread message → thread session.
    let mut msg = channel_message(Some("thread-1"), true, true);
    assert!(!is_chat_level_message(&msg, true));
    // Quote-reply (root_id set) → thread session of the quoted root.
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
    };
    let models = vec![model_info("kimi", "kimi-k2", 256_000)];

    let out = format_session_info(&session, "kimi", &models, 0, &[]);
    assert!(out.contains(&format!("- ID: `{}`", session.id.0)));
    assert!(out.contains("- Model: `kimi` · anthropic · `kimi-k2` · 256k ctx (default)"));
    assert!(out.contains("- Status: idle"));
    assert!(out.contains("- Created: 3h ago · Active: 5m ago"));
    assert!(out.contains("- Permission: dangerous"));
    assert!(out.contains("- Subagents (running): 0"));
    assert!(out.contains("- Background Shell: none"));

    // Persisted model key drops the (default) marker; shells are listed.
    let session = crate::types::SessionResponse {
        model_key: Some("kimi".to_string()),
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
    let out = format_session_info(&session, "kimi", &models, 2, &shells);
    assert!(out.contains("- Model: `kimi` · anthropic · `kimi-k2` · 256k ctx\n"));
    assert!(out.contains("- Subagents (running): 2"));
    assert!(out.contains("- Background Shell: `cargo test` (pid 42, 9m ago)"));
}

// ── deliver_reply ───────────────────────────────────────────────────

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
        &sid,
        SettleKind::Stopped(&completed()),
        // Attachment delivery is out of scope for these tests; a dangling
        // Weak keeps the workspace lookup inert.
        &std::sync::Weak::new(),
    )
    .await;

    // The old card freezes as a terminal receipt …
    let patches = mock.patches.lock().await;
    assert_eq!(patches.len(), 1);
    assert!(patches[0].1.contains("✅ Done"), "frozen terminal card");
    // … and the reply lands at the bottom as a NEW bare-text message —
    // no trace panel (the trace was already live on the card mid-run).
    let cards = mock.cards.lock().await;
    assert_eq!(cards.len(), 1, "materialize only; reply is bare text");
    let outgoing = mock.outgoing.lock().await;
    assert_eq!(outgoing.len(), 1);
    assert!(outgoing[0].contains("final answer"));
    assert!(!outgoing[0].contains("Trace ·"));
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
    assert!(text.contains("Trace · 2 steps · 1 tools"));
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
        },
        HistoryMessage {
            message_id: "m2".into(),
            create_time: 1_700_000_060,
            sender_id: "ou_bob".into(),
            text: "x".repeat(2500),
        },
    ];
    let refs: Vec<&HistoryMessage> = messages.iter().collect();
    let out = assemble_history(&refs);
    assert!(out.starts_with("<recent_chat_history>\n"));
    assert!(out.ends_with("</recent_chat_history>"));
    assert!(out.contains("ou_alice: hello world"), "trimmed: {out}");
    let bob_line = out.lines().find(|l| l.contains("ou_bob")).unwrap();
    assert!(bob_line.ends_with('…'), "capped: {bob_line}");
    assert!(bob_line.chars().count() <= 2000 + 40, "line: {bob_line}");
}

#[derive(Default)]
struct HistoryMockAdapter {
    calls: tokio::sync::Mutex<Vec<(Option<i64>, usize)>>,
    fail: std::sync::atomic::AtomicBool,
    empty: std::sync::atomic::AtomicBool,
    with_root: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl PlatformAdapter for HistoryMockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelMessage>,
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
            });
        }
        messages.extend([
            HistoryMessage {
                message_id: "m0".into(),
                create_time: 100,
                sender_id: "ou_a".into(),
                text: "earlier".into(),
            },
            HistoryMessage {
                message_id: "m1".into(),
                create_time: 200,
                sender_id: "ou_a".into(),
                text: "latest".into(),
            },
            HistoryMessage {
                message_id: "trigger".into(),
                create_time: 300,
                sender_id: "ou_b".into(),
                text: "trigger msg".into(),
            },
        ]);
        Ok(messages)
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
        thread_id,
        root_id: None,
        is_group: true,
        create_time: None,
    }
}

#[tokio::test]
async fn history_prefix_assembles_drops_trigger_and_advances_cursor() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    let msg = group_msg(None);

    let prefix = maybe_history_prefix(&adapter, &config, &store, "feishu", &msg).await;
    let prefix = prefix.expect("history prefix");
    assert!(prefix.contains("earlier"));
    assert!(prefix.contains("latest"));
    assert!(!prefix.contains("trigger msg"), "trigger dropped: {prefix}");

    // Cursor advanced to the newest fetched message (the trigger's ts).
    let cursor = store.get_history_cursor("feishu", "oc_1").await.unwrap();
    assert_eq!(cursor, Some(300));

    // Second call passes the stored cursor through to the adapter.
    let _ = maybe_history_prefix(&adapter, &config, &store, "feishu", &msg).await;
    let calls = mock.calls.lock().await;
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], (None, 20));
    assert_eq!(calls[1], (Some(300), 20));
}

#[tokio::test]
async fn history_prefix_skips_private_chats() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(HistoryMockAdapter::default());
    let config = ChannelConfig::default();
    let mut msg = group_msg(None);
    msg.is_group = false;

    let prefix = maybe_history_prefix(&adapter, &config, &store, "feishu", &msg).await;
    assert!(prefix.is_none());
}

#[tokio::test]
async fn history_prefix_uses_thread_container_when_present() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let adapter: Arc<dyn PlatformAdapter> = Arc::new(HistoryMockAdapter::default());
    let config = ChannelConfig::default();
    let msg = group_msg(Some("omt_1".into()));

    let _ = maybe_history_prefix(&adapter, &config, &store, "feishu", &msg).await;
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

    let prefix = maybe_history_prefix(&adapter, &config, &store, "feishu", &group_msg(None)).await;
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

    let prefix = maybe_history_prefix(&adapter, &config, &store, "feishu", &group_msg(None)).await;
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

    let prefix = maybe_history_prefix(&adapter, &config, &store, "feishu", &group_msg(None)).await;
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
    let prefix = maybe_history_prefix(&adapter, &config, &store, "feishu", &group_msg(None)).await;
    assert!(prefix.is_none());
    assert!(mock.calls.lock().await.is_empty(), "no fetch issued");

    // Inside an existing thread, that thread's history still applies.
    let prefix = maybe_history_prefix(
        &adapter,
        &config,
        &store,
        "feishu",
        &group_msg(Some("omt_1".into())),
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
async fn history_prefix_drops_consumed_thread_root_only() {
    let (_pool, store) = create_test_pool().await;
    let store: Arc<dyn ChannelStore> = store;
    let mock = Arc::new(HistoryMockAdapter::default());
    mock.with_root
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = ChannelConfig::default();
    let mut msg = group_msg(Some("omt_1".into()));
    msg.root_id = Some("root-msg".into());

    // Bot-created thread: the root was consumed at channel level (chat
    // cursor covers it) → dropped from the thread history.
    store
        .set_history_cursor("feishu", "oc_1", 100)
        .await
        .unwrap();
    let prefix = maybe_history_prefix(&adapter, &config, &store, "feishu", &msg)
        .await
        .expect("history");
    assert!(
        !prefix.contains("thread root"),
        "consumed root dropped: {prefix}"
    );
    assert!(prefix.contains("earlier"), "other entries kept: {prefix}");

    // Human thread: chat cursor does NOT cover the root → kept.
    let (_pool2, store2) = create_test_pool().await;
    let store2: Arc<dyn ChannelStore> = store2;
    store2
        .set_history_cursor("feishu", "oc_1", 10)
        .await
        .unwrap();
    let prefix = maybe_history_prefix(&adapter, &config, &store2, "feishu", &msg)
        .await
        .expect("history");
    assert!(
        prefix.contains("thread root"),
        "unconsumed root kept: {prefix}"
    );
}
