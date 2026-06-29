use crate::app::coordinator::{Coordinator, CreateSessionInput};
use crate::event::{Event, ModelEvent};
use crate::types::{ContentBlock, Result, SessionId};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::{
    ChannelConfig, ChannelInfo, ChannelMessage, ChannelStatus, ChannelStore, PlatformAdapter,
};

const STATUS_IDLE: u8 = 0;
const STATUS_CONNECTING: u8 = 1;
const STATUS_ERROR: u8 = 3;

/// A running channel instance.
struct ChannelInstance {
    config: ChannelConfig,
    status: Arc<AtomicU8>,
    adapter: Arc<dyn PlatformAdapter>,
    handle: tokio::task::JoinHandle<()>,
}

/// Manages the lifecycle of all platform channels and routes incoming
/// messages to the coordinator.
pub struct ChannelHub {
    store: Arc<dyn ChannelStore>,
    cancel: CancellationToken,
    instances: DashMap<String, ChannelInstance>,
    /// Track which sessions already have an event subscriber running.
    /// Stores `AbortHandle` so we can detect finished tasks and clean them up.
    active_subscribers: Arc<DashMap<SessionId, tokio::task::AbortHandle>>,
}

impl ChannelHub {
    pub fn new(store: Arc<dyn ChannelStore>, cancel: CancellationToken) -> Self {
        Self {
            store,
            cancel,
            instances: DashMap::new(),
            active_subscribers: Arc::new(DashMap::new()),
        }
    }

    /// Start all enabled channels from the given configurations.
    /// If a channel with the same name already exists, it is skipped.
    pub async fn start_all(
        &self,
        configs: Vec<ChannelConfig>,
        coordinator: std::sync::Weak<Coordinator>,
    ) -> Result<()> {
        let mut errors = Vec::new();
        for config in configs {
            if !config.enabled {
                info!(channel = %config.name, "skipping disabled channel");
                continue;
            }
            if self.instances.contains_key(&config.name) {
                warn!(channel = %config.name, "channel already running, skipping");
                continue;
            }
            if let Err(e) = self.start_instance(config, coordinator.clone()).await {
                error!(error = %e, "failed to start channel");
                errors.push(e);
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(crate::types::KernelError::storage(format!(
                "{} channels failed to start: {}",
                errors.len(),
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )))
        }
    }

    async fn start_instance(
        &self,
        config: ChannelConfig,
        coordinator: std::sync::Weak<Coordinator>,
    ) -> Result<()> {
        let name = config.name.clone();
        info!(channel = %name, "starting channel");

        let adapter = build_adapter(&config.platform, config.require_mention);
        let status = Arc::new(AtomicU8::new(STATUS_CONNECTING));

        let (incoming_tx, incoming_rx) = mpsc::channel::<ChannelMessage>(256);
        let sub_cancel = self.cancel.child_token();
        let store = Arc::clone(&self.store);
        let active_subs = Arc::clone(&self.active_subscribers);

        let adapter_clone = Arc::clone(&adapter);
        let cancel_clone = sub_cancel.clone();
        let name_recv = name.clone();

        // Spawn the adapter receiver
        let status_recv = Arc::clone(&status);
        let recv_handle = tokio::spawn(async move {
            match adapter_clone.run_receiver(incoming_tx, cancel_clone).await {
                Ok(()) => {
                    info!(channel = %name_recv, "receiver exited cleanly");
                    status_recv.store(STATUS_IDLE, Ordering::Relaxed);
                }
                Err(e) => {
                    error!(channel = %name_recv, error = %e, "receiver error");
                    status_recv.store(STATUS_ERROR, Ordering::Relaxed);
                }
            }
        });

        let name_proc = name.clone();
        let config_proc = config.clone();

        // Spawn the message processing loop
        let adapter_for_proc = Arc::clone(&adapter);
        let proc_handle = tokio::spawn(async move {
            let mut incoming_rx = incoming_rx;
            loop {
                tokio::select! {
                    biased;
                    () = sub_cancel.cancelled() => {
                        info!(channel = %name_proc, "processing loop cancelled");
                        break;
                    }
                    Some(msg) = incoming_rx.recv() => {
                        if let Err(e) = config_proc.check_access(&msg.external_chat_id, &msg.external_user_id) {
                            info!(channel = %name_proc, error = %e, "access denied");
                            continue;
                        }
                        if config_proc.require_mention && !msg.is_mention {
                            info!(channel = %name_proc, chat_id = %msg.external_chat_id, "ignoring non-mention message");
                            continue;
                        }
                        // Route to coordinator
                        let Some(coord) = coordinator.upgrade() else {
                            warn!("coordinator gone, stopping processing loop");
                            break;
                        };
                        if let Err(e) = handle_incoming_message(
                            &name_proc,
                            &config_proc,
                            &store,
                            &active_subs,
                            coord,
                            msg,
                            Arc::clone(&adapter_for_proc),
                        ).await {
                            error!(error = %e, "failed to handle incoming message");
                        }
                    }
                    else => {
                        info!(channel = %name_proc, "incoming channel closed, exiting");
                        break;
                    }
                }
            }
        });

        let name_done = name.clone();

        // Combine handles
        let handle = tokio::spawn(async move {
            let _ = recv_handle.await;
            let _ = proc_handle.await;
            info!(channel = %name_done, "channel instance fully shut down");
        });

        let instance = ChannelInstance {
            config,
            status: Arc::clone(&status),
            adapter,
            handle,
        };

        self.instances.insert(name, instance);
        Ok(())
    }

    /// Gracefully stop all channels and wait for tasks to exit.
    pub async fn shutdown(&self) -> Result<()> {
        info!("shutting down channel hub");
        self.cancel.cancel();

        // Abort all active subscriber tasks before waiting for channels.
        for entry in self.active_subscribers.iter() {
            entry.value().abort();
        }
        self.active_subscribers.clear();

        let mut handles = Vec::new();
        for mut entry in self.instances.iter_mut() {
            let instance = entry.value_mut();
            handles.push(std::mem::replace(
                &mut instance.handle,
                tokio::spawn(async {}),
            ));
        }

        for handle in handles {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(10), handle).await;
        }

        self.instances.clear();
        info!("channel hub shut down");
        Ok(())
    }

