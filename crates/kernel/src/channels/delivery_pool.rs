//! 每会话投递 actor：把一个会话的**全部投递状态与平台 IO** 收拢到一个
//! 任务里。全局事件循环只做分派（见 `hub.rs`），不再碰任何网络调用。
//!
//! 背景（2026-08-21 `EventBus` 洪峰事故）：旧架构由单一全局循环内联
//! 执行所有会话的飞书 API 调用，消费速度被网络延迟锁死；91 会话并发
//! 时 bus 队列被打满，投递事件（含 `Stopped`/回复正文）被静默丢弃。
//!
//! 设计（actor 模型）：
//! - 每个经路由的会话拥有**一个** worker，事件经一条 FIFO 通道按序
//!   到达；reply buffer、obs/ask/typing、回复投递全部在 worker 内
//!   闭环——同会话保序与旧单循环语义完全一致，跨会话天然并行；
//! - agent 死亡但 `Stopped` 丢失（bus 丢件）时，worker 靠自己的
//!   巡检节拍兜底投递残余回复（替代旧的全局 watchdog 回复扫描，
//!   延迟 60s → 30s）；判死前确认自身队列为空，杜绝与在队
//!   `Stopped` 的双重结算；
//! - buffer 为空且长期闲置的 worker 走四步防竞态退出（摘除登记 →
//!   close 通道 → 排空余量 → 退出），僵尸会话的 worker 不会只增
//!   不减，也绝不丢在飞事件；
//! - 全局信号量限制并发平台 IO 上限，避免洪峰期 API 洪泛。

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{mpsc, Semaphore};
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

use crate::event::{AgentEvent, AgentStatus, Event, ModelEvent, ToolEvent};
use crate::kernel::Kernel;
use crate::types::SessionId;

use super::ask::AskCardRegistry;
use super::hub::ChannelInstance;
use super::hub_deliver::{deliver_reply, notify_run_subscribers, RunEndStatus, SettleKind};
use super::obs::ObsTracker;
use super::reply::RunReplyBuffer;
use super::{ChannelStore, SessionRouting};

/// 每会话事件通道容量。与上游 bus 的全局队列（hub 侧 4096）相配：
/// 单会话 256 足够吸收正常突发，又限制洪峰期的内存占用（91 会话全满
/// ≈ 2.3 万事件）；真打满时 ERROR 告警（此时上游 bus 的丢件告警必然
/// 早已触发）。
const SESSION_EVENT_CAPACITY: usize = 256;

/// 全局并发平台 IO 上限（feishu 限流按 chat 计，worker 粒度≈chat 粒度，
/// 但总量仍需封顶以防 API 洪泛）。
const MAX_CONCURRENT_IO: usize = 16;

/// agent 死亡探测节拍：buffer 里有残余回复但 session 已不在运行
/// （`Stopped` 被 bus 丢弃），以 Timeout 形态兜底送出。
const SELF_SETTLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// buffer 为空且闲置超过此时长的 worker 自动退出（注册由 dispatch
/// 惰性清理），避免僵尸会话的 worker 只增不减。
const WORKER_IDLE_REAP: std::time::Duration = std::time::Duration::from_mins(15);

/// 派给会话 actor 的一条事件。`routing` 是分派时的快照（TTL 缓存，Arc
/// 共享避免每事件克隆），用于门禁、obs 展示以及 ask/typing 的目标定位
/// （≤2s 陈旧可接受：锚点漂移慢）；**投递路径（Deliver/兜底）总是新鲜
/// 重读**，回复锚点绝不用陈旧的（评审复核修正注释）。
pub(crate) struct DeliveryJob {
    pub(crate) routing: Arc<SessionRouting>,
    pub(crate) event: Event,
}

/// actor 共享上下文。
struct DeliveryCtx {
    obs: Arc<ObsTracker>,
    ask: Arc<AskCardRegistry>,
    store: Arc<dyn ChannelStore>,
    instances: Arc<DashMap<String, ChannelInstance>>,
    kernel: std::sync::Weak<Kernel>,
    /// 判死探针（测试缝）：返回 true 表示该会话的 agent 已不在运行。
    /// 生产实现经 kernel 查询；kernel 已消失（关闭流程）时返回 false，
    /// 避免误杀在飞的 run。
    agent_dead: Box<dyn Fn(&SessionId) -> bool + Send + Sync>,
    io_permits: Semaphore,
    token: CancellationToken,
    /// 巡检/回收节拍（测试可缩短）。
    settle_interval: std::time::Duration,
    idle_reap: std::time::Duration,
}

