use crate::event::{Event, ModelEvent};
use crate::kernel::{CreateSessionInput, Kernel};
use crate::storage::SessionStore;
use crate::types::{ContentBlock, Result, SessionId};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::{
    ChannelConfig, ChannelInfo, ChannelMessage, ChannelStatus, ChannelStore, PlatformAdapter,
    SessionRouting,
};

const STATUS_IDLE: u8 = 0;
const STATUS_CONNECTING: u8 = 1;
const STATUS_ERROR: u8 = 3;

/// A running channel instance.
struct ChannelInstance {
    config: ChannelConfig,
    status: Arc<AtomicU8>,
    adapter: Arc<dyn PlatformAdapter>,
}

/// Manages the lifecycle of all platform channels and routes incoming
/// messages to the kernel.
pub struct ChannelHub {
    store: Arc<dyn ChannelStore>,
    instances: Arc<DashMap<String, ChannelInstance>>,
}

impl ChannelHub {
    pub fn new(store: Arc<dyn ChannelStore>) -> Self {
        Self {
            store,
            instances: Arc::new(DashMap::new()),
        }
    }

    /// Start all enabled channels from the given configurations.
    /// If a channel with the same name already exists, it is skipped.
    pub async fn start_all(
        &self,
        token: CancellationToken,
        configs: Vec<ChannelConfig>,
        kernel: std::sync::Weak<Kernel>,
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
            if let Err(e) = self
                .start_instance(config, token.child_token(), kernel.clone())
                .await
            {
                error!(error = %e, "failed to start channel");
                errors.push(e);
            }
        }