    /// List current channel states.
    pub fn list_channels(&self) -> Vec<ChannelInfo> {
        self.instances
            .iter()
            .map(|entry| {
                let instance = entry.value();
                ChannelInfo {
                    name: instance.config.name.clone(),
                    status: match instance.status.load(Ordering::Relaxed) {
                        STATUS_CONNECTING => ChannelStatus::Connecting,
                        STATUS_ERROR => ChannelStatus::Error,
                        _ => ChannelStatus::Idle,
                    },
                }
            })
            .collect()
    }

    /// Find the channel adapter for a session if it belongs to an active channel.
    pub async fn get_adapter_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<(String, Arc<dyn PlatformAdapter>)>> {
        let maybe = self.store.find_by_session_id(session_id).await?;
        if let Some((channel_name, chat_id)) = maybe {
            if let Some(instance) = self.instances.get(&channel_name) {
                return Ok(Some((chat_id, Arc::clone(&instance.adapter))));
            }
        }
        Ok(None)
    }
}

async fn handle_incoming_message(
    channel_name: &str,
    _config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    active_subs: &Arc<DashMap<SessionId, tokio::task::AbortHandle>>,
    coordinator: Arc<Coordinator>,
    msg: ChannelMessage,
    adapter: Arc<dyn PlatformAdapter>,
) -> Result<()> {
    let chat_id = msg.external_chat_id.clone();
    let reply_msg_id = msg.external_message_id.filter(|_| msg.thread_id.is_some());
    let mapping_key = msg.thread_id.clone().unwrap_or_else(|| chat_id.clone());

    // 1. Find existing mapping or create new session
    let session_id = if let Some(sid) = store.find_mapping(channel_name, &mapping_key).await? {
        if coordinator.get_session(&sid).is_none() {
            coordinator
                .restore_session(
                    &sid,
                    vec![crate::tools::ask_user::ASK_USER_TOOL_NAME.to_string()],
                )
                .await?;
        }
        sid
    } else {
        let sid = coordinator
            .create_session(CreateSessionInput {
                project_id: None,
                working_dir: None,
                auto_approve_level: crate::permissions::Level::Dangerous,
                tool_blocklist: vec![crate::tools::ask_user::ASK_USER_TOOL_NAME.to_string()],
            })
            .await?;
        store.save_mapping(channel_name, &mapping_key, &sid).await?;
        sid
    };

    // 2. Ensure subscriber is alive before sending the message so that replies
    //    are not lost in a race between the agent generating output and the
    //    subscriber being spawned.
    spawn_subscriber_if_needed(
        active_subs,
        session_id.clone(),
        coordinator.clone(),
        chat_id.clone(),
        reply_msg_id.clone(),
        Arc::clone(&adapter),
    )
    .await;

    // 3. Send the message blocks to the session
    coordinator.send_message(&session_id, msg.content).await?;

    Ok(())
}