/// 会话 → 投递 actor 的分派器。
pub(crate) struct DeliveryPool {
    senders: Arc<DashMap<SessionId, mpsc::Sender<DeliveryJob>>>,
    /// 每 actor 的在飞任务计数（obs 死亡清扫判活谓词用）。
    inflight: Arc<DashMap<SessionId, Arc<AtomicU32>>>,
    ctx: Arc<DeliveryCtx>,
}

impl DeliveryPool {
    pub(crate) fn new(
        obs: Arc<ObsTracker>,
        ask: Arc<AskCardRegistry>,
        store: Arc<dyn ChannelStore>,
        instances: Arc<DashMap<String, ChannelInstance>>,
        kernel: std::sync::Weak<Kernel>,
        token: CancellationToken,
    ) -> Self {
        let agent_dead = {
            let kernel = kernel.clone();
            move |sid: &SessionId| kernel.upgrade().is_some_and(|k| !k.is_session_running(sid))
        };
        Self::with_timing(
            obs,
            ask,
            store,
            instances,
            kernel,
            Box::new(agent_dead),
            token,
            SELF_SETTLE_INTERVAL,
            WORKER_IDLE_REAP,
        )
    }

    /// 完整构造（判死探针与节拍可注入——测试用短节拍覆盖巡检/回收路径）。
    #[allow(clippy::too_many_arguments)] // 依赖注入构造器，参数即上下文
    fn with_timing(
        obs: Arc<ObsTracker>,
        ask: Arc<AskCardRegistry>,
        store: Arc<dyn ChannelStore>,
        instances: Arc<DashMap<String, ChannelInstance>>,
        kernel: std::sync::Weak<Kernel>,
        agent_dead: Box<dyn Fn(&SessionId) -> bool + Send + Sync>,
        token: CancellationToken,
        settle_interval: std::time::Duration,
        idle_reap: std::time::Duration,
    ) -> Self {
        Self {
            senders: Arc::new(DashMap::new()),
            inflight: Arc::new(DashMap::new()),
            ctx: Arc::new(DeliveryCtx {
                obs,
                ask,
                store,
                instances,
                kernel,
                agent_dead,
                io_permits: Semaphore::new(MAX_CONCURRENT_IO),
                token,
                settle_interval,
                idle_reap,
            }),
        }
    }

    /// 测试构造：判死探针恒真 + 短节拍，覆盖巡检兜底路径。
    #[cfg(test)]
    fn for_test(
        store: Arc<dyn ChannelStore>,
        instances: Arc<DashMap<String, ChannelInstance>>,
    ) -> Self {
        Self::with_timing(
            Arc::new(ObsTracker::new()),
            Arc::new(AskCardRegistry::new()),
            store,
            instances,
            std::sync::Weak::new(),
            Box::new(|_| true),
            CancellationToken::new(),
            std::time::Duration::from_millis(50),
            std::time::Duration::from_hours(1),
        )
    }

    /// 该会话的投递是否安静（无 actor，或队列空且无在飞任务）。
    ///
    /// 供 obs 死亡清扫的判活谓词使用（发版终审 #2）：`Stopped` 发出时
    /// conductor 的状态镜像即刻翻为 Idle，但它可能还排在 actor 队列
    /// 里（IO 信号量后）或正在投递中——此时把卡片错冻成 ⏰ 会产生
    /// "既丢又成"的矛盾 UX。
    pub(crate) fn is_quiet(&self, session_id: &SessionId) -> bool {
        let queue_empty = self
            .senders
            .get(session_id)
            .is_none_or(|tx| tx.capacity() == tx.max_capacity());
        let no_inflight = self
            .inflight
            .get(session_id)
            .is_none_or(|c| c.load(Ordering::Relaxed) == 0);
        queue_empty && no_inflight
    }

