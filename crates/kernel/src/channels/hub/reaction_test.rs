use super::*;
use crate::channels::{store::SqliteChannelStore, ChannelConfig};
use crate::channels::{ChannelError, ChannelEvent, PlatformConfig};
use crate::storage::migrations::run_migrations;
use sqlx::sqlite::SqlitePoolOptions;
use std::sync::Mutex;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct MockAdapter {
    fetch_calls: Mutex<Vec<String>>,
    target: Mutex<Option<ReactionTarget>>,
    fail_fetch: bool,
}

#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelEvent>,
        _cancel: CancellationToken,
    ) -> Result<(), ChannelError> {
        std::future::pending().await
    }

    async fn send_message(
        &self,
        _external_chat_id: &str,
        _blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> {
        Ok(None)
    }

    async fn fetch_reaction_target(
        &self,
        message_id: &str,
    ) -> Result<Option<ReactionTarget>, ChannelError> {
        self.fetch_calls
            .lock()
            .unwrap()
            .push(message_id.to_string());
        if self.fail_fetch {
            return Err(ChannelError::Platform("boom".into()));
        }
        Ok(self.target.lock().unwrap().clone())
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

fn feishu_config() -> ChannelConfig {
    ChannelConfig {
        name: "mock".to_string(),
        enabled: true,
        platform: PlatformConfig::Feishu {
            app_id: "a".to_string(),
            app_secret: "s".to_string(),
        },
        ..Default::default()
    }
}

fn notice() -> ReactionNotice {
    ReactionNotice {
        message_id: "om_target".to_string(),
        emoji_type: "THUMBSUP".to_string(),
        operator_open_id: "ou_user".to_string(),
        action_time: Some(1_760_000_000_000),
    }
}

fn target() -> ReactionTarget {
    ReactionTarget {
        chat_id: "oc_chat".to_string(),
        thread_id: None,
        root_id: None,
        excerpt: "some answer".to_string(),
    }
}

#[test]
fn candidate_keys_most_specific_first() {
    let t = ReactionTarget {
        chat_id: "oc_c".to_string(),
        thread_id: Some("omt_1".to_string()),
        root_id: Some("om_root".to_string()),
        excerpt: String::new(),
    };
    let keys = candidate_keys(&notice(), &t);
    assert_eq!(keys, ["om_root", "omt_1", "om_target", "oc_c"]);
}

#[test]
fn candidate_keys_dedups_repeated_ids() {
    // The reacted message IS the thread root: root_id == message_id.
    let t = ReactionTarget {
        chat_id: "oc_c".to_string(),
        thread_id: Some("omt_1".to_string()),
        root_id: Some("om_target".to_string()),
        excerpt: String::new(),
    };
    let keys = candidate_keys(&notice(), &t);
    assert_eq!(keys, ["om_target", "omt_1", "oc_c"]);
}

#[test]
fn steer_text_carries_header_emoji_and_excerpt() {
    let text = steer_text(&notice(), &target(), Some("白 水"), "feishu");
    let (header, body) = text.split_once('\n').unwrap();
    assert!(header.contains("[from: 白 水 (ou_user)]"));
    assert!(header.contains("[chat_id: oc_chat]"));
    // The header anchors the REACTED message, not a new user post.
    assert!(header.contains("[msg_id: om_target]"));
    assert!(header.contains("[platform: feishu]"));
    assert_eq!(body, "[reaction: THUMBSUP] some answer");
}

#[test]
fn steer_text_in_thread_carries_thread_and_root_segments() {
    let t = ReactionTarget {
        thread_id: Some("omt_1".to_string()),
        root_id: Some("om_root".to_string()),
        ..target()
    };
    let text = steer_text(&notice(), &t, None, "feishu");
    let (header, _) = text.split_once('\n').unwrap();
    assert!(header.contains("[thread: omt_1]"));
    assert!(header.contains("[root: om_root]"));
    // …且顺序固定：msg_id → thread → root → platform。
    assert!(header
        .find("[msg_id: om_target]")
        .zip(header.find("[thread: omt_1]"))
        .zip(header.find("[root: om_root]"))
        .is_some_and(|((m, t), r)| m < t && t < r));
}

#[test]
fn steer_text_without_excerpt_or_name() {
    let t = ReactionTarget {
        excerpt: String::new(),
        ..target()
    };
    let text = steer_text(&notice(), &t, None, "feishu");
    let (header, body) = text.split_once('\n').unwrap();
    assert!(header.contains("[from_user_id: ou_user]"));
    assert_eq!(body, "[reaction: THUMBSUP]");
}

#[tokio::test]
async fn disabled_feature_skips_target_fetch() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter {
        target: Mutex::new(Some(target())),
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut config = feishu_config();
    config.disabled_events = vec!["reaction".to_string()];
    let kernel = Weak::new();
    handle_message_reaction("mock", &config, &store, &kernel, &adapter, notice()).await;
    assert!(mock.fetch_calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn gone_target_drops_quietly() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter::default()); // target: None
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = feishu_config();
    let kernel = Weak::new();
    handle_message_reaction("mock", &config, &store, &kernel, &adapter, notice()).await;
    assert_eq!(mock.fetch_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn fetch_failure_drops_quietly() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter {
        fail_fetch: true,
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = feishu_config();
    let kernel = Weak::new();
    handle_message_reaction("mock", &config, &store, &kernel, &adapter, notice()).await;
    assert_eq!(mock.fetch_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn blocked_operator_drops() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter {
        target: Mutex::new(Some(target())),
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let mut config = feishu_config();
    config.blocked_users = vec!["ou_user".to_string()];
    let kernel = Weak::new();
    handle_message_reaction("mock", &config, &store, &kernel, &adapter, notice()).await;
    // The target fetch happened (the access check needs its chat id);
    // the denial drops the event before any session work.
    assert_eq!(mock.fetch_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn no_mapping_drops() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter {
        target: Mutex::new(Some(target())),
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = feishu_config();
    let kernel = Weak::new();
    handle_message_reaction("mock", &config, &store, &kernel, &adapter, notice()).await;
    assert_eq!(mock.fetch_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn idle_session_drops_without_steer() {
    let store = test_store().await;
    let mock = Arc::new(MockAdapter {
        target: Mutex::new(Some(target())),
        ..Default::default()
    });
    let adapter: Arc<dyn PlatformAdapter> = mock.clone();
    let config = feishu_config();

    // A real (never-started) kernel with a real session: the mapping
    // resolves but `is_session_running` is false — the reaction drops.
    let tmp = tempfile::TempDir::new().unwrap();
    let mut kconfig = crate::config::Config {
        data_dir: tmp.path().to_path_buf(),
        ..crate::config::Config::default()
    };
    kconfig.finalize();
    let kernel = crate::build_kernel(&kconfig, false).await.unwrap();
    let sid = kernel
        .create_session(crate::kernel::CreateSessionInput {
            project_id: None,
            working_dir: None,
            auto_approve_level: None,
            tool_blocklist: vec![],
            model_key: None,
            context_window: None,
        })
        .await
        .unwrap();
    store
        .save_mapping(
            "mock",
            "oc_chat",
            &sid,
            "oc_chat",
            None,
            crate::channels::MappingKind::Normal,
        )
        .await
        .unwrap();

    let kernel_weak = Arc::downgrade(&kernel);
    handle_message_reaction("mock", &config, &store, &kernel_weak, &adapter, notice()).await;
    let snapshot = kernel.mailbox_snapshot(&sid).await;
    assert!(snapshot.steer.is_empty() && snapshot.queue.is_empty());
}

/// The first (most specific) mapping hit wins even when a less specific
/// key also maps — feedback addressed at a thread must not leak into
/// the chat-level session.
#[tokio::test]
async fn first_mapping_hit_wins() {
    let store = test_store().await;
    let thread_sid = SessionId::new();
    let chat_sid = SessionId::new();
    store
        .save_mapping(
            "mock",
            "om_root",
            &thread_sid,
            "oc_c",
            None,
            crate::channels::MappingKind::Normal,
        )
        .await
        .unwrap();
    store
        .save_mapping(
            "mock",
            "oc_c",
            &chat_sid,
            "oc_c",
            None,
            crate::channels::MappingKind::Normal,
        )
        .await
        .unwrap();
    let t = ReactionTarget {
        chat_id: "oc_c".to_string(),
        thread_id: Some("omt_1".to_string()),
        root_id: Some("om_root".to_string()),
        excerpt: String::new(),
    };
    let sid = resolve_target_session("mock", &store, &notice(), &t)
        .await
        .unwrap();
    assert_eq!(sid, thread_sid);
}