        // Start the global event forwarder if we have a kernel with an event bus.
        if let Some(coord) = kernel.upgrade() {
            if let Some(bus) = coord.event_bus() {
                self.start_event_forwarder(bus, token.child_token()).await;
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
        token: CancellationToken,
        kernel: std::sync::Weak<Kernel>,
    ) -> Result<()> {
        let name = config.name.clone();
        info!(channel = %name, "starting channel");

        let adapter = build_adapter(&config.platform, config.require_mention);
        let status = Arc::new(AtomicU8::new(STATUS_CONNECTING));

        let (incoming_tx, incoming_rx) = mpsc::channel::<ChannelMessage>(256);
        let sub_cancel = token.child_token();
        let store = Arc::clone(&self.store);

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

        let adapter_proc = Arc::clone(&adapter);
        let name_proc = name.clone();
        let config_proc = config.clone();

        // Spawn the message processing loop
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
                        // Route to kernel
                        let Some(coord) = kernel.upgrade() else {
                            warn!("kernel gone, stopping processing loop");
                            break;
                        };
                        match handle_incoming_message(
                            &name_proc,
                            &config_proc,
                            &store,
                            coord,
                            msg.clone(),
                        ).await {
                            Ok(Some(reply_text)) => {
                                let chat_id = msg.external_chat_id.clone();
                                let reply_msg_id = reply_anchor(&msg, config_proc.reply_in_thread);
                                let adapter = Arc::clone(&adapter_proc);
                                tokio::spawn(async move {
                                    if let Err(e) = adapter.send_message(
                                        &chat_id,
                                        vec![ContentBlock::Text { text: reply_text }],
                                        reply_msg_id.as_deref(),
                                    ).await {
                                        error!(error = %e, "failed to send command reply");
                                    }
                                });
                            }
                            Ok(None) => {}
                            Err(e) => {
                                error!(error = %e, "failed to handle incoming message");
                            }
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
        let _handle = tokio::spawn(async move {
            let _ = recv_handle.await;
            let _ = proc_handle.await;
            info!(channel = %name_done, "channel instance fully shut down");
        });

        let instance = ChannelInstance {
            config,
            status: Arc::clone(&status),
            adapter,
        };

        self.instances.insert(name, instance);
        Ok(())
    }

    /// Start a single background task that subscribes to the global event bus
    /// and forwards model/system events for all channel-backed sessions.
    async fn start_event_forwarder(
        &self,
        event_bus: Arc<crate::comms::EventBus>,
        token: CancellationToken,
    ) {
        let store = Arc::clone(&self.store);
        let instances = Arc::clone(&self.instances);

        tokio::spawn(async move {
            let mut rx = event_bus.subscribe_all();

            loop {
                tokio::select! {
                    biased;
                    () = token.cancelled() => break,
                    Some((session_id, envelope)) = rx.recv() => {
                        let routing = match store.find_routing_by_session(&session_id).await {
                            Ok(Some(r)) => r,
                            Ok(None) => continue,
                            Err(e) => {
                                error!(error = %e, "failed to look up routing for session");
                                continue;
                            }
                        };

                        let Some(instance) = instances.get(&routing.channel_name) else { continue };
                        let adapter = Arc::clone(&instance.adapter);

                        match envelope.event {
                            Event::Model(ModelEvent::Request { .. }) => {
                                let chat_id = routing.external_chat_id.clone();
                                tokio::spawn(async move {
                                    let _ = adapter.send_typing(&chat_id).await;
                                });
                            }
                            Event::Model(ModelEvent::End { content, .. }) => {
                                let text = super::blocks_to_text(&content);
                                if !text.is_empty() {
                                    Self::spawn_reply(adapter, routing, text);
                                }
                            }
                            Event::Model(ModelEvent::Error { error, .. }) => {
                                Self::spawn_reply(adapter, routing, format!("Error: {error}"));
                            }
                            _ => {}
                        }
                    }
                }
            }

            info!("channel event forwarder exited");
        });
    }

    fn spawn_reply(
        adapter: Arc<dyn PlatformAdapter>,
        routing: SessionRouting,
        text: impl Into<String>,
    ) {
        let chat_id = routing.external_chat_id;
        let reply_msg_id = routing.reply_msg_id;
        let blocks = vec![ContentBlock::Text { text: text.into() }];
        tokio::spawn(async move {
            if let Err(e) = adapter
                .send_message(&chat_id, blocks, reply_msg_id.as_deref())
                .await
            {
                error!(error = %e, "failed to send reply to platform");
            }
        });
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

    /// Get routing info and adapter for a session.
    pub async fn get_routing_for_session(
        &self,
        session_id: &SessionId,
    ) -> Result<Option<(SessionRouting, Arc<dyn PlatformAdapter>)>> {
        let routing = match self.store.find_routing_by_session(session_id).await? {
            Some(r) => r,
            None => return Ok(None),
        };

        let adapter = if let Some(instance) = self.instances.get(&routing.channel_name) {
            Arc::clone(&instance.adapter)
        } else {
            return Ok(None);
        };

        Ok(Some((routing, adapter)))
    }
}

async fn handle_incoming_message(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: Arc<Kernel>,
    msg: ChannelMessage,
) -> Result<Option<String>> {
    let chat_id = msg.external_chat_id.clone();
    let reply_msg_id = reply_anchor(&msg, config.reply_in_thread);
    let mapping_key = msg.thread_id.clone().unwrap_or_else(|| chat_id.clone());

    let cmd = parse_channel_command(msg.raw_text.as_deref());
    match cmd {
        ChannelCommand::Clear => {
            if let Some(sid) = store.find_mapping(channel_name, &mapping_key).await? {
                if let Err(e) = kernel.clear_session(&sid) {
                    tracing::warn!("Failed to clear session {}: {}", sid.0, e);
                }
            }
            Ok(Some("Context cleared.".to_string()))
        }
        ChannelCommand::Stop => {
            if let Some(sid) = store.find_mapping(channel_name, &mapping_key).await? {
                kernel.cancel(&sid);
                return Ok(Some("Stopped.".to_string()));
            }
            Ok(Some("No active session to stop.".to_string()))
        }
        ChannelCommand::Steer(text) => {
            let sid = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                &mapping_key,
                reply_msg_id.as_deref(),
            )
            .await?;
            kernel.send_steer(&sid, vec![ContentBlock::Text { text }]);
            Ok(None)
        }
        ChannelCommand::Queue(text) => {
            let sid = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                &mapping_key,
                reply_msg_id.as_deref(),
            )
            .await?;
            kernel
                .send_message(&sid, vec![ContentBlock::Text { text }])
                .await?;
            Ok(None)
        }
        ChannelCommand::ListModels => {
            let sid = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                &mapping_key,
                reply_msg_id.as_deref(),
            )
            .await?;
            let models = kernel.list_models().await?;
            let current = kernel.get_session_model(&sid).await;
            Ok(Some(format_model_list(&models, &current)))
        }
        ChannelCommand::CurrentModel => {
            let sid = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                &mapping_key,
                reply_msg_id.as_deref(),
            )
            .await?;
            let models = kernel.list_models().await?;
            let current = kernel.get_session_model(&sid).await;
            Ok(Some(format_current_model(&models, &current)))
        }
        ChannelCommand::SwitchModel(key) => {
            let sid = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                &mapping_key,
                reply_msg_id.as_deref(),
            )
            .await?;
            let models = kernel.list_models().await?;
            if !models.iter().any(|model| model.name == key) {
                return Ok(Some(format_unknown_model(&key, &models)));
            }
            kernel.set_session_model(&sid, &key).await?;
            Ok(Some(format!(
                "Switched to `{key}`. It takes effect on the next model invocation."
            )))
        }
        ChannelCommand::InvalidModelCommand => Ok(Some(
            "Usage: `/model` or `/model <model_key>`. Use `/models` to list models.".to_string(),
        )),
        ChannelCommand::None => {
            let sid = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                &mapping_key,
                reply_msg_id.as_deref(),
            )
            .await?;
            kernel.send_steer(&sid, msg.content);
            Ok(None)
        }
    }
}