    /// 派一个事件给该会话的 actor（不存在则创建）。同会话严格 FIFO。
    ///
    /// 永不在调用方阻塞：队满记 ERROR 丢件（此时系统已处于深度异常，
    /// 上游 bus 的丢件告警必然先触发）；actor 恰好死亡/退出则摘除
    /// 登记、重建并重投一次。
    pub(crate) fn dispatch(&self, session_id: &SessionId, job: DeliveryJob) {
        let mut job = Some(job);
        for _ in 0..2 {
            let sender = match self.senders.entry(session_id.clone()) {
                dashmap::mapref::entry::Entry::Occupied(e) => e.get().clone(),
                dashmap::mapref::entry::Entry::Vacant(e) => {
                    let (tx, rx) = mpsc::channel::<DeliveryJob>(SESSION_EVENT_CAPACITY);
                    e.insert(tx.clone());
                    let ctx = Arc::clone(&self.ctx);
                    let senders = Arc::clone(&self.senders);
                    let sid = session_id.clone();
                    let own_tx = tx.clone();
                    let counter = self
                        .inflight
                        .entry(session_id.clone())
                        .or_insert_with(|| Arc::new(AtomicU32::new(0)))
                        .clone();
                    let inflight = Arc::clone(&self.inflight);
                    tokio::spawn(async move {
                        run_actor(sid, rx, own_tx, senders, inflight, counter, ctx).await;
                    });
                    tx
                }
            };
            match sender.try_send(job.take().expect("job consumed on retry")) {
                Ok(()) => return,
                Err(mpsc::error::TrySendError::Full(returned)) => {
                    error!(
                        session_id = %session_id.0,
                        "session delivery queue full, dropping event (deep overload)"
                    );
                    let _ = returned;
                    return;
                }
                Err(mpsc::error::TrySendError::Closed(returned)) => {
                    // actor 已退出（空闲回收/panic）：摘除死登记后重建重投。
                    // 必须确认登记的还是**这条**死通道——并发 dispatch 可能
                    // 已经重建过，误删新登记会造成同会话双 actor 乱序。
                    job = Some(returned);
                    self.senders
                        .remove_if(session_id, |_, tx| tx.same_channel(&sender));
                }
            }
        }
        error!(
            session_id = %session_id.0,
            "failed to dispatch delivery job after actor respawn"
        );
    }
}

/// 在飞任务计数 guard（Drop 自减，panic 安全）。
struct InflightGuard(Arc<AtomicU32>);

impl InflightGuard {
    fn new(counter: &Arc<AtomicU32>) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        Self(Arc::clone(counter))
    }
}

impl Drop for InflightGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// 会话投递 actor 主循环。
async fn run_actor(
    session_id: SessionId,
    mut rx: mpsc::Receiver<DeliveryJob>,
    own_tx: mpsc::Sender<DeliveryJob>,
    senders: Arc<DashMap<SessionId, mpsc::Sender<DeliveryJob>>>,
    inflight_map: Arc<DashMap<SessionId, Arc<AtomicU32>>>,
    own_inflight: Arc<AtomicU32>,
    ctx: Arc<DeliveryCtx>,
) {
    // 本 run 的回复缓冲：存在即"run 在飞"的标记（与原全局循环的
    // reply_buffers 语义一致），在 Stopped/兜底时取走投递。
    let mut buffer: Option<RunReplyBuffer> = None;
    let mut settle_tick = tokio::time::interval(ctx.settle_interval);
    settle_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last_job = std::time::Instant::now();

    loop {
        tokio::select! {
            biased;
            () = ctx.token.cancelled() => break,
            _ = settle_tick.tick() => {
                // 队列里还有未处理事件时，判死与回收都让步——Stopped
                // 可能正排在队里（原全局 watchdog 的 rx.is_empty()
                // 守卫同款语义；上游分派循环只增亚毫秒级残余窗口）。
                if !rx.is_empty() {
                    continue;
                }
                if buffer.is_some() {
                    // agent 已死但 Stopped 丢失：兜底投递（kernel 消失的
                    // 误判由 agent_dead 探针内部挡住；瞬时失败时 buffer
                    // 保留在原地，下个节拍继续重试）。
                    if (ctx.agent_dead)(&session_id) {
                        let _busy = InflightGuard::new(&own_inflight);
                        settle_deliver(&session_id, &mut buffer, SettleKind::Timeout, &ctx)
                            .await;
                    }
                } else if last_job.elapsed() >= ctx.idle_reap {
                    // 空闲回收，四步防竞态：① 原子摘除登记（仅当登记的
                    // 还是自己——并发 dispatch 可能已重建）；② close 通
                    // 道（此后持有旧发送端的 dispatch 得 Closed 错误，
                    // 自动走重建重投路径）；③ 排空余量；④ 退出。摘除
                    // 失败说明登记易手中，继续服务。
                    if senders
                        .remove_if(&session_id, |_, tx| tx.same_channel(&own_tx))
                        .is_some()
                    {
                        inflight_map.remove_if(&session_id, |_, c| {
                            Arc::ptr_eq(c, &own_inflight)
                        });
                        rx.close();
                        while let Ok(job) = rx.try_recv() {
                            let _busy = InflightGuard::new(&own_inflight);
                            run_job(&session_id, job, &mut buffer, &ctx).await;
                        }
                        // 回收窗口内进来的 run 起始事件可能建起了
                        // buffer——不能让它随 actor 一起死（静默部分丢
                        // 回复的同类，发版终审 #3）：兜底送出并留痕。
                        if buffer.is_some() {
                            warn!(
                                session_id = %session_id.0,
                                "reap drain left a live buffer, settling before exit"
                            );
                            settle_deliver(&session_id, &mut buffer, SettleKind::Timeout, &ctx)
                                .await;
                        }
                        break;
                    }
                }
            }
            job = rx.recv() => {
                let Some(job) = job else { break };
                last_job = std::time::Instant::now();
                let _busy = InflightGuard::new(&own_inflight);
                run_job(&session_id, job, &mut buffer, &ctx).await;
            }
        }
    }
}