async fn spawn_subscriber_if_needed(
    active_subs: &Arc<DashMap<SessionId, tokio::task::AbortHandle>>,
    session_id: SessionId,
    coordinator: Arc<Coordinator>,
    external_chat_id: String,
    reply_msg_id: Option<String>,
    adapter: Arc<dyn PlatformAdapter>,
) {
    // Clean up finished subscribers so dead tasks don't block respawning.
    active_subs.retain(|_, handle| !handle.is_finished());

    if active_subs.contains_key(&session_id) {
        return; // Already subscribed (or still running)
    }

    let session_id_for_spawn = session_id.clone();
    let handle = tokio::spawn(async move {
        let mut rx = match coordinator.subscribe_session_events(&session_id_for_spawn) {
            Some(r) => r,
            None => {
                error!(session_id = %session_id_for_spawn.0, "failed to subscribe to session events");
                return;
            }
        };

        let mut buffer = String::new();

        loop {
            match rx.recv().await {
                Ok(Event::Model(ModelEvent::Request { .. })) => {
                    buffer.clear();
                    let adapter = Arc::clone(&adapter);
                    let chat_id = external_chat_id.clone();
                    tokio::spawn(async move {
                        let _ = adapter.send_typing(&chat_id).await;
                    });
                }
                Ok(Event::Model(ModelEvent::Chunk {
                    content: crate::event::ContentChunk::Text(text),
                    ..
                })) => {
                    buffer.push_str(&text);
                }
                Ok(Event::Model(ModelEvent::Completed { .. })) => {
                    if buffer.is_empty() {
                        continue;
                    }
                    let blocks = vec![ContentBlock::Text {
                        text: buffer.clone(),
                    }];
                    if let Err(e) = adapter
                        .send_message(&external_chat_id, blocks, reply_msg_id.as_deref())
                        .await
                    {
                        error!(error = %e, "failed to send reply to platform");
                    }
                    buffer.clear();
                }
                Ok(Event::Model(ModelEvent::Error { error, .. })) => {
                    let text = format!("Error: {error}");
                    let blocks = vec![ContentBlock::Text { text }];
                    if let Err(e) = adapter
                        .send_message(&external_chat_id, blocks, reply_msg_id.as_deref())
                        .await
                    {
                        error!(error = %e, "failed to send error message to platform");
                    }
                    buffer.clear();
                }
                Ok(Event::System(crate::event::SystemEvent::Shutdown { .. })) => {
                    info!(session_id = %session_id_for_spawn.0, "session shutdown, stopping subscriber");
                    break;
                }
                Err(_) => {
                    info!(session_id = %session_id_for_spawn.0, "event channel closed, stopping subscriber");
                    break;
                }
                _ => {}
            }
        }
    });

    // Atomic insert: if another concurrent call already inserted, abort our task.
    match active_subs.entry(session_id) {
        dashmap::Entry::Occupied(_) => {
            handle.abort();
        }
        dashmap::Entry::Vacant(entry) => {
            entry.insert(handle.abort_handle());
        }
    }
}

