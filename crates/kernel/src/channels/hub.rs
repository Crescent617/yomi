use crate::event::{AgentEvent, AgentStatus, Event, ModelEvent, ToolEvent};
use crate::kernel::{CreateSessionInput, Kernel};
use crate::storage::{format_age, SessionStore};
use crate::types::{ContentBlock, Result, SessionId};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::{
    obs::ObsTracker, reply, ChannelConfig, ChannelInfo, ChannelMessage, ChannelStatus,
    ChannelStore, PlatformAdapter, SessionRouting,
};

const STATUS_IDLE: u8 = 0;
const STATUS_CONNECTING: u8 = 1;
const STATUS_ERROR: u8 = 3;

/// Watchdog sweep interval for dead-session status cards.
const WATCHDOG_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

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
    obs: Arc<ObsTracker>,
}

impl ChannelHub {
    pub fn new(store: Arc<dyn ChannelStore>) -> Self {
        Self {
            store,
            instances: Arc::new(DashMap::new()),
            obs: Arc::new(ObsTracker::new()),
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
                self.start_event_forwarder(bus, token.child_token(), kernel.clone())
                    .await;
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
        let obs_proc = Arc::clone(&self.obs);

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
                            &obs_proc,
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
        kernel: std::sync::Weak<Kernel>,
    ) {
        let store = Arc::clone(&self.store);
        let instances = Arc::clone(&self.instances);
        let obs = Arc::clone(&self.obs);

        tokio::spawn(async move {
            // `ToolCallDelta` floods (e.g. thousands of argument deltas for a
            // large file write) would overflow this listener's 256-slot
            // buffer while the loop is blocked in an inline card PATCH,
            // silently dropping text `Chunk`/`End` events (bus delivery is
            // try_send). The forwarder never consumes deltas, so filter them
            // out at the source.
            let mut rx = event_bus.subscribe_all_filtered(|envelope| {
                !matches!(
                    envelope.event,
                    Event::Model(ModelEvent::ToolCallDelta { .. })
                )
            });
            let mut watchdog = tokio::time::interval(WATCHDOG_SWEEP_INTERVAL);
            watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            // Per-session run reply buffers: assistant texts and tool calls
            // accumulate during a run; only the last text becomes a message
            // bubble when the run ends (design: reply buffering). A buffer
            // is (re)started by the first `Running` of a run and drained at
            // `Stopped`/watchdog, so its presence doubles as the
            // run-in-flight marker.
            let mut reply_buffers: HashMap<SessionId, reply::RunReplyBuffer> = HashMap::new();

            loop {
                tokio::select! {
                    biased;
                    () = token.cancelled() => break,
                    _ = watchdog.tick() => {
                        // Kernel gone = shutting down; nothing to settle.
                        if let Some(kernel) = kernel.upgrade() {
                            // Sessions whose agent died (crash / lost
                            // `Stopped`): flush whatever reply state remains
                            // so content is never silently lost, mirroring the
                            // obs timeout settlement. Race note: a `Stopped`
                            // already queued in the bus can lose to this flush
                            // (the reply lands a beat early, from possibly
                            // not-yet-processed events); the real `Stopped`
                            // then finds no buffer and sends nothing.
                            let dead: Vec<SessionId> = reply_buffers
                                .keys()
                                .filter(|sid| !kernel.is_session_running(sid))
                                .cloned()
                                .collect();
                            for sid in dead {
                                // Look up BEFORE draining the buffer: a
                                // failed lookup must not drop the reply.
                                let routing = match store.find_routing_by_session(&sid).await {
                                    Ok(Some(r)) => r,
                                    _ => continue,
                                };
                                let Some((adapter, tool_trace, observability)) = instances
                                    .get(&routing.channel_name)
                                    .map(|i| {
                                        (
                                            Arc::clone(&i.adapter),
                                            i.config.tool_trace,
                                            i.config.observability,
                                        )
                                    })
                                else { continue };
                                let Some(buf) = reply_buffers.remove(&sid) else { continue };
                                deliver_reply(
                                    &obs,
                                    &adapter,
                                    &routing,
                                    Some(buf.into_reply()),
                                    tool_trace,
                                    observability,
                                    &sid,
                                    SettleKind::Timeout,
                                )
                                .await;
                            }
                            obs.sweep_dead_sessions(|sid| kernel.is_session_running(sid)).await;
                        }
                    }
                    Some((session_id, envelope)) = rx.recv() => {
                        let routing = match store.find_routing_by_session(&session_id).await {
                            Ok(Some(r)) => r,
                            Ok(None) => continue,
                            Err(e) => {
                                error!(error = %e, "failed to look up routing for session");
                                continue;
                            }
                        };

                        let (adapter, observability, tool_trace) = {
                            let Some(instance) = instances.get(&routing.channel_name) else { continue };
                            (
                                Arc::clone(&instance.adapter),
                                instance.config.observability,
                                instance.config.tool_trace,
                            )
                        };
                        let supports_cards = adapter.supports_status_card();

                        // Reply buffering: collect assistant texts and tool
                        // calls for the run instead of sending each
                        // intermediate text as its own bubble.
                        match &envelope.event {
                            Event::Agent(AgentEvent::Lifecycle {
                                state: AgentStatus::Running,
                            }) => {
                                // `Running` fires per turn; `or_default`
                                // keeps an existing buffer — buffers are
                                // drained at `Stopped`/watchdog. (Crash then
                                // quick restart within a watchdog interval
                                // may blend the old run's trace in;
                                // cosmetic, self-heals on the next run.)
                                reply_buffers.entry(session_id.clone()).or_default();
                            }
                            Event::Model(ModelEvent::End { content, .. }) => {
                                let text = super::blocks_to_text(content);
                                if !text.is_empty() {
                                    reply_buffers
                                        .entry(session_id.clone())
                                        .or_default()
                                        .record_text(text);
                                }
                            }
                            Event::Tool(ToolEvent::Start {
                                tool_id,
                                tool_name,
                                arguments,
                                ..
                            }) => {
                                reply_buffers
                                    .entry(session_id.clone())
                                    .or_default()
                                    .record_tool_start(tool_id, tool_name, arguments.as_deref());
                            }
                            Event::Tool(ToolEvent::End {
                                tool_id,
                                elapsed_ms,
                                is_error,
                                ..
                            }) => {
                                if let Some(buf) = reply_buffers.get_mut(&session_id) {
                                    buf.record_tool_end(tool_id, *elapsed_ms, *is_error);
                                }
                            }
                            _ => {}
                        }

                        // Run end: deliver the buffered reply — morph the
                        // status card (single message), or freeze it as a
                        // terminal receipt and flush the reply at the bottom
                        // when the user posted mid-run.
                        if let Event::Agent(AgentEvent::Lifecycle {
                            state: AgentStatus::Stopped { reason },
                        }) = &envelope.event
                        {
                            let reply = reply_buffers
                                .remove(&session_id)
                                .map(reply::RunReplyBuffer::into_reply);
                            deliver_reply(
                                &obs,
                                &adapter,
                                &routing,
                                reply,
                                tool_trace,
                                observability,
                                &session_id,
                                SettleKind::Stopped(reason),
                            )
                            .await;
                            continue;
                        }

                        // Observability: cheap state updates + throttled
                        // in-place PATCHes (design: feishu-channel-observability).
                        if observability {
                            obs.handle_event(
                                &adapter,
                                &session_id,
                                &routing.external_chat_id,
                                routing.reply_msg_id.as_deref(),
                                &envelope.event,
                            ).await;
                        }

                        // Typing indicator as the fallback progress signal on
                        // platforms without status cards (or when
                        // observability is disabled).
                        if matches!(envelope.event, Event::Model(ModelEvent::Request { .. }))
                            && (!supports_cards || !observability)
                        {
                            let _ = adapter.send_typing(&routing.external_chat_id).await;
                        }
                    }
                }
            }

            info!("channel event forwarder exited");
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

    /// Check whether a session is routed from an external channel, regardless
    /// of whether the channel instance is currently running.
    pub async fn is_channel_session(&self, session_id: &SessionId) -> bool {
        matches!(
            self.store.find_routing_by_session(session_id).await,
            Ok(Some(_))
        )
    }
}

/// Flush a run's final reply as a new message (observability off, platforms
/// without card support, or the mid-run split where the status card freezes
/// as a terminal receipt): send the final text as a single message bubble,
/// with the run trace attached (collapsible panel on card-capable
/// platforms, plain-text lines otherwise). Runs without any text are
/// skipped, matching the pre-buffering behavior.
async fn flush_reply(
    adapter: &Arc<dyn PlatformAdapter>,
    routing: &SessionRouting,
    reply: reply::FinalReply,
    tool_trace: bool,
) {
    if reply.text().is_none() {
        return;
    }
    let sent = if tool_trace && adapter.supports_status_card() && reply.has_trace() {
        match reply::render_card(&reply, None) {
            Some(card) => {
                adapter
                    .send_card(
                        &routing.external_chat_id,
                        &card,
                        routing.reply_msg_id.as_deref(),
                    )
                    .await
            }
            // Unreachable in practice (a text reply always renders) — skip
            // rather than panic; the run's content was already delivered by
            // the settle path or is simply absent.
            None => return,
        }
    } else {
        let text = if tool_trace {
            reply::render_plain(&reply)
        } else {
            reply.into_text().unwrap_or_default()
        };
        adapter
            .send_message(
                &routing.external_chat_id,
                vec![ContentBlock::Text { text }],
                routing.reply_msg_id.as_deref(),
            )
            .await
    };
    if let Err(e) = sent {
        error!(error = %e, "failed to send reply to platform");
    }
}

/// How a run ends, for reply-delivery purposes.
#[derive(Clone, Copy)]
enum SettleKind<'a> {
    Stopped(&'a crate::event::StopReason),
    Timeout,
}

async fn settle_with(
    obs: &Arc<ObsTracker>,
    session_id: &SessionId,
    kind: SettleKind<'_>,
    reply: Option<reply::FinalReply>,
) -> Option<reply::FinalReply> {
    match kind {
        SettleKind::Stopped(reason) => obs.handle_stopped(session_id, reason, reply).await,
        SettleKind::Timeout => obs.handle_timeout(session_id, reply).await,
    }
}

/// Deliver a run's final reply. Card-capable platforms with observability
/// morph the status card into it (one message per run) — or, when the user
/// posted mid-run, freeze the card as a terminal receipt and flush the
/// reply as a new message at the bottom. All other cases flush as a new
/// message and settle the obs state without a reply. When the rich settle
/// comes back unsettled (no run state, or the settle send failed), the
/// reply falls back to a plain flush so content is never silently lost.
#[allow(clippy::too_many_arguments)]
async fn deliver_reply(
    obs: &Arc<ObsTracker>,
    adapter: &Arc<dyn PlatformAdapter>,
    routing: &SessionRouting,
    reply: Option<reply::FinalReply>,
    tool_trace: bool,
    observability: bool,
    session_id: &SessionId,
    kind: SettleKind<'_>,
) {
    if observability && adapter.supports_status_card() {
        if obs.has_mid_run_posts(session_id) {
            let _ = settle_with(obs, session_id, kind, None).await;
            if let Some(reply) = reply {
                flush_reply(adapter, routing, reply, tool_trace).await;
            }
        } else if let Some(reply) = settle_with(obs, session_id, kind, reply).await {
            // Nothing settled — fall back to a plain message instead of
            // dropping the reply.
            flush_reply(adapter, routing, reply, tool_trace).await;
        }
    } else {
        // Platforms without card support cannot morph — the reply goes out
        // as a plain message; obs still settles its memory-only state
        // (typing fallback).
        if let Some(reply) = reply {
            flush_reply(adapter, routing, reply, tool_trace).await;
        }
        if observability {
            let _ = settle_with(obs, session_id, kind, None).await;
        }
    }
}

async fn handle_incoming_message(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: Arc<Kernel>,
    msg: ChannelMessage,
    obs: &Arc<ObsTracker>,
) -> Result<Option<String>> {
    let chat_id = msg.external_chat_id.clone();
    let reply_msg_id = reply_anchor(&msg, config.reply_in_thread);
    let mapping_key = session_mapping_key(&msg, &chat_id, config.reply_in_thread);

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
            record_receipt(config, obs, &sid, &msg);
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
            record_receipt(config, obs, &sid, &msg);
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
            let models = kernel.list_models().await?;
            if !models.iter().any(|model| model.name == key) {
                return Ok(Some(format_unknown_model(&key, &models)));
            }
            if is_chat_level_message(&msg, config.reply_in_thread) {
                // Switch the whole chat: update every existing thread
                // session routed to this chat, and persist the choice on
                // the chat-level session so future threads inherit it.
                let chat_sid =
                    get_or_create_session(channel_name, store, &kernel, &chat_id, &chat_id, None)
                        .await?;
                kernel.set_session_model(&chat_sid, &key).await?;
                for (mk, sid) in store.list_mappings(channel_name).await? {
                    if mk == chat_id {
                        continue;
                    }
                    if let Ok(Some(routing)) = store.find_routing_by_session(&sid).await {
                        if routing.external_chat_id == chat_id {
                            kernel.set_session_model(&sid, &key).await?;
                        }
                    }
                }
                return Ok(Some(format!(
                    "Switched all threads in this chat to `{key}`. It takes effect on the next model invocation."
                )));
            }
            let sid = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                &mapping_key,
                reply_msg_id.as_deref(),
            )
            .await?;
            kernel.set_session_model(&sid, &key).await?;
            Ok(Some(format!(
                "Switched to `{key}`. It takes effect on the next model invocation."
            )))
        }
        ChannelCommand::InvalidModelCommand => Ok(Some(
            "Usage: `/model` or `/model <model_key>`. Use `/models` to list models.".to_string(),
        )),
        ChannelCommand::Info => {
            // Top-level group messages in reply_in_thread mode show the
            // chat-level session; in-thread messages show the thread's.
            let chat_level = is_chat_level_message(&msg, config.reply_in_thread);
            let key = if chat_level { &chat_id } else { &mapping_key };
            let sid = get_or_create_session(
                channel_name,
                store,
                &kernel,
                &chat_id,
                key,
                // Don't re-anchor the chat-level routing to the /info message.
                if chat_level {
                    None
                } else {
                    reply_msg_id.as_deref()
                },
            )
            .await?;
            let session = kernel.get_session(&sid).await?;
            let model_key = kernel.get_session_model(&sid).await;
            let models = kernel.list_models().await?;
            let running_subagents = kernel
                .list_subagents(&sid)
                .await?
                .into_iter()
                .filter(|s| s.is_running)
                .count();
            let shells = kernel.list_background_shells(&sid);
            Ok(Some(format_session_info(
                &session,
                &model_key,
                &models,
                running_subagents,
                &shells,
            )))
        }
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
            record_receipt(config, obs, &sid, &msg);
            kernel.send_steer(&sid, msg.content);
            Ok(None)
        }
    }
}