/// 执行一条事件；panic 降级为 ERROR 日志（actor 与队列存活，后续事件
/// 不受牵连）——**静默丢回复正是本次修复要消灭的失败模式**（评审
/// should-fix #1）。
async fn run_job(
    session_id: &SessionId,
    job: DeliveryJob,
    buffer: &mut Option<RunReplyBuffer>,
    ctx: &Arc<DeliveryCtx>,
) {
    use futures::FutureExt as _;
    let result = std::panic::AssertUnwindSafe(handle_event(session_id, job, buffer, ctx))
        .catch_unwind()
        .await;
    if let Err(panic) = result {
        error!(
            session_id = %session_id.0,
            panic = ?panic,
            "delivery actor: event handling panicked, actor continues"
        );
    }
}

/// 处理一条事件：记账 → run 结束投递 → ask 卡 → obs → typing。
/// 顺序与原全局单循环逐行对应。
async fn handle_event(
    session_id: &SessionId,
    job: DeliveryJob,
    buffer: &mut Option<RunReplyBuffer>,
    ctx: &DeliveryCtx,
) {
    let DeliveryJob { routing, event } = job;
    let is_doc_comment = routing.doc_comment.is_some();

    // 并发 IO 封顶；信号量永不关闭，取不到 permit 时退化为不限流。
    let _permit = ctx.io_permits.acquire().await.ok();

    // ── reply buffer 记账（纯内存，与原循环一致）──
    match &event {
        Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Running,
        }) => {
            // Running 每 turn 触发；已有 buffer 则保留（run 跨 turn）。
            let buf = buffer.get_or_insert_with(RunReplyBuffer::default);
            // run 的第一条 Running：把会话模型放上状态卡（每 run 一次
            // store 读；kernel 没了就跳过）。
            let observability = channel_flags(&routing, ctx).is_some_and(|f| f.observability);
            if observability && !is_doc_comment && !ctx.obs.has_state(session_id) {
                if let Some(k) = ctx.kernel.upgrade() {
                    let model = k.get_session_model(session_id).await;
                    ctx.obs.set_model(session_id, model.clone());
                    buf.set_model(model);
                }
            }
        }
        Event::Model(ModelEvent::End { content, .. }) => {
            let text = super::blocks_to_text(content);
            buffer
                .get_or_insert_with(RunReplyBuffer::default)
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
            if let Some(buf) = buffer.as_mut() {
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
            buffer
                .get_or_insert_with(RunReplyBuffer::default)
                .record_tool_start(tool_id, tool_name, arguments.as_deref());
        }
        Event::Tool(ToolEvent::End {
            tool_id,
            elapsed_ms,
            is_error,
            ..
        }) => {
            if let Some(buf) = buffer.as_mut() {
                buf.record_tool_end(tool_id, *elapsed_ms, *is_error);
            }
        }
        _ => {}
    }

    // ── run 结束：取走 buffer 投递（投递路径新鲜重读路由；易错前置
    // 未过时 buffer 原样保留，等巡检节拍重试）──
    if let Event::Agent(AgentEvent::Lifecycle {
        state: AgentStatus::Stopped { reason },
    }) = &event
    {
        settle_deliver(session_id, buffer, SettleKind::Stopped(reason), ctx).await;
        return;
    }

    // 以下平台操作需要渠道实例；实例没了直接丢（与原循环的 continue 同义）。
    let Some(flags) = channel_flags(&routing, ctx) else {
        return;
    };
    let supports_cards = flags.adapter.supports_status_card();
    let observability = flags.observability;

    // ── ask_user 决策卡 ──
    match &event {
        Event::Agent(AgentEvent::AskUserQuestion {
            req_id, questions, ..
        }) if supports_cards && !is_doc_comment => {
            ctx.ask
                .send_question_cards(
                    &flags.adapter,
                    &routing.external_chat_id,
                    routing.reply_msg_id.as_deref(),
                    session_id,
                    req_id,
                    questions,
                )
                .await;
        }
        Event::Agent(AgentEvent::AskUserAck { req_id }) => {
            ctx.ask.close_cards(&flags.adapter, req_id).await;
        }
        _ => {}
    }

    // ── observability：状态卡更新（内部有 PATCH 节流）──
    if observability && !is_doc_comment {
        ctx.obs
            .handle_event(
                &flags.adapter,
                session_id,
                &routing.external_chat_id,
                routing.reply_msg_id.as_deref(),
                &event,
            )
            .await;
    }

    // ── typing：无卡平台/observability 关闭时的进度信号 ──
    if matches!(event, Event::Model(ModelEvent::Request { .. }))
        && (!supports_cards || !observability)
        && !is_doc_comment
    {
        let _ = flags.adapter.send_typing(&routing.external_chat_id).await;
    }
}

