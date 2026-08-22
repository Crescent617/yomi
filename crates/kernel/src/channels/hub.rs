use crate::event::{Event, ModelEvent};
use crate::kernel::Kernel;

use crate::types::{ContentBlock, Result, SessionId};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use super::delivery_pool::{DeliveryJob, DeliveryPool};
use super::hub_context::{
    advance_history_cursor, prepare_trigger, record_passive_receipt, TriggerKind,
};
use super::hub_deliver::send_command_reply;
use super::hub_gate::{gate_message, send_gate_reaction, Gate};
use super::hub_handlers::handle_incoming_message;
use super::hub_routing::{reply_anchor, resolve_reply_in_thread};

use super::{
    ask::AskCardRegistry, obs::ObsTracker, ChannelConfig, ChannelEvent, ChannelInfo,
    ChannelMessage, ChannelStatus, ChannelStore, PlatformAdapter, SessionRouting,
};

const STATUS_IDLE: u8 = 0;

const STATUS_CONNECTING: u8 = 1;

const STATUS_ERROR: u8 = 3;

/// Watchdog sweep interval for dead-session status cards.
const WATCHDOG_SWEEP_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

/// The forwarder's bus queue: sized for bursts (the default listener
/// capacity is 256). Platform I/O runs on per-session workers, but a
/// flood can still outpace this bookkeeping loop — headroom converts
/// silent drops into queued latency.
const FORWARDER_QUEUE_CAPACITY: usize = 4096;

/// Routing-gate cache retention (positive entries; negatives use a much
/// shorter TTL inline in `routing_for`). Also the eviction granularity —
/// stale entries are dropped on the watchdog tick.
const ROUTING_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// 心跳 PATCH 在飞标记的复位 guard：panic/取消等任何退出路径都不会
/// 把标记卡在 true 让心跳永久停摆（评审复核 #6）。
struct ResetOnDrop(Arc<std::sync::atomic::AtomicBool>);

impl Drop for ResetOnDrop {
    fn drop(&mut self) {
        self.0.store(false, std::sync::atomic::Ordering::Release);
    }
}

/// Heartbeat interval for refreshing live status cards. Long tool calls
/// emit no events, so event-driven PATCHes stop and the card looks frozen
/// (elapsed stuck at the last patch) — this keeps it visibly alive.
const LIVE_CARD_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// A running channel instance.
pub(crate) struct ChannelInstance {
    pub(crate) config: ChannelConfig,
    status: Arc<AtomicU8>,
    pub(crate) adapter: Arc<dyn PlatformAdapter>,
}

impl ChannelInstance {
    /// 测试用构造（`status` 字段刻意保持私有，测试走这里）。
    #[cfg(test)]
    pub(crate) fn test_instance(config: ChannelConfig, adapter: Arc<dyn PlatformAdapter>) -> Self {
        Self {
            config,
            status: Arc::new(AtomicU8::new(STATUS_IDLE)),
            adapter,
        }
    }
}

/// Manages the lifecycle of all platform channels and routes incoming
/// messages to the kernel.
pub struct ChannelHub {
    store: Arc<dyn ChannelStore>,
    instances: Arc<DashMap<String, ChannelInstance>>,
    obs: Arc<ObsTracker>,
    ask: Arc<AskCardRegistry>,
}

impl ChannelHub {
    pub fn new(store: Arc<dyn ChannelStore>) -> Self {
        Self {
            store,
            instances: Arc::new(DashMap::new()),
            obs: Arc::new(ObsTracker::new()),
            ask: Arc::new(AskCardRegistry::new()),
        }
    }

