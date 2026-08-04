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
    obs::ObsTracker, reply, ChannelConfig, ChannelEvent, ChannelInfo, ChannelMessage,
    ChannelStatus, ChannelStore, HistoryContainer, HistoryMessage, PlatformAdapter, PlatformConfig,
    SessionRouting,
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

        let kv = kernel.upgrade().and_then(|k| k.kv_cache());
        let adapter = build_adapter(&config.platform, kv);
        let status = Arc::new(AtomicU8::new(STATUS_CONNECTING));

        let (incoming_tx, incoming_rx) = mpsc::channel::<ChannelEvent>(256);
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
                    Some(event) = incoming_rx.recv() => {
                        let msg = match event {
                            ChannelEvent::Message(msg) => msg,
                            // Platform events and callbacks bypass access
                            // control / the mention requirement — they are
                            // not user chat messages (callbacks have their
                            // own admin check downstream). Handled off-loop
                            // so slow platform APIs (notification fans out
                            // to N admins, grant + card updates) can't stall
                            // chat processing; the resolve race is guarded
                            // by the store's conditional update.
                            ChannelEvent::DocPermissionApplied(req) => {
                                let (name, config, store, adapter) = (
                                    name_proc.clone(),
                                    config_proc.clone(),
                                    Arc::clone(&store),
                                    Arc::clone(&adapter_proc),
                                );
                                tokio::spawn(async move {
                                    super::approval::handle_doc_permission_applied(
                                        &name, &config, &store, &adapter, req,
                                    ).await;
                                });
                                continue;
                            }
                            ChannelEvent::CardAction(action) => {
                                let (name, config, store, adapter) = (
                                    name_proc.clone(),
                                    config_proc.clone(),
                                    Arc::clone(&store),
                                    Arc::clone(&adapter_proc),
                                );
                                tokio::spawn(async move {
                                    super::approval::handle_card_action(
                                        &name, &config, &store, &adapter, action,
                                    ).await;
                                });
                                continue;
                            }
                        };
                        // Route to kernel
                        let Some(coord) = kernel.upgrade() else {
                            warn!("kernel gone, stopping processing loop");
                            break;
                        };
                        match gate_message(&adapter_proc, &config_proc, &msg).await {
                            Gate::Allow => {}
                            Gate::Denied => continue,
                            // Non-addressed chatter still counts as a
                            // mid-run post when it lands in a running
                            // session's conversation.
                            Gate::NotAddressed => {
                                record_passive_receipt(
                                    &name_proc,
                                    &config_proc,
                                    &store,
                                    &obs_proc,
                                    &msg,
                                    |sid| coord.is_session_running(sid),
                                )
                                .await;
                                continue;
                            }
                        }
                        let handled = handle_incoming_message(
                            &name_proc,
                            &config_proc,
                            &store,
                            coord,
                            msg.clone(),
                            &obs_proc,
                            &adapter_proc,
                        ).await;
                        // Advance the cursor only after a successfully
                        // handled message; a failed trigger consumed
                        // nothing (a history fetch failing mid-handle
                        // still skips its window — best-effort).
                        if handled.is_ok() {
                            advance_history_cursor(&config_proc, &store, &name_proc, &msg).await;
                        }
                        match handled {
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
                        if let Some(k) = kernel.upgrade() {
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
                                .filter(|sid| !k.is_session_running(sid))
                                .cloned()
                                .collect();
                            for sid in dead {
                                // Look up BEFORE draining the buffer: a
                                // failed lookup must not drop the reply.
                                let routing = match store.find_routing_by_session(&sid).await {
                                    Ok(Some(r)) => r,
                                    // Routing gc'd: the reply is undeliverable
                                    // — drop the buffer instead of re-querying
                                    // the store every sweep forever.
                                    Ok(None) => {
                                        reply_buffers.remove(&sid);
                                        continue;
                                    }
                                    // Transient store error: keep the buffer
                                    // and retry next sweep.
                                    Err(_) => continue,
                                };
                                let Some((adapter, tool_trace, observability, mid_run_split)) = instances
                                    .get(&routing.channel_name)
                                    .map(|i| {
                                        (
                                            Arc::clone(&i.adapter),
                                            i.config.tool_trace,
                                            i.config.observability,
                                            i.config.mid_run_split,
                                        )
                                    })
                                else {
                                    // Channel instance gone: undeliverable.
                                    reply_buffers.remove(&sid);
                                    continue;
                                };
                                let Some(buf) = reply_buffers.remove(&sid) else { continue };
                                deliver_reply(
                                    &obs,
                                    &adapter,
                                    &routing,
                                    Some(buf.into_reply()),
                                    tool_trace,
                                    observability,
                                    mid_run_split,
                                    &sid,
                                    SettleKind::Timeout,
                                    &kernel,
                                )
                                .await;
                            }
                            obs.sweep_dead_sessions(|sid| k.is_session_running(sid)).await;
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

                        let (adapter, observability, tool_trace, mid_run_split) = {
                            let Some(instance) = instances.get(&routing.channel_name) else { continue };
                            (
                                Arc::clone(&instance.adapter),
                                instance.config.observability,
                                instance.config.tool_trace,
                                instance.config.mid_run_split,
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
                                // One step per completed model response —
                                // tool-call-only turns (no text) count too.
                                let text = super::blocks_to_text(content);
                                reply_buffers
                                    .entry(session_id.clone())
                                    .or_default()
                                    .record_model_end(&text);
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
                        // when the user posted mid-run (`mid_run_split`).
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
                                mid_run_split,
                                &session_id,
                                SettleKind::Stopped(reason),
                                &kernel,
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
///
/// Returns `true` when the reply was actually delivered — `false` when
/// there was nothing to send or every send attempt failed (the caller
/// relies on it to decide whether the trace still needs a home).
async fn flush_reply(
    adapter: &Arc<dyn PlatformAdapter>,
    routing: &SessionRouting,
    reply: reply::FinalReply,
    tool_trace: bool,
) -> bool {
    if reply.text().is_none() {
        return false;
    }
    if tool_trace && adapter.supports_status_card() && reply.has_trace() {
        match reply::render_card(&reply, None) {
            Some(card) => {
                match adapter
                    .send_card(
                        &routing.external_chat_id,
                        &card,
                        routing.reply_msg_id.as_deref(),
                    )
                    .await
                {
                    Ok(Some(_)) => return true,
                    // Platform skipped the card — fall through to text.
                    Ok(None) => {}
                    // The card was rejected (oversize payload, API error) —
                    // fall through to a plain text send so the reply content
                    // is never dropped.
                    Err(e) => {
                        warn!(error = %e, "reply card send failed, falling back to plain text");
                    }
                }
            }
            // Unreachable in practice (a text reply always renders) — skip
            // rather than panic; the run's content was already delivered by
            // the settle path or is simply absent.
            None => return false,
        }
    }
    let text = if tool_trace {
        reply::render_plain(&reply)
    } else {
        reply.into_text().unwrap_or_default()
    };
    match adapter
        .send_message(
            &routing.external_chat_id,
            vec![ContentBlock::Text { text }],
            routing.reply_msg_id.as_deref(),
        )
        .await
    {
        Ok(_) => true,
        Err(e) => {
            error!(error = %e, "failed to send reply to platform");
            false
        }
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

/// Freeze the status card in place as a terminal receipt (mid-run split;
/// see `deliver_reply`). `keep_trace` = the card itself carries the run
/// trace panel — false when the reply message carries it instead.
async fn freeze_with(
    obs: &Arc<ObsTracker>,
    session_id: &SessionId,
    kind: SettleKind<'_>,
    keep_trace: bool,
) {
    match kind {
        SettleKind::Stopped(reason) => obs.freeze_stopped(session_id, reason, keep_trace).await,
        SettleKind::Timeout => obs.freeze_timeout(session_id, keep_trace).await,
    }
}

/// Deliver a run's final reply, then its attachment files. Declared
/// attachments (`<yomi_attachments>` blocks, stripped at record time) are
/// resolved up front — resolution notes ride with the reply text — while
/// the files themselves go out AFTER the reply, landing at the bottom of
/// the chat. The reply itself: card-capable platforms with observability
/// morph the status card into it (one message per run) — or, when the user
/// posted mid-run and `mid_run_split` is enabled, flush the reply as a new
/// message at the bottom carrying the run trace, then freeze the card in
/// place as a terminal receipt (the card keeps the trace panel itself
/// whenever the reply didn't carry it). All other cases flush as a new
/// message and settle the obs state without a reply. When the rich
/// settle comes back unsettled (no run state, or the settle send failed),
/// the reply falls back to a plain flush so content is never silently lost.
#[allow(clippy::too_many_arguments)]
async fn deliver_reply(
    obs: &Arc<ObsTracker>,
    adapter: &Arc<dyn PlatformAdapter>,
    routing: &SessionRouting,
    reply: Option<reply::FinalReply>,
    tool_trace: bool,
    observability: bool,
    mid_run_split: bool,
    session_id: &SessionId,
    kind: SettleKind<'_>,
    kernel: &std::sync::Weak<Kernel>,
) {
    let mut reply = reply;
    // Split files off the reply up front: resolution failures become notes
    // on the reply text; the files are sent after the reply below.
    let files = match reply.as_mut() {
        Some(reply) if !reply.attachments().is_empty() => {
            // Best-effort workspace lookup for relative paths; a gone
            // kernel still allows absolute-path attachments.
            let cwd = match kernel.upgrade() {
                Some(k) => Some(k.session_cwd(session_id).await),
                None => None,
            };
            super::attachments::resolve_attachments(cwd.as_deref(), reply).await
        }
        _ => Vec::new(),
    };
    if observability && adapter.supports_status_card() {
        if mid_run_split && obs.has_mid_run_posts(session_id) {
            // The reply lands as a new message below the user's mid-run
            // posts, carrying the run trace; the status card then freezes
            // in place as a terminal receipt. Flush first and freeze with
            // the outcome: the card keeps the trace panel itself whenever
            // the reply didn't carry it (nothing delivered — no text or
            // every send failed — or the trace is disabled), so the trace
            // is never lost.
            let delivered = match reply {
                Some(reply) => flush_reply(adapter, routing, reply, tool_trace).await,
                None => false,
            };
            let keep_trace = !tool_trace || !delivered;
            freeze_with(obs, session_id, kind, keep_trace).await;
        } else {
            // Morph in place (no mid-run posts, or the split disabled).
            // Receipts only drive the split decision — with the split
            // disabled they must not suppress the settle reaction for
            // this silent in-place morph.
            if !mid_run_split {
                obs.clear_receipts(session_id);
            }
            if let Some(reply) = settle_with(obs, session_id, kind, reply).await {
                // Nothing settled — fall back to a plain message instead of
                // dropping the reply.
                flush_reply(adapter, routing, reply, tool_trace).await;
            }
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

    // Attachments last: files land at the bottom of the chat, below the
    // reply text/card.
    super::attachments::send_attachments(adapter, routing, files).await;
}

/// Outcome of gating one incoming message (see `gate_message`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Gate {
    /// Accepted — process normally.
    Allow,
    /// Access denied (disabled / allowlist miss / blocklist hit).
    Denied,
    /// Access is fine but the message doesn't address the bot
    /// (mention-missed group chatter): not processed, but it may still
    /// count as a mid-run post (see `record_passive_receipt`).
    NotAddressed,
}

/// Gate one incoming message: enforce access control and the mention
/// requirement, emitting the platform's gate reactions.
///
/// Reaction policy: an accepted, addressed message gets the platform's ack
/// reaction (when it has one); an allowlist miss gets the access-denied
/// reaction — but only when the message addresses the bot, so random group
/// chatter stays untouched. Blocklist hits, disabled channels, and
/// non-addressed messages stay silent.
async fn gate_message(
    adapter: &Arc<dyn PlatformAdapter>,
    config: &ChannelConfig,
    msg: &ChannelMessage,
) -> Gate {
    let addressed = !config.require_mention || msg.is_mention;
    if let Err(e) = config.check_access(&msg.external_chat_id, &msg.external_user_id) {
        info!(channel = %config.name, error = %e, "access denied");
        if addressed && e.is_allowlist_miss() {
            send_gate_reaction(
                adapter,
                config,
                msg,
                config.platform.access_denied_reaction(),
            )
            .await;
        }
        return Gate::Denied;
    }
    if !addressed {
        info!(channel = %config.name, chat_id = %msg.external_chat_id, "ignoring non-mention message");
        return Gate::NotAddressed;
    }
    send_gate_reaction(adapter, config, msg, config.platform.ack_reaction()).await;
    Gate::Allow
}

/// Best-effort gate reaction; needs a message to target and only logs on
/// failure.
async fn send_gate_reaction(
    adapter: &Arc<dyn PlatformAdapter>,
    config: &ChannelConfig,
    msg: &ChannelMessage,
    emoji: &'static str,
) {
    let Some(message_id) = msg.external_message_id.as_deref() else {
        return;
    };
    if let Err(e) = adapter
        .send_reaction(&msg.external_chat_id, message_id, emoji)
        .await
    {
        warn!(channel = %config.name, error = %e, "gate reaction failed");
    }
}

/// Why a `/thread` command cannot open a thread off this message (its
/// refusal text), if it can't. Telegram has no threads at all — the
/// message-id-keyed session would be an orphan there; without the
/// message id there is nothing to anchor and key by, and the chat-level
/// session must never be hijacked. Also gates the history-cursor
/// advance: a refused command ran nothing and settles no prior context.
fn thread_refusal(config: &ChannelConfig, msg: &ChannelMessage) -> Option<&'static str> {
    // Already in a thread: the command's promise can't be kept —
    // refuse rather than silently run a plain trigger (or worse, fork
    // a parallel session into the same visible thread).
    if msg.thread_id.is_some() {
        return Some("Already in a thread — just send your message directly.");
    }
    if !matches!(config.platform, PlatformConfig::Feishu { .. }) {
        return Some("This platform does not support threads.");
    }
    if msg.external_message_id.is_none() {
        return Some("Cannot open a thread off this message.");
    }
    None
}

async fn handle_incoming_message(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: Arc<Kernel>,
    msg: ChannelMessage,
    obs: &Arc<ObsTracker>,
    adapter: &Arc<dyn PlatformAdapter>,
) -> Result<Option<String>> {
    let chat_id = msg.external_chat_id.clone();
    let reply_msg_id = reply_anchor(&msg, config.reply_in_thread);
    let mapping_key =
        effective_mapping_key(store, channel_name, &msg, &chat_id, config.reply_in_thread).await?;
    tracing::debug!(
        channel = %channel_name,
        chat_id = %chat_id,
        msg_id = msg.external_message_id.as_deref().unwrap_or(""),
        thread_id = msg.thread_id.as_deref().unwrap_or(""),
        root_id = msg.root_id.as_deref().unwrap_or(""),
        mapping_key = %mapping_key,
        "session mapping"
    );

    let cmd = parse_channel_command(msg.raw_text.as_deref());
    match cmd {
        ChannelCommand::Help => Ok(Some(HELP_TEXT.to_string())),
        ChannelCommand::Clear => {
            if let Some(sid) = store.find_mapping(channel_name, &mapping_key).await? {
                if let Err(e) = kernel.clear_session(&sid) {
                    tracing::warn!("Failed to clear session {}: {}", sid.0, e);
                }
            }
            Ok(Some("Context cleared.".to_string()))
        }
        ChannelCommand::Compact => {
            // Fire-and-forget: compacting progress shows on the status
            // card when a run is live; otherwise this ack is the only
            // feedback (outcome is only logged).
            if let Some(sid) = store.find_mapping(channel_name, &mapping_key).await? {
                kernel.compact_session(&sid);
                return Ok(Some("⏳ Compacting context…".to_string()));
            }
            Ok(Some("No session to compact.".to_string()))
        }
        ChannelCommand::Stop => {
            if let Some(sid) = store.find_mapping(channel_name, &mapping_key).await? {
                kernel.cancel(&sid);
                return Ok(Some("Stopped.".to_string()));
            }
            Ok(Some("No active session to stop.".to_string()))
        }
        ChannelCommand::Restart => {
            if let Some(deny) = super::approval::check_admin(config, &msg.external_user_id) {
                return Ok(Some(deny));
            }
            if !kernel.can_restart() {
                return Ok(Some("Restart is not supported by this daemon.".to_string()));
            }
            let runs = kernel.live_session_count();
            let text = if runs == 0 {
                "🔄 Restarting daemon…".to_string()
            } else {
                format!("🔄 Restarting daemon… {runs} active run(s) will be interrupted.")
            };
            // The ack is sent inline: the usual spawned command reply
            // could be aborted when the daemon shuts down mid-flight.
            if let Err(e) = adapter
                .send_message(
                    &chat_id,
                    vec![ContentBlock::Text { text }],
                    reply_msg_id.as_deref(),
                )
                .await
            {
                warn!(channel = %channel_name, error = %e, "restart ack send failed");
            }
            kernel.request_restart();
            Ok(None)
        }
        ChannelCommand::Steer(text) => {
            let (sid, mut blocks) = prepare_trigger(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                obs,
                &msg,
                TriggerKind::Normal,
            )
            .await?;
            kernel.note_title_input(&sid, &text);
            blocks.push(ContentBlock::Text { text });
            kernel.send_steer(&sid, blocks).await;
            Ok(None)
        }
        ChannelCommand::Thread(text) => {
            // One-shot thread: key by and anchor to the command
            // message itself, so the reply opens a thread off it (see
            // prepare_trigger); follow-ups inside adopt the session
            // via the thread root (see effective_mapping_key).
            if let Some(refusal) = thread_refusal(config, &msg) {
                return Ok(Some(refusal.to_string()));
            }
            let (sid, mut blocks) = prepare_trigger(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                obs,
                &msg,
                TriggerKind::OneShotThread,
            )
            .await?;
            kernel.note_title_input(&sid, &text);
            blocks.push(ContentBlock::Text { text });
            // Deferred image download — as for a plain trigger, only
            // now, after the gate, does an attached image cost
            // bandwidth.
            append_message_images(
                adapter,
                msg.external_message_id.as_deref().unwrap_or(""),
                &msg.image_keys,
                &mut blocks,
            )
            .await;
            kernel.send_steer(&sid, blocks).await;
            Ok(None)
        }
        ChannelCommand::InvalidThreadCommand => Ok(Some(
            "Usage: `/thread <text>` — the reply opens a new thread.".to_string(),
        )),
        ChannelCommand::Queue(text) => {
            let (sid, mut blocks) = prepare_trigger(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                obs,
                &msg,
                TriggerKind::Normal,
            )
            .await?;
            kernel.note_title_input(&sid, &text);
            blocks.push(ContentBlock::Text { text });
            // The title was just fed from the user's own text — don't
            // let send_message re-extract it from the merged blocks.
            kernel.send_message_inner(&sid, blocks, false).await?;
            Ok(None)
        }
        ChannelCommand::ListModels => {
            let models = kernel.list_models().await?;
            let current =
                session_model_key(channel_name, store, &kernel, &chat_id, &mapping_key).await?;
            Ok(Some(format_model_list(&models, &current)))
        }
        ChannelCommand::CurrentModel => {
            let models = kernel.list_models().await?;
            let current =
                session_model_key(channel_name, store, &kernel, &chat_id, &mapping_key).await?;
            Ok(Some(format_current_model(&models, &current)))
        }
        ChannelCommand::SwitchModel(key) => {
            let models = kernel.list_models().await?;
            if !models.iter().any(|model| model.name == key) {
                return Ok(Some(format_unknown_model(&key, &models)));
            }
            // Chat level — or a thread without a session: switch the chat
            // session instead and let the thread inherit it (also keeps
            // thread mappings conversation-only).
            let chat_level = is_chat_level_message(&msg, config.reply_in_thread)
                || store
                    .find_mapping(channel_name, &mapping_key)
                    .await?
                    .is_none();
            if chat_level {
                // Switch the whole chat: update every existing thread
                // session routed to this chat, and persist the choice on
                // the chat-level session so future threads inherit it.
                let (chat_sid, _) =
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
            // find_mapping above proved the mapping exists; this only
            // refreshes its routing anchor.
            let (sid, _) = get_or_create_session(
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
            // Chat-level messages show the chat session, in-thread ones
            // the thread's. Read-only: never creates a session or mapping.
            let chat_level = is_chat_level_message(&msg, config.reply_in_thread);
            let key = if chat_level { &chat_id } else { &mapping_key };
            let Some(sid) = store.find_mapping(channel_name, key).await? else {
                let model_key =
                    session_model_key(channel_name, store, &kernel, &chat_id, key).await?;
                return Ok(Some(format!(
                    "No session yet in this {}. First message will use `{model_key}`.",
                    if chat_level { "chat" } else { "thread" },
                )));
            };
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
        ChannelCommand::Permits => {
            super::approval::list_pending(channel_name, config, store, &msg.external_user_id).await
        }
        ChannelCommand::Approve { id, perm } => {
            super::approval::approve(
                channel_name,
                config,
                store,
                adapter,
                &msg.external_user_id,
                id,
                perm.as_deref(),
            )
            .await
        }
        ChannelCommand::Deny { id } => {
            super::approval::deny(
                channel_name,
                config,
                store,
                adapter,
                &msg.external_user_id,
                id,
            )
            .await
        }
        ChannelCommand::InvalidApprovalCommand => Ok(Some(super::approval::usage())),
        ChannelCommand::None => {
            let (sid, mut content) = prepare_trigger(
                channel_name,
                config,
                store,
                &kernel,
                adapter,
                obs,
                &msg,
                TriggerKind::Normal,
            )
            .await?;
            // Title from the user's bare text: msg.content carries the
            // adapter's metadata header ([ts][from_user_id:…]), and
            // context blocks merge ahead of it (see note_title_input).
            if let Some(raw) = msg.raw_text.as_deref() {
                kernel.note_title_input(&sid, raw);
            }
            content.extend(msg.content);
            // Deferred image download — only now, after the gate, does
            // an attached image cost bandwidth.
            append_message_images(
                adapter,
                msg.external_message_id.as_deref().unwrap_or(""),
                &msg.image_keys,
                &mut content,
            )
            .await;
            kernel.send_steer(&sid, content).await;
            Ok(None)
        }
    }
}

/// The history container for a triggering message: its thread when sent
/// inside one, otherwise the chat itself.
fn history_container(msg: &ChannelMessage) -> HistoryContainer {
    match &msg.thread_id {
        Some(tid) => HistoryContainer::Thread(tid.clone()),
        None => HistoryContainer::Chat(msg.external_chat_id.clone()),
    }
}

/// Advance the container's history cursor to a processed message's
/// timestamp (group only, monotonic) — only for messages that settle
/// prior context: run triggers consume it, `/clear` discards it.
async fn advance_history_cursor(
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    msg: &ChannelMessage,
) {
    if config.history_context == 0 || !msg.is_group {
        return;
    }
    let cmd = parse_channel_command(msg.raw_text.as_deref());
    // A refused `/thread` ran nothing — it settles no prior context.
    if !consumes_history(&cmd)
        || (matches!(cmd, ChannelCommand::Thread(_)) && thread_refusal(config, msg).is_some())
    {
        return;
    }
    let Some(ts) = msg.create_time else {
        return;
    };
    let container = history_container(msg);
    let current = store
        .get_history_cursor(channel_name, container.id())
        .await
        .ok()
        .flatten();
    if current.is_none_or(|c| ts > c) {
        let _ = store
            .set_history_cursor(channel_name, container.id(), ts)
            .await;
    }
}

/// Delivery state of a thread's root message for the current trigger.
/// The root of a human-created thread is exactly the context the bot
/// needs — but it must arrive exactly once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootDelivery {
    /// Fresh session: the root has not been delivered yet.
    Pending,
    /// Reused session: an earlier trigger already delivered the root.
    Consumed,
    /// The quoted block (any link of its chain) just delivered the root
    /// for this trigger.
    ByQuote,
}

/// Assemble recent-chat history since the last trigger as a
/// `<recent_chat_history>` block plus images; best-effort. The cursor
/// advances at fetch time, so a later send failure can leave a small
/// gap (accepted). `root`: the thread root's delivery state — non-
/// `Pending` dedups it; a `Pending` root missing from the page gets a
/// direct-fetch backstop.
async fn maybe_history_prefix(
    adapter: &Arc<dyn PlatformAdapter>,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    msg: &ChannelMessage,
    root: RootDelivery,
) -> Option<Vec<ContentBlock>> {
    if config.history_context == 0 || !msg.is_group {
        return None;
    }
    // With reply_in_thread, a channel-level trigger opens a fresh thread —
    // the chat's cross-topic chatter is noise there, not context. Triggers
    // inside an existing thread still get that thread's history.
    if config.reply_in_thread && msg.thread_id.is_none() {
        return None;
    }
    let container = history_container(msg);
    let cursor = store
        .get_history_cursor(channel_name, container.id())
        .await
        .ok()
        .flatten();
    let messages = match adapter
        .fetch_history(&container, cursor, config.history_context)
        .await
    {
        Ok(m) => m,
        Err(e) => {
            warn!(error = %e, "history fetch failed, continuing without context");
            return None;
        }
    };
    // Advance the cursor to the newest fetched message so it isn't
    // re-fetched next time.
    if let Some(newest_ts) = messages.iter().map(|m| m.create_time).max() {
        if let Err(e) = store
            .set_history_cursor(channel_name, container.id(), newest_ts)
            .await
        {
            warn!(error = %e, "failed to advance history cursor");
        }
    }
    // Drop the triggering message itself — it's delivered verbatim below.
    let trigger_id = msg.external_message_id.as_deref();
    // The thread root stays unless already delivered (see RootDelivery).
    let drop_root = matches!(&container, HistoryContainer::Thread(_))
        && msg.root_id.is_some()
        && root != RootDelivery::Pending;
    let fetched_root = fetch_root_backstop(adapter, &container, msg, root, &messages, cursor).await;
    let history: Vec<&HistoryMessage> = fetched_root
        .iter()
        .chain(
            messages
                .iter()
                .filter(|m| Some(m.message_id.as_str()) != trigger_id)
                .filter(|m| !drop_root || msg.root_id.as_deref() != Some(m.message_id.as_str())),
        )
        .collect();
    if history.is_empty() {
        return None;
    }
    let quotes = resolve_history_quotes(adapter, &messages, &history).await;
    let mut blocks = vec![ContentBlock::Text {
        text: assemble_history(&history, &quotes),
    }];

    // Attach images behind `[image]`/`[post]` placeholders, capped at
    // the newest few; the backstopped root's images go last to survive.
    let mut pairs = image_pairs(
        history[usize::from(fetched_root.is_some())..]
            .iter()
            .copied(),
    );
    pairs.extend(image_pairs(
        history[..usize::from(fetched_root.is_some())]
            .iter()
            .copied(),
    ));
    if pairs.len() > IMAGE_DOWNLOAD_MAX {
        pairs.drain(..pairs.len() - IMAGE_DOWNLOAD_MAX);
    }
    blocks.extend(download_image_pairs(adapter, &pairs).await);
    Some(blocks)
}

/// Collect (`message_id`, `image_key`) pairs from history messages.
fn image_pairs<'m>(messages: impl Iterator<Item = &'m HistoryMessage>) -> Vec<(&'m str, &'m str)> {
    messages
        .flat_map(|m| {
            m.image_keys
                .iter()
                .map(move |k| (m.message_id.as_str(), k.as_str()))
        })
        .collect()
}

/// Download (`message_id`, `image_key`) pairs; failures are dropped (the
/// `[image]` text marker already records their presence).
async fn download_image_pairs(
    adapter: &Arc<dyn PlatformAdapter>,
    pairs: &[(&str, &str)],
) -> Vec<ContentBlock> {
    futures::future::join_all(
        pairs
            .iter()
            .map(|(message_id, key)| adapter.download_message_image(message_id, key)),
    )
    .await
    .into_iter()
    .flatten()
    .collect()
}

/// Backstop a still-`Pending` thread root missing from the fetched page
/// with a direct fetch (as the oldest history line); skipped when the
/// cursor covers it (a `/clear` deliberately forgot it).
async fn fetch_root_backstop(
    adapter: &Arc<dyn PlatformAdapter>,
    container: &HistoryContainer,
    msg: &ChannelMessage,
    root: RootDelivery,
    page: &[HistoryMessage],
    cursor: Option<i64>,
) -> Option<HistoryMessage> {
    if root != RootDelivery::Pending {
        return None;
    }
    let (HistoryContainer::Thread(_), Some(root_id)) = (container, &msg.root_id) else {
        return None;
    };
    if page.iter().any(|m| &m.message_id == root_id) {
        return None;
    }
    match adapter.fetch_message(root_id).await {
        Ok(Some(m)) if cursor.is_none_or(|c| m.create_time > c) => Some(m),
        Ok(_) => None,
        Err(e) => {
            warn!(error = %e, root_id, "root backstop fetch failed");
            None
        }
    }
}

/// Max images downloaded per triggering message or injected history
/// block, so an image dump can't blow up the context (or stall delivery
/// behind dozens of downloads). History keeps the newest; a message
/// keeps the first ones and gets a note for the rest.
const IMAGE_DOWNLOAD_MAX: usize = 5;

/// How a run trigger picks its session key and reply anchor.
enum TriggerKind {
    /// The channel's `reply_in_thread` rules (see [`session_mapping_key`]
    /// and [`reply_anchor`]), plus `/thread` adoption.
    Normal,
    /// `/thread`: key by and anchor to the trigger's own message id
    /// regardless of the group-scoped `reply_in_thread` rules, so the
    /// reply opens a thread in any chat. The command arm refuses
    /// triggers without a message id, so the key fallback here is
    /// defensive only.
    OneShotThread,
}

/// Resolve a run trigger's session and assemble its context blocks.
/// `root_in_session` = the mapping predates this trigger — mappings are
/// conversation-only (model commands degrade or fall back to the chat
/// session), so it means the thread's root is already in the session.
#[allow(clippy::too_many_arguments)]
async fn prepare_trigger(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    adapter: &Arc<dyn PlatformAdapter>,
    obs: &Arc<ObsTracker>,
    msg: &ChannelMessage,
    kind: TriggerKind,
) -> Result<(SessionId, Vec<ContentBlock>)> {
    let chat_id = msg.external_chat_id.clone();
    let (mapping_key, reply_msg_id) = match kind {
        TriggerKind::OneShotThread => {
            let id = msg.external_message_id.clone();
            (id.clone().unwrap_or_else(|| chat_id.clone()), id)
        }
        TriggerKind::Normal => (
            effective_mapping_key(store, channel_name, msg, &chat_id, config.reply_in_thread)
                .await?,
            reply_anchor(msg, config.reply_in_thread),
        ),
    };
    let (sid, root_in_session) = get_or_create_session(
        channel_name,
        store,
        kernel,
        &chat_id,
        &mapping_key,
        reply_msg_id.as_deref(),
    )
    .await?;
    record_receipt(config, obs, kernel, &sid, msg);
    let blocks = context_prefix(adapter, config, store, channel_name, msg, root_in_session).await;
    Ok((sid, blocks))
}

/// Assemble a trigger's context: recent-chat history first, then the
/// quoted message adjacent to the trigger (both best-effort).
/// `root_in_session` seeds the root's delivery state so a human-created
/// thread's root arrives exactly once — quoted first, history as
/// fallback (hence the quoted fetch runs before its block comes last).
async fn context_prefix(
    adapter: &Arc<dyn PlatformAdapter>,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    msg: &ChannelMessage,
    root_in_session: bool,
) -> Vec<ContentBlock> {
    let mut root = if root_in_session {
        RootDelivery::Consumed
    } else {
        RootDelivery::Pending
    };
    let quoted = maybe_quoted_prefix(adapter, msg, root).await;
    // A quoted chain containing the thread root flips the state.
    if quoted.as_ref().is_some_and(|(_, in_chain)| *in_chain) {
        root = RootDelivery::ByQuote;
    }
    // History first, quoted last — the quote belongs to the trigger.
    let mut blocks = maybe_history_prefix(adapter, config, store, channel_name, msg, root)
        .await
        .unwrap_or_default();
    blocks.extend(quoted.map(|(b, _)| b).unwrap_or_default());
    blocks
}

/// Assemble the quoted message as a `<quoted_message>` block plus
/// images, walking its own quote chain (cap [`QUOTE_CHAIN_MAX`]) so a
/// root that is itself a quote-reply keeps its context. In-thread
/// replies to an already-delivered root are skipped; on a fresh
/// (human-created) thread the root is injected like any other quote.
/// The bool reports whether any chain link IS the thread root (drives
/// history dedup). Best-effort: failures degrade to no/partial block.
async fn maybe_quoted_prefix(
    adapter: &Arc<dyn PlatformAdapter>,
    msg: &ChannelMessage,
    root: RootDelivery,
) -> Option<(Vec<ContentBlock>, bool)> {
    let parent_id = msg.parent_id.as_deref()?;
    if root != RootDelivery::Pending
        && msg.thread_id.is_some()
        && msg.root_id.as_deref() == Some(parent_id)
    {
        return None;
    }
    // Walk the quote chain: the quoted message first, then its own
    // quoted ancestors.
    let mut chain = Vec::new();
    let mut root_in_chain = false;
    let mut next = Some(parent_id.to_string());
    while let Some(id) = next.take() {
        if chain.len() >= QUOTE_CHAIN_MAX {
            break;
        }
        match adapter.fetch_message(&id).await {
            Ok(Some(m)) => {
                next.clone_from(&m.parent_id);
                root_in_chain |= msg.root_id.as_deref() == Some(m.message_id.as_str());
                chain.push(m);
            }
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, message_id = %id, "quoted message fetch failed");
                break;
            }
        }
    }
    if chain.is_empty() {
        return None;
    }
    // Ancestors first, the quoted message last — chronological reading.
    chain.reverse();
    let lines = chain.iter().map(sender_line).collect::<Vec<_>>().join("\n");
    let mut blocks = vec![ContentBlock::Text {
        text: format!("<quoted_message>\n{lines}\n</quoted_message>"),
    }];
    // The quoted message's own images win over its ancestors' under the
    // cap (chain is oldest-first; iterate newest-first here).
    let pairs = image_pairs(chain.iter().rev());
    blocks
        .extend(download_image_pairs(adapter, &pairs[..pairs.len().min(IMAGE_DOWNLOAD_MAX)]).await);
    Some((blocks, root_in_chain))
}

/// Max messages fetched for one quoted block: the quoted message plus
/// its quote ancestors (bounds latency on pathologically long chains).
const QUOTE_CHAIN_MAX: usize = 3;

/// `[HH:MM] sender: text` (local time, per-message capped) — the shared
/// line format for quoted and history context blocks.
fn sender_line(m: &HistoryMessage) -> String {
    let ts = chrono::DateTime::from_timestamp_millis(m.create_time)
        .map(|dt| dt.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default();
    let text = crate::utils::strs::truncate_by_chars(m.text.trim(), HISTORY_MESSAGE_MAX_CHARS, "…");
    format!("[{ts}] {}: {text}", m.sender_id)
}

/// Download a message's attached images (deferred until post-gate) and
/// append them to the content, capped at [`IMAGE_DOWNLOAD_MAX`]; a
/// failure degrades to a visible text placeholder instead of dropping
/// the message.
async fn append_message_images(
    adapter: &Arc<dyn PlatformAdapter>,
    message_id: &str,
    image_keys: &[String],
    content: &mut Vec<ContentBlock>,
) {
    let omitted = image_keys.len().saturating_sub(IMAGE_DOWNLOAD_MAX);
    let keys = &image_keys[..image_keys.len() - omitted];
    let results = futures::future::join_all(
        keys.iter()
            .map(|key| adapter.download_message_image(message_id, key)),
    )
    .await;
    for (key, result) in keys.iter().zip(results) {
        match result {
            Ok(block) => content.push(block),
            Err(e) => {
                warn!(error = %e, image_key = %key, "image download failed");
                content.push(ContentBlock::Text {
                    text: format!("[Failed to download image: {e}]"),
                });
            }
        }
    }
    if omitted > 0 {
        content.push(ContentBlock::Text {
            text: format!("[{omitted} more image(s) omitted]"),
        });
    }
}

/// Per-message cap in the injected history block (UTF-8 safe truncation).
const HISTORY_MESSAGE_MAX_CHARS: usize = 2000;

/// Format fetched messages as a context block: chronological, one line
/// each (`[HH:MM] open_id: text`, per-message capped), quote-replies
/// carrying an inline snippet of the quoted message (` ↩ sender: text`).
fn assemble_history(
    messages: &[&HistoryMessage],
    quotes: &std::collections::HashMap<String, String>,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::from("<recent_chat_history>\n");
    for m in messages {
        let _ = write!(out, "{}", sender_line(m));
        if let Some(q) = quotes.get(&m.message_id) {
            let _ = write!(out, " ↩ {q}");
        }
        let _ = writeln!(out);
    }
    out.push_str("</recent_chat_history>");
    out
}

/// Max quoted parents fetched per history block for inline snippets
/// (bounds extra latency; parents already in the fetched page are free).
const HISTORY_QUOTE_FETCH_MAX: usize = 3;

/// Per-quote snippet cap in history lines (quotes are secondary context).
const QUOTE_SNIPPET_MAX_CHARS: usize = 80;

/// `sender: text` for an inline quote snippet (whitespace-collapsed to
/// keep the one-line-per-message block shape).
fn quote_snippet(m: &HistoryMessage) -> String {
    let collapsed = m.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text = crate::utils::strs::truncate_by_chars(&collapsed, QUOTE_SNIPPET_MAX_CHARS, "…");
    format!("{}: {text}", m.sender_id)
}

/// Resolve quoted-message snippets for quote-replies in `history` (one
/// level only — history is background context). Parents already in the
/// fetched page are free; others are fetched directly, distinct parents
/// capped at [`HISTORY_QUOTE_FETCH_MAX`]. Keyed by history message id.
async fn resolve_history_quotes(
    adapter: &Arc<dyn PlatformAdapter>,
    page: &[HistoryMessage],
    history: &[&HistoryMessage],
) -> std::collections::HashMap<String, String> {
    let mut quotes = std::collections::HashMap::new();
    let mut fetched: std::collections::HashMap<&str, Option<String>> =
        std::collections::HashMap::new();
    for m in history {
        let Some(parent_id) = m.parent_id.as_deref() else {
            continue;
        };
        // Free lookups: the kept history (incl. a backstopped root) and
        // the fetched page (incl. a dropped root).
        let parent = history
            .iter()
            .find(|p| p.message_id == parent_id)
            .copied()
            .or_else(|| page.iter().find(|p| p.message_id == parent_id));
        if let Some(p) = parent {
            quotes.insert(m.message_id.clone(), quote_snippet(p));
            continue;
        }
        if fetched.len() >= HISTORY_QUOTE_FETCH_MAX && !fetched.contains_key(parent_id) {
            continue;
        }
        let snippet = match fetched.entry(parent_id) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let snippet = match adapter.fetch_message(parent_id).await {
                    Ok(Some(p)) => Some(quote_snippet(&p)),
                    Ok(None) => None,
                    Err(err) => {
                        warn!(error = %err, parent_id, "history quote fetch failed");
                        None
                    }
                };
                e.insert(snippet)
            }
        };
        if let Some(snippet) = snippet {
            quotes.insert(m.message_id.clone(), snippet.clone());
        }
    }
    quotes
}

/// Record a user message posted while the session's agent is running, for
/// the mid-run post detection (morph vs. new-message settle). Messages that
/// arrive while the session is idle are run triggers, not mid-run posts —
/// recording only while running also means a receipt can never outlive its
/// run into the next one (settlement clears them). Every accepted message
/// additionally refreshes the session's settle-reaction target (its latest
/// user message), so async runs without a fresh trigger still have one.
/// No-op when observability is disabled or the message carries no platform ID.
fn record_receipt(
    config: &ChannelConfig,
    obs: &ObsTracker,
    kernel: &Kernel,
    session_id: &SessionId,
    msg: &ChannelMessage,
) {
    if !config.observability {
        return;
    }
    if let Some(msg_id) = &msg.external_message_id {
        obs.record_user_msg(session_id, msg_id.clone());
    }
    if !kernel.is_session_running(session_id) {
        return;
    }
    if let Some(msg_id) = &msg.external_message_id {
        obs.record_receipt(session_id, msg_id.clone());
    }
}

/// Record a mid-run receipt for a message NOT addressed to the bot
/// (mention-missed group chatter). The message itself stays unprocessed
/// — no session is created, no reply, no reaction — but if it belongs to
/// a session whose agent is running, it counts as a mid-run post: the
/// user is still talking in that conversation, so the run's reply should
/// sink below their message instead of morphing above it. The
/// settle-reaction target is untouched (the bot never engaged with the
/// message), and commands are skipped — consistent with addressed
/// receipts, which record only on the Steer/Queue/plain-text routes.
async fn record_passive_receipt(
    channel_name: &str,
    config: &ChannelConfig,
    store: &Arc<dyn ChannelStore>,
    obs: &ObsTracker,
    msg: &ChannelMessage,
    is_running: impl Fn(&SessionId) -> bool,
) {
    if !config.observability {
        return;
    }
    let Some(msg_id) = msg.external_message_id.as_deref() else {
        return;
    };
    if !matches!(
        parse_channel_command(msg.raw_text.as_deref()),
        ChannelCommand::None
    ) {
        return;
    }
    let Ok(mapping_key) = effective_mapping_key(
        store,
        channel_name,
        msg,
        &msg.external_chat_id,
        config.reply_in_thread,
    )
    .await
    else {
        return;
    };
    // A top-level group message in reply_in_thread mode keys by its own
    // id — never mapped, never a mid-run post (it doesn't interleave
    // with any thread's run).
    let Ok(Some(sid)) = store.find_mapping(channel_name, &mapping_key).await else {
        return;
    };
    if !is_running(&sid) {
        return;
    }
    obs.record_receipt(&sid, msg_id.to_string());
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
/// inside the thread carries one and replies to the thread's root message
/// (Feishu sets `root_id` to it). Keying in-thread messages by root id and
/// everything else by its own message id therefore keeps a whole thread in
/// one session while each new top-level message starts a fresh session.
///
/// A plain quote-reply (not in any thread) also carries `root_id` — it must
/// NOT join the quoted message's session: it starts its own, and the bot's
/// `reply_in_thread` answer opens a fresh thread anchored at it.
fn session_mapping_key(msg: &ChannelMessage, chat_id: &str, reply_in_thread: bool) -> String {
    if reply_in_thread && msg.is_group {
        if msg.thread_id.is_some() {
            // Inside a thread: key by the thread's root message so the whole
            // thread shares one session (fall back to thread_id for older
            // event shapes without root_id).
            msg.root_id
                .clone()
                .or_else(|| msg.thread_id.clone())
                .unwrap_or_else(|| chat_id.to_string())
        } else {
            // Top-level trigger or plain quote-reply: a fresh session keyed
            // by the message itself.
            msg.external_message_id
                .clone()
                .unwrap_or_else(|| chat_id.to_string())
        }
    } else {
        msg.thread_id.clone().unwrap_or_else(|| chat_id.to_string())
    }
}

/// The session mapping key with `/thread` adoption: a thread opened by
/// the one-shot command keeps its session under the thread's root
/// message id (the trigger's own key under the forced flag, see
/// [`session_mapping_key`]); follow-ups in such a thread adopt that
/// session instead of starting a fresh thread-id-keyed one. With
/// `reply_in_thread` on, in-thread keying already roots at the same
/// id — nothing to adopt.
async fn effective_mapping_key(
    store: &Arc<dyn ChannelStore>,
    channel_name: &str,
    msg: &ChannelMessage,
    chat_id: &str,
    reply_in_thread: bool,
) -> Result<String> {
    let key = session_mapping_key(msg, chat_id, reply_in_thread);
    if reply_in_thread || msg.thread_id.is_none() {
        return Ok(key);
    }
    let Some(root_id) = msg.root_id.as_deref() else {
        return Ok(key);
    };
    if store.find_mapping(channel_name, &key).await?.is_none()
        && store.find_mapping(channel_name, root_id).await?.is_some()
    {
        return Ok(root_id.to_string());
    }
    Ok(key)
}

/// Whether a message is a top-level group message in `reply_in_thread`
/// mode (i.e. not inside any thread). Such messages address the chat as a
/// whole — e.g. a top-level `/model` switches every thread session, and a
/// top-level `/info` shows the chat-level session.
fn is_chat_level_message(msg: &ChannelMessage, reply_in_thread: bool) -> bool {
    reply_in_thread && msg.is_group && msg.thread_id.is_none() && msg.root_id.is_none()
}

/// Get an existing session or create a new one, updating routing info.
/// The bool reports whether an existing mapping was reused — context-
/// injecting callers read it as "the thread's root is already consumed"
/// (thread mappings are conversation-only, see [`prepare_trigger`]).
async fn get_or_create_session(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    chat_id: &str,
    mapping_key: &str,
    reply_msg_id: Option<&str>,
) -> Result<(SessionId, bool)> {
    if let Some(sid) = store.find_mapping(channel_name, mapping_key).await? {
        info!(channel = %channel_name, mapping_key, session_id = %sid.0, "reusing session");
        store
            .save_mapping(channel_name, mapping_key, &sid, chat_id, reply_msg_id)
            .await?;
        return Ok((sid, true));
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
    info!(channel = %channel_name, mapping_key, session_id = %sid.0, "created session");
    Ok((sid, false))
}

/// The session's model key, or what a fresh session would resolve to
/// (threads inherit the chat session's explicit choice). Read-only.
async fn session_model_key(
    channel_name: &str,
    store: &Arc<dyn ChannelStore>,
    kernel: &Kernel,
    chat_id: &str,
    mapping_key: &str,
) -> Result<String> {
    if let Some(sid) = store.find_mapping(channel_name, mapping_key).await? {
        return Ok(kernel.get_session_model(&sid).await);
    }
    Ok(model_key_for_new_channel_session(
        channel_name,
        chat_id,
        mapping_key,
        store,
        &kernel.session_store().await,
    )
    .await?
    .unwrap_or_else(|| kernel.default_model_key()))
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
const CMD_COMPACT: &str = "/compact";
const CMD_STOP: &str = "/stop";
const CMD_STEER: &str = "/steer";
const CMD_QUEUE: &str = "/queue";
const CMD_INFO: &str = "/info";
const CMD_HELP: &str = "/help";
const CMD_PERMITS: &str = "/permits";
const CMD_APPROVE: &str = "/approve";
const CMD_DENY: &str = "/deny";
const CMD_RESTART: &str = "/restart";
const CMD_THREAD: &str = "/thread";

/// All channel command prefixes, longest-first so `/models` is matched
/// before `/model` (the latter is a prefix of the former).
const CMD_PREFIXES: &[&str] = &[
    CMD_MODELS,
    CMD_MODEL,
    CMD_CLEAR,
    CMD_COMPACT,
    CMD_STOP,
    CMD_STEER,
    CMD_QUEUE,
    CMD_INFO,
    CMD_HELP,
    CMD_PERMITS,
    CMD_APPROVE,
    CMD_DENY,
    CMD_RESTART,
    CMD_THREAD,
];

/// `/help` response: the channel command list.
const HELP_TEXT: &str = "\
**Commands**
`/help` — this help
`/info` — current session info
`/models` — list configured models (current one marked)
`/model` — show current model; `/model <key>` to switch
`/clear` — clear context and start fresh
`/compact` — summarize and compact the context
`/stop` — stop the current run
`/steer <text>` — inject a message into the current run
`/queue <text>` — queue a message for a later turn
`/thread <text>` — ask in a new thread opened off this message (Feishu)
`/permits` — list pending doc-permission requests (admin)
`/approve <id> [perm]` — approve a doc-permission request (admin)
`/deny <id>` — deny a doc-permission request (admin)
`/restart` — restart the daemon (admin)

Anything else is sent to the agent as a message.";

/// Parsed channel command from an incoming message.
enum ChannelCommand {
    /// Clear context and start fresh.
    Clear,
    /// Summarize and compact the session context.
    Compact,
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
    /// Show the command list.
    Help,
    /// List pending doc-permission applications (admin only).
    Permits,
    /// Approve a doc-permission application, optionally overriding the level.
    Approve { id: i64, perm: Option<String> },
    /// Deny a doc-permission application.
    Deny { id: i64 },
    /// An approval command with missing or malformed arguments.
    InvalidApprovalCommand,
    /// Restart the daemon (admin only).
    Restart,
    /// One-shot: run this trigger with the reply anchored to the
    /// command message, opening a new thread off it.
    Thread(String),
    /// A `/thread` command without text.
    InvalidThreadCommand,
    /// Not a command.
    None,
}

/// Whether a command settles everything before it — run triggers by
/// consuming context, `/clear` by discarding it.
fn consumes_history(cmd: &ChannelCommand) -> bool {
    matches!(
        cmd,
        ChannelCommand::None
            | ChannelCommand::Steer(_)
            | ChannelCommand::Queue(_)
            | ChannelCommand::Thread(_)
            | ChannelCommand::Clear
    )
}

fn parse_channel_command(raw_text: Option<&str>) -> ChannelCommand {
    let Some(text) = raw_text.map(str::trim).filter(|text| !text.is_empty()) else {
        return ChannelCommand::None;
    };
    let mut parts = text.split_whitespace();
    let Some(cmd) = parts.next() else {
        return ChannelCommand::None;
    };

    let Some(&command) = CMD_PREFIXES.iter().find(|prefix| cmd_matches(cmd, prefix)) else {
        return ChannelCommand::None;
    };

    match command {
        CMD_CLEAR if parts.next().is_none() => ChannelCommand::Clear,
        CMD_COMPACT if parts.next().is_none() => ChannelCommand::Compact,
        CMD_STOP if parts.next().is_none() => ChannelCommand::Stop,
        CMD_INFO if parts.next().is_none() => ChannelCommand::Info,
        CMD_HELP if parts.next().is_none() => ChannelCommand::Help,
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
        CMD_PERMITS if parts.next().is_none() => ChannelCommand::Permits,
        CMD_APPROVE => match (parts.next(), parts.next()) {
            (Some(id), extra) if extra.is_none() || parts.next().is_none() => {
                match id.parse::<i64>() {
                    Ok(id) => ChannelCommand::Approve {
                        id,
                        perm: extra.map(str::to_string),
                    },
                    Err(_) => ChannelCommand::InvalidApprovalCommand,
                }
            }
            _ => ChannelCommand::InvalidApprovalCommand,
        },
        CMD_DENY => match (parts.next(), parts.next()) {
            (Some(id), None) => match id.parse::<i64>() {
                Ok(id) => ChannelCommand::Deny { id },
                Err(_) => ChannelCommand::InvalidApprovalCommand,
            },
            _ => ChannelCommand::InvalidApprovalCommand,
        },
        CMD_RESTART if parts.next().is_none() => ChannelCommand::Restart,
        CMD_THREAD => {
            let rest = parts.collect::<Vec<_>>().join(" ");
            if rest.is_empty() {
                ChannelCommand::InvalidThreadCommand
            } else {
                ChannelCommand::Thread(rest)
            }
        }
        _ => ChannelCommand::None,
    }
}

/// A command token matches a prefix exactly or with an `@bot` suffix
/// (`/clear`, `/clear@yomi_bot`) — never a longer word (`/clearance` is
/// not a command).
fn cmd_matches(cmd: &str, prefix: &str) -> bool {
    cmd == prefix
        || cmd
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('@'))
}

pub(super) fn has_channel_command_prefix(raw_text: &str) -> bool {
    let command = raw_text.split_whitespace().next().unwrap_or_default();
    CMD_PREFIXES
        .iter()
        .any(|prefix| cmd_matches(command, prefix))
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
    kv: Option<Arc<crate::kv_cache::KvCache>>,
) -> Arc<dyn PlatformAdapter> {
    match platform {
        super::PlatformConfig::Telegram { token } => {
            Arc::new(super::telegram::TelegramAdapter::new(token.clone()))
        }
        super::PlatformConfig::Feishu { app_id, app_secret } => {
            let mut adapter = super::feishu::FeishuAdapter::new(app_id.clone(), app_secret.clone());
            adapter.set_kv_cache(kv);
            Arc::new(adapter)
        }
    }
}

#[cfg(test)]
#[path = "hub_test.rs"]
mod tests;