/// 投递回复 + 通知订阅者。Stopped 与巡检兜底共用。**总是新鲜重读
/// 路由与渠道实例**——回复锚点可能在 run 中移动，陈旧锚点会把回复
/// 挂到旧消息上。
///
/// buffer 以 `&mut` 传入、**易错前置条件全部通过后才 take**（评审
/// 复核）：瞬时 store 错误原样保留、下拍重试（旧全局 watchdog 本来
/// 就给第二次机会）；路由/实例永久消失才取走丢弃（均有 warn）。
/// 残余窗口：take 之后若 `deliver_reply`/`notify` 自身 panic，该回复
/// 仍会丢（有 ERROR 日志）——彻底消灭它需要持久化 outbox，属另一个
/// 课题。
async fn settle_deliver(
    session_id: &SessionId,
    buffer: &mut Option<RunReplyBuffer>,
    settle_kind: SettleKind<'_>,
    ctx: &DeliveryCtx,
) {
    let routing = match ctx.store.find_routing_by_session(session_id).await {
        Ok(Some(r)) => r,
        // 路由被 gc：回复无处投递（与原 watchdog 的丢 buffer 同义）。
        Ok(None) => {
            if buffer.take().is_some() {
                warn!(
                    session_id = %session_id.0,
                    "routing gone, dropping undeliverable reply buffer"
                );
            }
            return;
        }
        // 瞬时 store 错误：buffer 原样保留，下个节拍重试。告警（巡检
        // 节拍天然限频 30s）——store 若持续损坏，这是唯一的可见性
        // （评审 must-fix b）。
        Err(e) => {
            warn!(
                session_id = %session_id.0,
                error = %e,
                "routing lookup failed, reply buffer kept for retry"
            );
            return;
        }
    };
    let Some(flags) = channel_flags(&routing, ctx) else {
        // 渠道实例已删（运维摘了 channel）：同样无处投递——必须留痕
        // （评审 must-fix a）。
        if buffer.take().is_some() {
            warn!(
                session_id = %session_id.0,
                channel = %routing.channel_name,
                "channel instance gone, dropping undeliverable reply buffer"
            );
        }
        return;
    };
    let (kind, status) = (settle_kind, {
        match &settle_kind {
            SettleKind::Stopped(r) => RunEndStatus::from_stop_reason(r),
            // 兜底路径的已知取舍（评审 nit #5）：Timeout 时订阅者状态记为
            // ❌/“session lost”——投递可靠性优先于状态保真度。
            SettleKind::Timeout => RunEndStatus::Failed,
        }
    });
    let reply_msg_id = deliver_reply(
        &ctx.obs,
        &flags.adapter,
        &routing,
        buffer.take().map(RunReplyBuffer::into_reply),
        flags.tool_trace,
        flags.observability,
        flags.mid_run_split,
        session_id,
        kind,
        &ctx.kernel,
    )
    .await;
    // 回复已送达（或已尽力）——订阅者拿到带链接的完成通知。
    notify_run_subscribers(
        &ctx.store,
        &flags.adapter,
        &routing,
        reply_msg_id.as_deref(),
        status,
        session_id,
        &ctx.kernel,
        &ctx.obs,
    )
    .await;
}

