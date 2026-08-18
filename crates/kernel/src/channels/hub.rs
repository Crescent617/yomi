use crate::event::{AgentEvent, AgentStatus, Event, ModelEvent, ToolEvent};
use crate::kernel::Kernel;

use crate::types::{ContentBlock, Result, SessionId};
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::hub_context::{
    advance_history_cursor, prepare_trigger, record_passive_receipt, TriggerKind,
};
use super::hub_deliver::{
    deliver_reply, notify_run_subscribers, send_command_reply, RunEndStatus, SettleKind,
};
use super::hub_gate::{gate_message, send_gate_reaction, Gate};
use super::hub_handlers::handle_incoming_message;
use super::hub_routing::{reply_anchor, resolve_reply_in_thread};

use super::{
    obs::ObsTracker, reply, ChannelConfig, ChannelEvent, ChannelInfo, ChannelMessage,
    ChannelStatus, ChannelStore, PlatformAdapter, SessionRouting,
};

const STATUS_IDLE: u8 = 0;

const STATUS_CONNECTING: u8 = 1;

const STATUS_ERROR: u8 = 3;

/// Watchdog sweep interval for dead-session status cards.
const WATCHDOG_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

/// Heartbeat interval for refreshing live status cards. Long tool calls
/// emit no events, so event-driven PATCHes stop and the card looks frozen
/// (elapsed stuck at the last patch) — this keeps it visibly alive.
const LIVE_CARD_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

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
        for event in &config.disabled_events {
            if !config
                .platform
                .known_event_names()
                .contains(&event.as_str())
            {
                warn!(
                    channel = %name,
                    event = %event,
                    "disabled_events entry not recognized for this platform"
                );
            }
        }

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

        let adapter_gate = Arc::clone(&adapter);
        let name_gate = name.clone();
        let config_gate = config.clone();

        // Gated messages awaiting serial dispatch (session routing,
        // history backfill, receipts) — bounded like the incoming queue.
        let (dispatch_tx, dispatch_rx) = mpsc::channel::<(ChannelMessage, Gate)>(256);
        let adapter_dispatch = Arc::clone(&adapter);
        let name_dispatch = name.clone();
        let config_dispatch = config.clone();
        let store_dispatch = Arc::clone(&self.store);
        let obs_dispatch = Arc::clone(&self.obs);
        let kernel_dispatch = kernel.clone();
        let cancel_dispatch = sub_cancel.clone();

        // Spawn the gate loop: cheap per-message decisions only (access
        // control, mention check) with the reaction fired off-loop, so
        // slow dispatch work (history fetches, image downloads) can't
        // delay the ack reaction of whatever queued up behind it — up to
        // the dispatch queue's capacity, past which backpressure stalls
        // the gate by design. A /mention toggle still in dispatch can be
        // gated under the stale override; the window is milliseconds and
        // self-corrects on the next message.
        let gate_handle = tokio::spawn(async move {
            let mut incoming_rx = incoming_rx;
            loop {
                tokio::select! {
                    biased;
                    () = sub_cancel.cancelled() => {
                        info!(channel = %name_gate, "gate loop cancelled");
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
                                    name_gate.clone(),
                                    config_gate.clone(),
                                    Arc::clone(&store),
                                    Arc::clone(&adapter_gate),
                                );
                                tokio::spawn(async move {
                                    super::approval::handle_doc_permission_applied(
                                        &name, &config, &store, &adapter, req,
                                    ).await;
                                });
                                continue;
                            }
                            ChannelEvent::CardAction(action) => {
                                let (name, config, store, adapter, kernel_weak) = (
                                    name_gate.clone(),
                                    config_gate.clone(),
                                    Arc::clone(&store),
                                    Arc::clone(&adapter_gate),
                                    kernel.clone(),
                                );
                                tokio::spawn(async move {
                                    // Card-action gate: button clicks
                                    // bypass the message gate, so every
                                    // action first re-applies its user
                                    // rule (blocked / allowed_users).
                                    // Admin-gated surfaces (mb_*, doc
                                    // approvals) stack check_admin in
                                    // their own handlers.
                                    if let Some(deny) = super::check_user_access(
                                        &config,
                                        &action.operator_open_id,
                                    ) {
                                        super::approval::send_action_denial(
                                            &adapter, &action, deny,
                                        )
                                        .await;
                                        return;
                                    }
                                    // 按钮命名空间路由：mb_* 归 mailbox
                                    // 管理面，act_* 归状态卡动作（stop
                                    // 等），其余归权限审批。
                                    let ns = action.value["action"].as_str().unwrap_or_default();
                                    if ns.starts_with("mb_") {
                                        let Some(kernel) = kernel_weak.upgrade() else {
                                            return;
                                        };
                                        super::mailbox::handle_card_action(
                                            &name, &config, &kernel, &adapter, action,
                                        )
                                        .await;
                                    } else if ns.starts_with("act_") {
                                        let Some(kernel) = kernel_weak.upgrade() else {
                                            return;
                                        };
                                        super::obs::handle_stop_action(&kernel, &action);
                                    } else {
                                        super::approval::handle_card_action(
                                            &name, &config, &store, &adapter, action,
                                        )
                                        .await;
                                    }
                                });
                                continue;
                            }
                            // Doc comments: policy + content fetch run
                            // off-loop (spawned); the accepted trigger
                            // enters the serial dispatch path like any
                            // chat message.
                            ChannelEvent::DocCommentAdded(notice) => {
                                let (name, config, store, adapter, dispatch) = (
                                    name_gate.clone(),
                                    config_gate.clone(),
                                    Arc::clone(&store),
                                    Arc::clone(&adapter_gate),
                                    dispatch_tx.clone(),
                                );
                                tokio::spawn(async move {
                                    super::comment::handle_doc_comment_added(
                                        &name, &config, &store, &adapter, &dispatch, notice,
                                    ).await;
                                });
                                continue;
                            }
                        };
                        // Route to kernel
                        if kernel.upgrade().is_none() {
                            warn!("kernel gone, stopping gate loop");
                            break;
                        }
                        let (gate, reaction) = gate_message(&config_gate, &store, &msg).await;
                        // Fire-and-forget: a slow reactions API must not
                        // stall the gate. Silent messages (no reaction
                        // decided) skip the spawn entirely.
                        if reaction.is_some() {
                            let adapter = Arc::clone(&adapter_gate);
                            let config = config_gate.clone();
                            let react_msg = msg.clone();
                            tokio::spawn(async move {
                                send_gate_reaction(&adapter, &config, &react_msg, reaction).await;
                            });
                        }
                        if gate == Gate::Denied {
                            continue;
                        }
                        // Allow / NotAddressed: stateful handling stays
                        // serial, in arrival order, behind the gate.
                        if dispatch_tx.send((msg, gate)).await.is_err() {
                            break;
                        }
                    }
                    else => {
                        info!(channel = %name_gate, "incoming channel closed, exiting");
                        break;
                    }
                }
            }
        });

        // Spawn the dispatch loop: the serial heavy worker. Receipts,
        // session routing, history backfill and the cursor advance run
        // here in arrival order — exactly what the gate loop's predecessor
        // did after gating, just no longer in the reaction's way.
        let dispatch_handle = tokio::spawn(async move {
            let mut dispatch_rx = dispatch_rx;
            loop {
                tokio::select! {
                    biased;
                    () = cancel_dispatch.cancelled() => {
                        info!(channel = %name_dispatch, "dispatch loop cancelled");
                        break;
                    }
                    Some((msg, gate)) = dispatch_rx.recv() => {
                        let Some(coord) = kernel_dispatch.upgrade() else {
                            warn!("kernel gone, stopping dispatch loop");
                            break;
                        };
                        // Non-addressed chatter still counts as a mid-run
                        // post when it lands in a running session's
                        // conversation.
                        if gate == Gate::NotAddressed {
                            record_passive_receipt(
                                &name_dispatch,
                                &config_dispatch,
                                &store_dispatch,
                                &obs_dispatch,
                                &msg,
                                |sid| coord.is_session_running(sid),
                            )
                            .await;
                            continue;
                        }
                        let handled = handle_incoming_message(
                            &name_dispatch,
                            &config_dispatch,
                            &store_dispatch,
                            coord,
                            msg.clone(),
                            &obs_dispatch,
                            &adapter_dispatch,
                        ).await;
                        // Advance the cursor only after a successfully
                        // handled message; a failed trigger consumed
                        // nothing (a history fetch failing mid-handle
                        // still skips its window — best-effort).
                        if handled.is_ok() {
                            advance_history_cursor(
                                &config_dispatch,
                                &store_dispatch,
                                &name_dispatch,
                                &msg,
                            )
                            .await;
                        }
                        match handled {
                            Ok(Some(reply_text)) => {
                                let rit = resolve_reply_in_thread(
                                    &store_dispatch,
                                    &config_dispatch,
                                    &msg.external_chat_id,
                                )
                                .await;
                                let reply_msg_id = reply_anchor(&msg, rit);
                                let adapter = Arc::clone(&adapter_dispatch);
                                tokio::spawn(async move {
                                    if let Err(e) = send_command_reply(
                                        &adapter,
                                        &msg,
                                        reply_msg_id,
                                        reply_text,
                                    )
                                    .await
                                    {
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
                        info!(channel = %name_dispatch, "dispatch channel closed, exiting");
                        break;
                    }
                }
            }
        });

        let name_done = name.clone();
        let _handle = tokio::spawn(async move {
            let _ = recv_handle.await;
            let _ = gate_handle.await;
            let _ = dispatch_handle.await;
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
            let mut live_refresh = tokio::time::interval(LIVE_CARD_REFRESH_INTERVAL);
            live_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

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
                    // Ticks yield to queued events (guard disables the arm
                    // under `biased`): a terminal event (`Stopped`/
                    // `Compacted`) flips the conductor's state mirror to
                    // Idle the instant it is emitted, but this loop may not
                    // have processed it yet — judging liveness before
                    // draining the queue sweeps live cards into false
                    // ⏰ Session lost receipts. Guards are evaluated once at
                    // `select!` entry, so a tick waking the loop can still
                    // win over events that landed while parked — hence the
                    // re-check inside each handler. Sustained event flow
                    // defers ticks (MissedTickBehavior::Skip, no burst);
                    // they catch up when traffic quiets.
                    _ = live_refresh.tick(), if rx.is_empty() => {
                        if !rx.is_empty() {
                            continue;
                        }
                        // Live-card heartbeat: re-render + PATCH cards that
                        // haven't updated within the interval (long tool).
                        obs.refresh_stale(LIVE_CARD_REFRESH_INTERVAL).await;
                    }
                    _ = watchdog.tick(), if rx.is_empty() => {
                        if !rx.is_empty() {
                            continue;
                        }
                        // Kernel gone = shutting down; nothing to settle.
                        if let Some(k) = kernel.upgrade() {
                            // Sessions whose agent died (crash / lost
                            // `Stopped`): flush whatever reply state remains
                            // so content is never silently lost, mirroring the
                            // obs timeout settlement. Tick arms yield to
                            // queued events (guard + in-handler re-check),
                            // so an already-delivered `Stopped` is always
                            // drained before this check; the residual window
                            // is the bus-forwarder hop (sub-ms scheduling
                            // latency) before an event reaches this listener.
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
                                let reply_msg_id = deliver_reply(
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
                                // The reply was still delivered — subscribers
                                // get their card (agent died mid-run, so the
                                // status says so).
                                notify_run_subscribers(
                                    &store,
                                    &adapter,
                                    &routing,
                                    reply_msg_id.as_deref(),
                                    RunEndStatus::Failed,
                                    &sid,
                                    &kernel,
                                    &obs,
                                )
                                .await;
                            }
                            obs.sweep_dead_sessions(|sid| k.is_session_running(sid)).await;
                        }
                    }
                    Some((session_id, envelope)) = rx.recv() => {
                        // Forwarder liveness breadcrumb (trace level): the
                        // task processes every event on one loop — if it
                        // ever hangs/panics, all channel replies silently
                        // stop, and this is the only tell.
                        tracing::trace!(
                            session_id = %session_id.0,
                            event = ?std::mem::discriminant(&envelope.event),
                            "channel forwarder event"
                        );
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
                        // Doc-comment sessions have no chat surface: no
                        // status cards, no typing — only the final reply
                        // (deliver_reply's doc-comment branch).
                        let is_doc_comment = routing.doc_comment.is_some();

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
                                // First Running of the run: put the session's
                                // model on the status card (one store read
                                // per run; a gone kernel just skips it).
                                if observability && !is_doc_comment && !obs.has_state(&session_id) {
                                    if let Some(k) = kernel.upgrade() {
                                        let model = k.get_session_model(&session_id).await;
                                        obs.set_model(&session_id, model.clone());
                                        // The reply buffer renders the same
                                        // title on the settled reply card.
                                        if let Some(buf) = reply_buffers.get_mut(&session_id) {
                                            buf.set_model(model);
                                        }
                                    }
                                }
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
                            Event::Model(ModelEvent::TokenUsage {
                                message_id,
                                prompt_tokens,
                                completion_tokens,
                                total_tokens,
                                context_window,
                                ..
                            }) => {
                                // Real usage rides the settled reply card's
                                // trace title, same segment as the live card.
                                if let Some(buf) = reply_buffers.get_mut(&session_id) {
                                    buf.set_ctx_footer(*total_tokens, *context_window);
                                    buf.add_usage(message_id, *prompt_tokens, *completion_tokens);
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
                        // when the user posted mid-run (`mid_run_split`).
                        if let Event::Agent(AgentEvent::Lifecycle {
                            state: AgentStatus::Stopped { reason },
                        }) = &envelope.event
                        {
                            let reply = reply_buffers
                                .remove(&session_id)
                                .map(reply::RunReplyBuffer::into_reply);
                            let reply_msg_id = deliver_reply(
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
                            notify_run_subscribers(
                                &store,
                                &adapter,
                                &routing,
                                reply_msg_id.as_deref(),
                                RunEndStatus::from_stop_reason(reason),
                                &session_id,
                                &kernel,
                                &obs,
                            )
                            .await;
                            continue;
                        }

                        // Observability: cheap state updates + throttled
                        // in-place PATCHes (design: feishu-channel-observability).
                        if observability && !is_doc_comment {
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
                            && !is_doc_comment
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

    /// `yomi channel new-thread`: post an anchor message in `chat_id` and
    /// fire a `/thread`-style one-shot trigger off it — the task runs in a
    /// fresh session keyed by the anchor, and the run's replies open the
    /// platform thread under it. `title` sets a short root text and posts
    /// the task as the thread's opener (default: the task text is the
    /// root). Returns the session id, anchor id and jump link.
    pub async fn create_thread_in_chat(
        &self,
        kernel: &Kernel,
        channel: Option<&str>,
        platform: &str,
        chat_id: &str,
        title: Option<&str>,
        text: &str,
    ) -> Result<serde_json::Value> {
        // Resolve the channel instance: explicit name, or the sole
        // channel of the requested platform.
        let (name, config, adapter) = match channel {
            Some(name) => {
                let entry = self.instances.get(name).ok_or_else(|| {
                    crate::types::KernelError::Config(format!("no such channel: `{name}`"))
                })?;
                (
                    name.to_string(),
                    entry.config.clone(),
                    Arc::clone(&entry.adapter),
                )
            }
            None => {
                let matches: Vec<_> = self
                    .instances
                    .iter()
                    .filter(|e| e.config.platform.name_is(platform))
                    .map(|e| (e.key().clone(), e.config.clone(), Arc::clone(&e.adapter)))
                    .collect();
                match matches.as_slice() {
                    [] => {
                        return Err(crate::types::KernelError::Config(format!(
                            "no {platform} channel is running"
                        )));
                    }
                    [one] => one.clone(),
                    _ => {
                        return Err(crate::types::KernelError::Config(format!(
                            "multiple {platform} channels are running — pass --channel"
                        )));
                    }
                }
            }
        };
        // Threads are a Feishu capability for now.
        if !matches!(config.platform, super::PlatformConfig::Feishu { .. }) {
            return Err(crate::types::KernelError::Config(format!(
                "channel `{name}` does not support threads"
            )));
        }

        // 1. The anchor (thread root): the task text, or the short title.
        let root_id = adapter
            .send_message(
                chat_id,
                vec![ContentBlock::Text {
                    text: title.unwrap_or(text).to_string(),
                }],
                None,
            )
            .await?
            .ok_or_else(|| {
                crate::types::KernelError::Io("platform returned no anchor message id".into())
            })?;
        // 2. With an explicit title, the task itself opens the thread.
        if title.is_some() {
            adapter
                .send_message(
                    chat_id,
                    vec![ContentBlock::Text {
                        text: text.to_string(),
                    }],
                    Some(&root_id),
                )
                .await?;
        }

        // 3. A synthetic top-level trigger keyed by (and anchored to) the
        // anchor message — the `/thread` flow minus its human message.
        let msg = ChannelMessage {
            external_chat_id: chat_id.to_string(),
            external_user_id: "yomi-cli".to_string(),
            external_message_id: Some(root_id.clone()),
            is_mention: true,
            raw_text: Some(text.to_string()),
            content: vec![],
            image_keys: vec![],
            thread_id: None,
            root_id: None,
            parent_id: None,
            is_group: true,
            create_time: Some(chrono::Utc::now().timestamp_millis()),
            doc_comment: None,
        };
        send_gate_reaction(
            &adapter,
            &config,
            &msg,
            Some(config.platform.ack_reaction()),
        )
        .await;
        let (sid, mut blocks) = prepare_trigger(
            &name,
            &config,
            &self.store,
            kernel,
            &adapter,
            &self.obs,
            &msg,
            TriggerKind::OneShotThread,
        )
        .await?;
        kernel.note_title_input(&sid, text);
        blocks.push(ContentBlock::Text {
            text: text.to_string(),
        });
        kernel.send_steer(&sid, blocks).await;

        Ok(serde_json::json!({
            "session_id": sid.0,
            "channel": name,
            "chat_id": chat_id,
            "root_id": root_id,
            // The thread opens with the run's first in-thread reply, so a
            // thread link can't exist yet — the anchor's message link is
            // where the thread will appear (Feishu backfills thread_id on
            // the root then).
            "thread_url": adapter.message_link(chat_id, &root_id).await,
        }))
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