fn build_adapter(
    platform: &super::PlatformConfig,
    require_mention: bool,
) -> Arc<dyn PlatformAdapter> {
    match platform {
        super::PlatformConfig::Telegram { token } => {
            Arc::new(super::telegram::TelegramAdapter::new(token.clone()))
        }
        super::PlatformConfig::Feishu { app_id, app_secret } => Arc::new(
            super::feishu::FeishuAdapter::new(app_id.clone(), app_secret.clone(), require_mention),
        ),
    }
}

// ── Mock adapter for testing ───────────────────────────────────────

#[cfg(test)]
pub struct MockAdapter {
    pub name: String,
    pub outgoing: tokio::sync::Mutex<Vec<(String, Vec<ContentBlock>)>>,
}

#[cfg(test)]
impl MockAdapter {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            outgoing: tokio::sync::Mutex::new(Vec::new()),
        }
    }
}

#[cfg(test)]
#[async_trait::async_trait]
impl PlatformAdapter for MockAdapter {
    async fn run_receiver(
        &self,
        _incoming: mpsc::Sender<ChannelMessage>,
        cancel: CancellationToken,
    ) -> std::result::Result<(), super::ChannelError> {
        cancel.cancelled().await;
        Ok(())
    }

    async fn send_message(
        &self,
        external_chat_id: &str,
        blocks: Vec<ContentBlock>,
        _reply_msg_id: Option<&str>,
    ) -> std::result::Result<(), super::ChannelError> {
        self.outgoing
            .lock()
            .await
            .push((external_chat_id.to_string(), blocks));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::store::SqliteChannelStore;
    use crate::channels::PlatformConfig;
    use sqlx::sqlite::SqlitePoolOptions;

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

    #[tokio::test]
    async fn test_start_and_shutdown() {
        let (_pool, store) = create_test_pool().await;
        let cancel = CancellationToken::new();
        let hub = ChannelHub::new(store, cancel.clone());

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
                auto_approve_level: crate::permissions::Level::Safe,
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
                auto_approve_level: crate::permissions::Level::Safe,
            },
        ];

        hub.start_all(configs, std::sync::Weak::new())
            .await
            .unwrap();

        let channels = hub.list_channels();
        assert_eq!(channels.len(), 2);

        hub.shutdown().await.unwrap();

        let channels = hub.list_channels();
        assert!(channels.is_empty());
    }

    #[tokio::test]
    async fn test_disabled_channel_skipped() {
        let (_pool, store) = create_test_pool().await;
        let cancel = CancellationToken::new();
        let ch = ChannelHub::new(store, cancel.clone());

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

        ch.start_all(configs, std::sync::Weak::new()).await.unwrap();

        let channels = ch.list_channels();
        assert_eq!(channels.len(), 1);
        assert_eq!(channels[0].name, "enabled");

        ch.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_skip_existing_channel() {
        let (_pool, store) = create_test_pool().await;
        let cancel = CancellationToken::new();
        let hub = ChannelHub::new(store, cancel.clone());

        let config = ChannelConfig {
            name: "only_once".to_string(),
            enabled: true,
            platform: PlatformConfig::Telegram {
                token: "fake".into(),
            },
            ..Default::default()
        };

        hub.start_all(vec![config.clone()], std::sync::Weak::new())
            .await
            .unwrap();
        assert_eq!(hub.list_channels().len(), 1);

        // Second attempt should be skipped
        hub.start_all(vec![config], std::sync::Weak::new())
            .await
            .unwrap();
        assert_eq!(hub.list_channels().len(), 1);

        hub.shutdown().await.unwrap();
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
        let hub = ChannelHub::new(store, cancel.clone());

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
        hub.start_all(configs, std::sync::Weak::new())
            .await
            .unwrap();
        assert_eq!(hub.list_channels().len(), 1);

        hub.shutdown().await.unwrap();
    }
}