/// 渠道实例的投递相关配置（命名字段——曾发生 (adapter, bool, bool, bool)
/// 元组在两个站点顺序不一致导致旗标绑反的发版级 bug，改结构体消灭这一类）。
struct ChannelFlags {
    adapter: Arc<dyn super::PlatformAdapter>,
    observability: bool,
    tool_trace: bool,
    mid_run_split: bool,
}

fn channel_flags(routing: &SessionRouting, ctx: &DeliveryCtx) -> Option<ChannelFlags> {
    ctx.instances
        .get(&routing.channel_name)
        .map(|i| ChannelFlags {
            adapter: Arc::clone(&i.adapter),
            observability: i.config.observability,
            tool_trace: i.config.tool_trace,
            mid_run_split: i.config.mid_run_split,
        })
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::hub::ChannelInstance;
    use crate::channels::store::SqliteChannelStore;
    use crate::channels::{ChannelConfig, ChannelError, ChannelEvent, PlatformAdapter};
    use crate::event::StopReason;
    use crate::storage::migrations::run_migrations;
    use crate::types::ContentBlock;
    use sqlx::sqlite::SqlitePoolOptions;
    use tokio::sync::mpsc as tokio_mpsc;

    /// 极简 adapter：只记录 `send_message` 的文本（typing 用默认 no-op）。
    struct MockAdapter {
        sent: tokio::sync::Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl PlatformAdapter for MockAdapter {
        async fn run_receiver(
            &self,
            _incoming: tokio_mpsc::Sender<ChannelEvent>,
            cancel: CancellationToken,
        ) -> Result<(), ChannelError> {
            cancel.cancelled().await;
            Ok(())
        }

        async fn send_message(
            &self,
            _chat: &str,
            blocks: Vec<ContentBlock>,
            _anchor: Option<&str>,
        ) -> Result<Option<String>, ChannelError> {
            let text = blocks
                .iter()
                .find_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            self.sent.lock().await.push(text);
            Ok(Some("m-1".into()))
        }
    }

    fn test_routing() -> Arc<SessionRouting> {
        Arc::new(SessionRouting {
            channel_name: "feishu".to_string(),
            external_chat_id: "chat-1".to_string(),
            reply_msg_id: None,
            mapping_key: "chat-1".to_string(),
            doc_comment: None,
        })
    }

    /// 端到端：Running → End(文本) → Stopped 流经 actor 后，回复必须
    /// 经 adapter 送出（2026-08-21 事故的正面回归：投递链路工作）。
    #[tokio::test]
    async fn actor_delivers_reply_on_stopped() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&db).await.unwrap();
        let store: Arc<dyn ChannelStore> = Arc::new(SqliteChannelStore::new(db));
        let sid = SessionId::from("sess_actor_e2e");
        store
            .save_mapping("feishu", "chat-1", &sid, "chat-1", None)
            .await
            .unwrap();

        let adapter = Arc::new(MockAdapter {
            sent: tokio::sync::Mutex::new(Vec::new()),
        });
        let mut config = ChannelConfig {
            name: "feishu".to_string(),
            enabled: true,
            ..ChannelConfig::default()
        };
        // 纯文本投递路径（无状态卡分支，直接断言 send_message）。
        config.observability = false;
        let instances: Arc<DashMap<String, ChannelInstance>> = Arc::new(DashMap::new());
        instances.insert(
            "feishu".to_string(),
            ChannelInstance::test_instance(config, adapter.clone()),
        );

        let pool = DeliveryPool::new(
            Arc::new(ObsTracker::new()),
            Arc::new(AskCardRegistry::new()),
            store,
            instances,
            std::sync::Weak::new(),
            CancellationToken::new(),
        );

        let routing = test_routing();
        let events = vec![
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Running,
            }),
            Event::Model(ModelEvent::End {
                message_id: "m-1".into(),
                content: vec![ContentBlock::Text {
                    text: "答案42".to_string(),
                }],
            }),
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Stopped {
                    reason: StopReason::Completed {
                        finish_reason: None,
                    },
                },
            }),
        ];
        for event in events {
            pool.dispatch(
                &sid,
                DeliveryJob {
                    routing: Arc::clone(&routing),
                    event,
                },
            );
        }

        // actor 是异步的：轮询直到投递落地（超时即失败）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if adapter
                .sent
                .lock()
                .await
                .iter()
                .any(|t| t.contains("答案42"))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "reply was not delivered by the session actor"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// 事故核心回归（评审 should-fix #4）：`Stopped` 被 bus 丢弃（只发
    /// Running + End，永远不发 `Stopped`）→ actor 的巡检判死（注入
    /// `agent_dead=true`）后必须把残余回复以 Timeout 形态兜底送出。
    #[tokio::test]
    async fn actor_settles_reply_when_stopped_lost() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&db).await.unwrap();
        let store: Arc<dyn ChannelStore> = Arc::new(SqliteChannelStore::new(db));
        let sid = SessionId::from("sess_actor_settle");
        store
            .save_mapping("feishu", "chat-1", &sid, "chat-1", None)
            .await
            .unwrap();

        let adapter = Arc::new(MockAdapter {
            sent: tokio::sync::Mutex::new(Vec::new()),
        });
        let mut config = ChannelConfig {
            name: "feishu".to_string(),
            enabled: true,
            ..ChannelConfig::default()
        };
        config.observability = false;
        let instances: Arc<DashMap<String, ChannelInstance>> = Arc::new(DashMap::new());
        instances.insert(
            "feishu".to_string(),
            ChannelInstance::test_instance(config, adapter.clone()),
        );

        // 判死探针恒真（模拟 agent 已死），巡检节拍 50ms。
        let pool = DeliveryPool::for_test(store, instances);

        let routing = test_routing();
        // 只发 Running + End——Stopped“丢了”。
        for event in [
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Running,
            }),
            Event::Model(ModelEvent::End {
                message_id: "m-1".into(),
                content: vec![ContentBlock::Text {
                    text: "兜底答案7".to_string(),
                }],
            }),
        ] {
            pool.dispatch(
                &sid,
                DeliveryJob {
                    routing: Arc::clone(&routing),
                    event,
                },
            );
        }

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if adapter
                .sent
                .lock()
                .await
                .iter()
                .any(|t| t.contains("兜底答案7"))
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "lost-Stopped reply was not settled via the actor watchdog"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// 旗标绑反回归（发版终审 BLOCK 项）：`observability=false` +
    /// `tool_trace=true` 的配置下，`channel_flags` 必须按名字映射——
    /// 历史上此处按位置传递布尔导致两旗标互换（同 true 时无症状，
    /// 配置不同才爆炸）。
    #[tokio::test]
    async fn channel_flags_maps_config_by_name() {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&db).await.unwrap();
        let store: Arc<dyn ChannelStore> = Arc::new(SqliteChannelStore::new(db));

        let adapter = Arc::new(MockAdapter {
            sent: tokio::sync::Mutex::new(Vec::new()),
        });
        let mut config = ChannelConfig {
            name: "feishu".to_string(),
            enabled: true,
            ..ChannelConfig::default()
        };
        config.observability = false;
        config.tool_trace = true;
        let instances: Arc<DashMap<String, ChannelInstance>> = Arc::new(DashMap::new());
        instances.insert(
            "feishu".to_string(),
            ChannelInstance::test_instance(config, adapter),
        );

        let pool = DeliveryPool::for_test(store, instances);
        let flags = channel_flags(&test_routing(), &pool.ctx).expect("instance exists");
        assert!(!flags.observability, "observability must map from config");
        assert!(flags.tool_trace, "tool_trace must map from config");
    }
}