/// Compute the message ID a reply should be anchored to.
///
/// Replies to in-thread messages always stay in that thread. When the
/// channel's `reply_in_thread` is enabled, group messages additionally anchor
/// to the triggering message so the reply opens/continues its thread
/// (Feishu thread reply, Telegram quote-reply). Private chats are never
/// anchored — threading there is just noise.
fn reply_anchor(msg: &ChannelMessage, reply_in_thread: bool) -> Option<String> {
    msg.external_message_id
        .clone()
        .filter(|_| msg.thread_id.is_some() || (reply_in_thread && msg.is_group))
}

/// Get an existing session or create a new one, updating routing info.
async fn get_or_create_session(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    chat_id: &str,
    mapping_key: &str,
    reply_msg_id: Option<&str>,
) -> Result<SessionId> {
    if let Some(sid) = store.find_mapping(channel_name, mapping_key).await? {
        store
            .save_mapping(channel_name, mapping_key, &sid, chat_id, reply_msg_id)
            .await?;
        return Ok(sid);
    }

    let model_key = model_key_for_new_channel_session(
        channel_name,
        chat_id,
        mapping_key,
        store,
        &kernel.session_store().await,
    )
    .await?;
    let sid = kernel
        .create_session(CreateSessionInput {
            project_id: None,
            working_dir: None,
            auto_approve_level: crate::permission::Level::Dangerous,
            tool_blocklist: vec![crate::tools::ask_user::ASK_USER_TOOL_NAME.to_string()],
            model_key,
        })
        .await?;
    store
        .save_mapping(channel_name, mapping_key, &sid, chat_id, reply_msg_id)
        .await?;
    Ok(sid)
}

/// Resolve the persisted model key for a newly-created channel session.
/// Thread sessions inherit an explicit model choice from their parent chat
/// session. Missing mappings, sessions, or model keys intentionally yield
/// `None`, allowing runtime model resolution to use the configured default
/// without persisting it.
async fn model_key_for_new_channel_session(
    channel_name: &str,
    chat_id: &str,
    mapping_key: &str,
    channel_store: &Arc<dyn ChannelStore>,
    session_store: &Arc<dyn SessionStore>,
) -> Result<Option<String>> {
    if mapping_key == chat_id {
        return Ok(None);
    }

    let Some(parent_session_id) = channel_store.find_mapping(channel_name, chat_id).await? else {
        return Ok(None);
    };

    Ok(session_store
        .get(&parent_session_id)
        .await?
        .and_then(|session| session.model_key))
}