    /// Channel mapping store（ext_route 的 pseudo-channel 映射复用）。
    pub fn store(&self) -> Arc<dyn ChannelStore> {
        Arc::clone(&self.store)
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

        let adapter = build_adapter(&config.platform);
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
        let ask_gate = Arc::clone(&self.ask);

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
                                let (name, config, store, adapter, kernel_weak, ask_reg) = (
                                    name_gate.clone(),
                                    config_gate.clone(),
                                    Arc::clone(&store),
                                    Arc::clone(&adapter_gate),
                                    kernel.clone(),
                                    Arc::clone(&ask_gate),
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
                                    // 管理面，act_*/bg_*/pg_*/ask_* 各归
                                    // 其动作面，cfg_* 归设置面板，其余
                                    // 归权限审批。
                                    let ns = action.value["action"].as_str().unwrap_or_default();
                                    if ns.starts_with("ask_") {
                                        let Some(kernel) = kernel_weak.upgrade() else {
                                            return;
                                        };
                                        ask_reg.handle_answer(&kernel, &adapter, &action).await;
                                    } else if ns.starts_with("mb_") {
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
                                    } else if ns.starts_with("bg_") {
                                        let Some(kernel) = kernel_weak.upgrade() else {
                                            return;
                                        };
                                        super::hub_handlers::handle_bg_action(
                                            &name, &kernel, &adapter, &action,
                                        )
                                        .await;
                                    } else if ns.starts_with("pg_") {
                                        let Some(kernel) = kernel_weak.upgrade() else {
                                            return;
                                        };
                                        super::hub_handlers::handle_sessions_action(
                                            &name, &config, &store, &kernel, &adapter, &action,
                                        )
                                        .await;
                                    } else if ns.starts_with("cfg_") {
                                        let Some(kernel) = kernel_weak.upgrade() else {
                                            return;
                                        };
                                        super::settings::handle_card_action(
                                            &name, &config, &kernel, &store, &adapter, action,
                                        )
                                        .await;
                                    } else if ns.starts_with("cron_") {
                                        let Some(kernel) = kernel_weak.upgrade() else {
                                            return;
                                        };
                                        super::cron_card::handle_card_action(
                                            &name, &config, &kernel, &adapter, action,
                                        )
                                        .await;
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
        let ask = Arc::clone(&self.ask);

        tokio::spawn(async move {
            // `ToolCallDelta` floods (e.g. thousands of argument deltas for a
            // large file write) would overflow this listener's queue while
            // the loop is busy, silently dropping text `Chunk`/`End` events
            // (bus delivery is try_send). The forwarder never consumes
            // deltas, so filter them out at the source.
            let mut rx = event_bus.subscribe_all_filtered_with_capacity(
                FORWARDER_QUEUE_CAPACITY,
                |envelope| {
                    !matches!(
                        envelope.event,
                        Event::Model(ModelEvent::ToolCallDelta { .. })
                    )
                },
            );
            // 平台 IO 与投递状态全部收进每会话 actor（delivery_pool）；
            // 本循环只剩分派与全局 tick，不做任何网络调用——2026-08-21
            // 洪峰事故的根修。
            let pool = DeliveryPool::new(
                Arc::clone(&obs),
                Arc::clone(&ask),
                Arc::clone(&store),
                Arc::clone(&instances),
                kernel.clone(),
                token.clone(),
            );
            // 路由门禁：无路由的会话（TUI/CLI/cron）不派发给 actor。
            // 短 TTL 缓存（含负结果）避免每事件一次 sqlite 读；投递侧
            // （actor 内）总是新鲜重读，此处用快照不构成锚点风险。
            let routing_cache: DashMap<
                SessionId,
                (Option<Arc<SessionRouting>>, std::time::Instant),
            > = DashMap::new();
            // 心跳 PATCH 在飞标记（评审 nit #6）：上一次还没跑完就不再
            // 叠加 spawn，挂起的 PATCH 不会堆积任务。
            let refresh_in_flight = Arc::new(std::sync::atomic::AtomicBool::new(false));
            // 死亡清扫在飞标记（终审 #1：清扫给每张死卡做 PATCH/表情等
            // 平台 IO，必须和心跳一样移出本循环——内联就是事故根机制）。
            let sweep_in_flight = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let mut watchdog = tokio::time::interval(WATCHDOG_SWEEP_INTERVAL);
            watchdog.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut live_refresh = tokio::time::interval(LIVE_CARD_REFRESH_INTERVAL);
            live_refresh.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    biased;
                    () = token.cancelled() => break,
                    // Ticks yield to queued events (guard disables the arm
                    // under `biased`): terminal events flip the conductor's
                    // state mirror to Idle the instant they are emitted, but
                    // this loop may not have processed them yet — judging
                    // liveness (obs dead-card sweep) before draining the
                    // queue could sweep live sessions' cards. The reply
                    // settle path has the same guard per-actor (delivery_pool).
                    // Sustained flow defers ticks (MissedTickBehavior::Skip);
                    // they catch up when traffic quiets.
                    _ = live_refresh.tick(), if rx.is_empty() => {
                        if !rx.is_empty() {
                            continue;
                        }
                        // Live-card heartbeat off-loop: obs state is
                        // DashMap-sharded; a racing heartbeat PATCH only
                        // costs a seconds-stale render that self-heals on
                        // the next update. Keeps the loop free of PATCH
                        // bursts after a flood (91 stale cards ≈ a minute
                        // of inline PATCHes — exactly what killed it).
                        if refresh_in_flight
                            .swap(true, std::sync::atomic::Ordering::AcqRel)
                        {
                            continue;
                        }
                        let guard = ResetOnDrop(Arc::clone(&refresh_in_flight));
                        let obs = Arc::clone(&obs);
                        tokio::spawn(async move {
                            let _guard = guard;
                            obs.refresh_stale(LIVE_CARD_REFRESH_INTERVAL).await;
                        });
                    }
                    _ = watchdog.tick(), if rx.is_empty() => {
                        if !rx.is_empty() {
                            continue;
                        }
                        // 顺手驱逐路由缓存的过期项（cron/subagent churn，
                        // 防无界缓慢增长——评审 should-fix #2）。
                        routing_cache
                            .retain(|_, (_, at)| at.elapsed() < ROUTING_CACHE_TTL);
                        // 死亡清扫移出循环（终审 #1：每张死卡一次 PATCH+
                        // 表情，内联执行 = 事故根机制复刻）。判活谓词同
                        // 时看 conductor 镜像、actor 队列/在飞（终审 #2）
                        // 与 actor 持有的回复 buffer（终审 #2 双重结算）：
                        // 有投递状态的一律不是孤儿卡，归 actor 结算。
                        if sweep_in_flight
                            .swap(true, std::sync::atomic::Ordering::AcqRel)
                        {
                            continue;
                        }
                        let guard = ResetOnDrop(Arc::clone(&sweep_in_flight));
                        let obs = Arc::clone(&obs);
                        let pool = pool.clone();
                        let kernel = kernel.clone();
                        tokio::spawn(async move {
                            let _guard = guard;
                            // Kernel gone = 关闭中，不扫。
                            if let Some(k) = kernel.upgrade() {
                                obs.sweep_dead_sessions(|sid| {
                                    k.is_session_running(sid)
                                        || !pool.is_quiet(sid)
                                        || pool.has_buffer(sid)
                                })
                                .await;
                            }
                        });
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
                        // 门禁：无路由的会话不派发。事件的一切处理
                        // （记账/obs/ask/typing/投递）都在 actor 内闭环。
                        let Some(routing) =
                            routing_for(&store, &routing_cache, &session_id).await
                        else {
                            continue;
                        };
                        pool.dispatch(
                            &session_id,
                            DeliveryJob {
                                routing,
                                event: envelope.event,
                            },
                        );
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

    /// Whether the channel this session routes to can render cards —
    /// the ask_user question card exists only on such surfaces (text
    /// platforms keep the tool blocked to avoid the 2-minute timeout).
    /// `false` for unrouted sessions and unknown channels alike.
    pub async fn session_channel_supports_cards(&self, session_id: &SessionId) -> bool {
        let Ok(Some(routing)) = self.store.find_routing_by_session(session_id).await else {
            return false;
        };
        self.instances
            .get(&routing.channel_name)
            .is_some_and(|instance| instance.adapter.supports_status_card())
    }
}

/// Routing gate lookup with a short TTL cache: keeps the demux loop off
/// sqlite under floods. Hits and *misses* are both cached (unrouted
/// sessions — TUI/CLI/subagent — would otherwise cost a store read per
/// event). Only a *gate* plus a display snapshot — delivery paths always
/// re-read fresh inside the session actor, so a stale snapshot can never
/// mis-anchor a reply.
async fn routing_for(
    store: &Arc<dyn ChannelStore>,
    cache: &DashMap<SessionId, (Option<Arc<SessionRouting>>, std::time::Instant)>,
    session_id: &SessionId,
) -> Option<Arc<SessionRouting>> {
    // 分级 TTL（评审 should-fix #3）：正结果 2s，负结果 250ms——负缓存
    // 若遮住一个刚建好路由的会话，会在门禁处吞掉它整个 run（连 actor
    // 都不会创建，巡检也救不回来）；250ms 把理论窗口压到近零，同时仍
    // 把 subagent/TUI 会话的查询压到每秒几次。
    const POS_TTL: std::time::Duration = std::time::Duration::from_secs(2);
    const NEG_TTL: std::time::Duration = std::time::Duration::from_millis(250);
    if let Some(entry) = cache.get(session_id) {
        let (routing, at) = entry.value();
        let ttl = if routing.is_some() { POS_TTL } else { NEG_TTL };
        if at.elapsed() < ttl {
            return routing.clone();
        }
    }
    match store.find_routing_by_session(session_id).await {
        Ok(found) => {
            let found = found.map(Arc::new);
            cache.insert(
                session_id.clone(),
                (found.clone(), std::time::Instant::now()),
            );
            found
        }
        Err(e) => {
            // 瞬时错误不缓存（下个事件重试），但本轮按无路由跳过。
            error!(error = %e, "failed to look up routing for session");
            None
        }
    }
}

fn build_adapter(platform: &super::PlatformConfig) -> Arc<dyn PlatformAdapter> {
    match platform {
        super::PlatformConfig::Telegram { token } => {
            Arc::new(super::telegram::TelegramAdapter::new(token.clone()))
        }
        super::PlatformConfig::Feishu { app_id, app_secret } => Arc::new(
            super::feishu::FeishuAdapter::new(app_id.clone(), app_secret.clone()),
        ),
    }
}

#[cfg(test)]
#[path = "hub_test.rs"]
mod tests;
