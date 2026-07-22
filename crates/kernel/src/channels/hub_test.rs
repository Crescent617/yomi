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
    ) -> std::result::Result<(), crate::channels::ChannelError> {
        self.outgoing
            .lock()
            .await
            .push((external_chat_id.to_string(), blocks));
        Ok(())
    }
}

async fn create_test_pool() -> (sqlx::SqlitePool, Arc<SqliteChannelStore>) {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .unwrap();
    sqlx::query(
        r"CREATE TABLE channel_session_mappings (
                channel_name TEXT NOT NULL,
                external_chat_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                actual_chat_id TEXT NOT NULL,
                reply_msg_id TEXT,
                created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
                PRIMARY KEY (channel_name, external_chat_id)
            );
            CREATE INDEX idx_channel_mapping_session ON channel_session_mappings(session_id);",
    )
    .execute(&pool)
    .await
    .unwrap();
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
fn chat_wide_model_command_only_for_top_level_group_in_thread_mode() {
    // Top-level group message in reply_in_thread mode → chat-wide switch.
    let msg = channel_message(None, true, true);
    assert!(is_chat_wide_model_command(&msg, true));

    // In-thread message → per-thread switch.
    let mut msg = channel_message(Some("thread-1"), true, true);
    assert!(!is_chat_wide_model_command(&msg, true));
    // Quote-reply (root_id set) → per-session switch.
    msg.root_id = Some("msg-root".to_string());
    assert!(!is_chat_wide_model_command(&msg, true));

    // Private chat → never chat-wide.
    let msg = channel_message(None, false, true);
    assert!(!is_chat_wide_model_command(&msg, true));

    // reply_in_thread off → never chat-wide.
    let msg = channel_message(None, true, true);
    assert!(!is_chat_wide_model_command(&msg, false));
}