/// Parsed channel command from an incoming message.
enum ChannelCommand {
    /// Clear context and start fresh.
    Clear,
    /// Stop current streaming.
    Stop,
    /// Inject a steer message before the next turn.
    Steer(String),
    /// Queue a normal user message for a later turn.
    Queue(String),
    /// List configured models and mark the current one.
    ListModels,
    /// Show the current session model.
    CurrentModel,
    /// Switch this session to the model identified by its config key.
    SwitchModel(String),
    /// A model command with too many arguments.
    InvalidModelCommand,
    /// Not a command.
    None,
}

fn parse_channel_command(raw_text: Option<&str>) -> ChannelCommand {
    let Some(text) = raw_text.map(str::trim).filter(|text| !text.is_empty()) else {
        return ChannelCommand::None;
    };
    let mut parts = text.split_whitespace();
    let Some(cmd) = parts.next() else {
        return ChannelCommand::None;
    };

    let command = if cmd.starts_with("/models") {
        "/models"
    } else if cmd.starts_with("/model") {
        "/model"
    } else if cmd.starts_with("/clear") {
        "/clear"
    } else if cmd.starts_with("/stop") {
        "/stop"
    } else if cmd.starts_with("/steer") {
        "/steer"
    } else if cmd.starts_with("/queue") {
        "/queue"
    } else {
        return ChannelCommand::None;
    };

    match command {
        "/clear" if parts.next().is_none() => ChannelCommand::Clear,
        "/stop" if parts.next().is_none() => ChannelCommand::Stop,
        "/steer" | "/queue" => {
            let rest = parts.collect::<Vec<_>>().join(" ");
            if rest.is_empty() {
                ChannelCommand::None
            } else if command == "/queue" {
                ChannelCommand::Queue(rest)
            } else {
                ChannelCommand::Steer(rest)
            }
        }
        "/models" | "/model" => match (parts.next(), parts.next()) {
            (None, None) if command == "/models" => ChannelCommand::ListModels,
            (None, None) => ChannelCommand::CurrentModel,
            (Some(key), None) => ChannelCommand::SwitchModel(key.to_string()),
            _ => ChannelCommand::InvalidModelCommand,
        },
        _ => ChannelCommand::None,
    }
}

pub(super) fn has_channel_command_prefix(raw_text: &str) -> bool {
    let command = raw_text.split_whitespace().next().unwrap_or_default();
    ["/models", "/model", "/clear", "/stop", "/steer", "/queue"]
        .iter()
        .any(|prefix| command.starts_with(prefix))
}

fn format_model_list(models: &[crate::kernel::ModelInfo], current: &str) -> String {
    if models.is_empty() {
        return "No models are currently available.".to_string();
    }

    let mut lines = vec!["**Available models**".to_string(), String::new()];
    for model in models {
        let marker = if model.name == current {
            " **← current**"
        } else {
            ""
        };
        lines.push(format!(
            "- `{}` · {} · `{}` · {}k ctx{}",
            model.name,
            model.provider,
            model.model_id,
            model.context_window / 1000,
            marker
        ));
    }
    lines.push(String::new());
    lines.push("Switch with `/model <model_key>`.".to_string());
    lines.join("\n")
}

fn format_current_model(models: &[crate::kernel::ModelInfo], current: &str) -> String {
    models
        .iter()
        .find(|model| model.name == current)
        .map_or_else(
            || format!("Current model: `{current}`. Use `/models` to list available models."),
            |model| {
                format!(
                "Current model: `{}` · {} · `{}` · {}k ctx\n\nSwitch with `/model <model_key>`.",
                model.name,
                model.provider,
                model.model_id,
                model.context_window / 1000
            )
            },
        )
}

fn format_unknown_model(key: &str, models: &[crate::kernel::ModelInfo]) -> String {
    let keys = models
        .iter()
        .map(|model| format!("`{}`", model.name))
        .collect::<Vec<_>>()
        .join(", ");
    if keys.is_empty() {
        format!("Model `{key}` was not found. No models are currently available.")
    } else {
        format!(
            "Model `{key}` was not found.\n\nAvailable model keys: {keys}\n\nUse `/models` for details."
        )
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

#[cfg(test)]
#[path = "hub_test.rs"]
mod tests;
