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
    pub reactions: tokio::sync::Mutex<Vec<(String, String)>>,
    pub quoted: tokio::sync::Mutex<Option<crate::channels::HistoryMessage>>,
    /// Per-id quoted responses (quote chains); consulted before `quoted`.
    pub quoted_map:
        tokio::sync::Mutex<std::collections::HashMap<String, crate::channels::HistoryMessage>>,
    pub quoted_calls: tokio::sync::Mutex<Vec<String>>,
    /// When false (default), image downloads fail — mirroring the trait's
    /// default unsupported behavior so degradation paths stay testable.
    pub image_download_ok: tokio::sync::Mutex<bool>,
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
        }
    }
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
        Ok(None)
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
            mid_run_split: true,
            history_context: 0,
            approval_chat_id: None,
            admin_users: vec![],
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
    // Prefix lookalikes are not commands.
    assert!(matches!(
        parse_channel_command(Some("/compaction")),
        ChannelCommand::None
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
    // Text is required — a bare command is a usage error, and prefix
    // lookalikes are not commands.
    assert!(matches!(
        parse_channel_command(Some("/thread")),
        ChannelCommand::InvalidThreadCommand
    ));
    assert!(matches!(
        parse_channel_command(Some("/threads hi")),
        ChannelCommand::None
    ));
    assert!(HELP_TEXT.contains("/thread"));
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
        Some("permission denied：你不在 admin_users 中。")
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

    // A follow-up inside the opened thread adopts the root-keyed
    // session — no fresh thread-id-keyed session appears.
    let mut follow_up = base("m2", "继续");
    follow_up.thread_id = Some("omt_1".to_string());
    follow_up.root_id = Some("m1".to_string());
    follow_up.parent_id = Some("om_bot".to_string());
    let reply = handle(follow_up).await.unwrap();
    assert_eq!(reply, None);
    assert!(
        store.find_mapping("mock", "omt_1").await.unwrap().is_none(),
        "adoption must not spawn a thread-keyed session"
    );
    assert_eq!(
        store.find_mapping("mock", "m1").await.unwrap(),
        Some(sid),
        "the follow-up kept the /thread session"
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
    assert!(store.find_mapping("mock", "omt_1").await.unwrap().is_none());
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
    assert!(current.contains("Current model: `nova`"));
    assert!(current.contains("`nova-2`"));

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
    // Prefix lookalikes are not commands.
    assert!(matches!(
        parse_channel_command(Some("/approved 3")),
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
    let models = vec![model_info("nova", "nova-2", 256_000)];

    let out = format_session_info(&session, "nova", &models, 0, &[]);
    assert!(out.contains(&format!("- ID: `{}`", session.id.0)));
    assert!(out.contains("- Model: `nova` · anthropic · `nova-2` · 256k ctx (default)"));
    assert!(out.contains("- Status: idle"));
    assert!(out.contains("- Created: 3h ago · Active: 5m ago"));
    assert!(out.contains("- Permission: dangerous"));
    assert!(out.contains("- Subagents (running): 0"));
    assert!(out.contains("- Background Shell: none"));

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
    let out = format_session_info(&session, "nova", &models, 2, &shells);
    assert!(out.contains("- Model: `nova` · anthropic · `nova-2` · 256k ctx\n"));
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
    assert!(patches[0].1.contains("✅ Done"), "frozen terminal card");
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
        cards[1].1.contains("collapsible_panel") && cards[1].1.contains("Trace ·"),
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
    assert!(patches[0].1.contains("✅ Done"), "frozen terminal card");
    assert!(
        patches[0].1.contains("collapsible_panel") && patches[0].1.contains("Trace ·"),
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
    assert!(patches[0].1.contains("✅ Done"));
    assert!(
        patches[0].1.contains("collapsible_panel"),
        "trace stays on the card: {patches:?}"
    );
    drop(patches);
    let outgoing = mock.outgoing.lock().await;
    assert_eq!(outgoing.len(), 1, "bare-text flush");
    assert!(outgoing[0].contains("final answer"));
    assert!(!outgoing[0].contains("Trace ·"));
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
    assert!(patches[0].1.contains("✅ Done"));
    assert!(
        patches[0].1.contains("collapsible_panel") && patches[0].1.contains("Trace ·"),
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
            HistoryMessage {
                message_id: "trigger".into(),
                create_time: 300,
                sender_id: "ou_b".into(),
                text: "trigger msg".into(),
                image_keys: vec![],
                parent_id: None,
            },
        ]);
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
    assert!(reply.is_some());
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

#[tokio::test]
async fn gate_accepts_allowed_mention_with_ack_reaction() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let msg = channel_message(None, true, true);

    assert_eq!(
        gate_message(&adapter, &feishu_gate_config(), &msg).await,
        Gate::Allow
    );
    let reactions = mock.reactions.lock().await.clone();
    assert_eq!(reactions, [("msg-1".to_string(), "OneSecond".to_string())]);
}

#[tokio::test]
async fn gate_denies_unlisted_user_with_denied_reaction() {
    let mock = Arc::new(MockAdapter::new("fs"));
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut msg = channel_message(None, true, true);
    msg.external_user_id = "stranger".to_string();

    assert_eq!(
        gate_message(&adapter, &feishu_gate_config(), &msg).await,
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
        gate_message(&adapter, &feishu_gate_config(), &msg).await,
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
        gate_message(&adapter, &feishu_gate_config(), &msg).await,
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

    assert_eq!(gate_message(&adapter, &config, &msg).await, Gate::Denied);
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

    assert_eq!(gate_message(&adapter, &config, &msg).await, Gate::Denied);
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
    assert_eq!(gate_message(&adapter, &config, &msg).await, Gate::Allow);

    let mut denied = channel_message(None, true, true);
    denied.external_user_id = "stranger".to_string();
    assert_eq!(gate_message(&adapter, &config, &denied).await, Gate::Denied);

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

    assert_eq!(gate_message(&adapter, &config, &msg).await, Gate::Allow);
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
        gate_message(&adapter, &feishu_gate_config(), &msg).await,
        Gate::Denied
    );
    assert!(mock.reactions.lock().await.is_empty());
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