/// Record a user message for the mid-run post detection (morph vs.
/// new-message settle). No-op when observability is disabled or the
/// message carries no platform ID.
fn record_receipt(
    config: &ChannelConfig,
    obs: &ObsTracker,
    session_id: &SessionId,
    msg: &ChannelMessage,
) {
    if !config.observability {
        return;
    }
    if let Some(msg_id) = &msg.external_message_id {
        obs.record_receipt(session_id, msg_id.clone());
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

/// Compute the session mapping key for an incoming message.
///
/// In `reply_in_thread` group chats each conversation thread gets its own
/// session. The bot's reply is what opens the thread, so the thread's
/// *starting* message itself carries no `thread_id` — but every message
/// inside the thread replies to the thread's root message (Feishu sets
/// `root_id` to it). Keying by root/message id therefore keeps a whole
/// thread in one session while each new top-level message starts a fresh
/// session.
fn session_mapping_key(msg: &ChannelMessage, chat_id: &str, reply_in_thread: bool) -> String {
    if reply_in_thread && msg.is_group {
        msg.root_id
            .clone()
            .or_else(|| msg.thread_id.clone())
            .or_else(|| msg.external_message_id.clone())
            .unwrap_or_else(|| chat_id.to_string())
    } else {
        msg.thread_id.clone().unwrap_or_else(|| chat_id.to_string())
    }
}

/// Whether a message is a top-level group message in `reply_in_thread`
/// mode (i.e. not inside any thread). Such messages address the chat as a
/// whole — e.g. a top-level `/model` switches every thread session, and a
/// top-level `/info` shows the chat-level session.
fn is_chat_level_message(msg: &ChannelMessage, reply_in_thread: bool) -> bool {
    reply_in_thread && msg.is_group && msg.thread_id.is_none() && msg.root_id.is_none()
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

const CMD_MODELS: &str = "/models";
const CMD_MODEL: &str = "/model";
const CMD_CLEAR: &str = "/clear";
const CMD_STOP: &str = "/stop";
const CMD_STEER: &str = "/steer";
const CMD_QUEUE: &str = "/queue";
const CMD_INFO: &str = "/info";

/// All channel command prefixes, longest-first so `/models` is matched
/// before `/model` (the latter is a prefix of the former).
const CMD_PREFIXES: &[&str] = &[
    CMD_MODELS, CMD_MODEL, CMD_CLEAR, CMD_STOP, CMD_STEER, CMD_QUEUE, CMD_INFO,
];

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
    /// Show basic info about the current session.
    Info,
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

    let Some(&command) = CMD_PREFIXES.iter().find(|prefix| cmd.starts_with(**prefix)) else {
        return ChannelCommand::None;
    };

    match command {
        CMD_CLEAR if parts.next().is_none() => ChannelCommand::Clear,
        CMD_STOP if parts.next().is_none() => ChannelCommand::Stop,
        CMD_INFO if parts.next().is_none() => ChannelCommand::Info,
        CMD_STEER | CMD_QUEUE => {
            let rest = parts.collect::<Vec<_>>().join(" ");
            if rest.is_empty() {
                ChannelCommand::None
            } else if command == CMD_QUEUE {
                ChannelCommand::Queue(rest)
            } else {
                ChannelCommand::Steer(rest)
            }
        }
        CMD_MODELS | CMD_MODEL => match (parts.next(), parts.next()) {
            (None, None) if command == CMD_MODELS => ChannelCommand::ListModels,
            (None, None) => ChannelCommand::CurrentModel,
            (Some(key), None) => ChannelCommand::SwitchModel(key.to_string()),
            _ => ChannelCommand::InvalidModelCommand,
        },
        _ => ChannelCommand::None,
    }
}

pub(super) fn has_channel_command_prefix(raw_text: &str) -> bool {
    let command = raw_text.split_whitespace().next().unwrap_or_default();
    CMD_PREFIXES
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

fn format_session_info(
    session: &crate::types::SessionResponse,
    model_key: &str,
    models: &[crate::kernel::ModelInfo],
    running_subagents: usize,
    shells: &[crate::agent::BackgroundShellTask],
) -> String {
    let model = models.iter().find(|m| m.name == model_key).map_or_else(
        || format!("`{model_key}`"),
        |m| {
            format!(
                "`{}` · {} · `{}` · {}k ctx",
                m.name,
                m.provider,
                m.model_id,
                m.context_window / 1000
            )
        },
    );
    // Sessions without a persisted model key resolve to the default model.
    let default_marker = if session.model_key.is_none() {
        " (default)"
    } else {
        ""
    };
    let shells_text = if shells.is_empty() {
        "none".to_string()
    } else {
        shells
            .iter()
            .map(|s| {
                format!(
                    "`{}` (pid {}, {})",
                    s.command,
                    s.pid,
                    format_age(s.started_at)
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    [
        "**Session Info**".to_string(),
        String::new(),
        format!("- ID: `{}`", session.id.0),
        format!("- Model: {model}{default_marker}"),
        format!("- Status: {}", session.phase),
        format!(
            "- Created: {} · Active: {}",
            format_age(session.created_at),
            format_age(session.updated_at)
        ),
        format!(
            "- Permission: {}",
            session.auto_approve_level.as_deref().unwrap_or("default")
        ),
        format!("- Subagents (running): {running_subagents}"),
        format!("- Background Shell: {shells_text}"),
    ]
    .join("\n")
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
