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
//! - buffer 为空且长期闲置的 worker 走三步防竞态退出（摘除登记 →
//!   close 通道 → 余量**重投**给新 actor），僵尸会话的 worker 不会
//!   只增不减，也绝不丢在飞事件（M1：重投而非就地执行，旧 actor
//!   不产生状态，杜绝同一 run 被两个 actor 劈开结算）；
//! - 全局信号量限制并发平台 IO 上限，避免洪峰期 API 洪泛。

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// 每会话 actor 登记：发送端与对外旗标一体。原 senders/inflight/
/// buffers 三表需锁步维护，回收窗口内新 actor 会复用旧旗标 Arc、随后
/// 被旧 actor 的摘除误伤（旗标entry消失、旗标失真）——单表单 entry
/// 原子建删，从结构上消灭这类错位（评审复核）。
struct ActorHandle {
    tx: mpsc::Sender<DeliveryJob>,
    /// 在飞任务计数（obs 死亡清扫判活谓词用）。
    inflight: Arc<AtomicU32>,
    /// 是否持有未投递的回复 buffer（清扫对持有者让步，杜绝"先冻后成"
    /// 的双重结算——终审 #2）。
    has_buffer: Arc<AtomicBool>,
}

/// 会话 → 投递 actor 的分派器。
#[derive(Clone)]
pub(crate) struct DeliveryPool {
    actors: Arc<DashMap<SessionId, ActorHandle>>,
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
            actors: Arc::new(DashMap::new()),
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
        self.actors.get(session_id).is_none_or(|h| {
            h.tx.capacity() == h.tx.max_capacity() && h.inflight.load(Ordering::Relaxed) == 0
        })
    }

    /// 该会话的 actor 是否持有未投递的回复 buffer。清扫谓词对它让步：
    /// 卡片+回复的结算权威是 actor，清扫只收真正的孤儿卡。
    pub(crate) fn has_buffer(&self, session_id: &SessionId) -> bool {
        self.actors
            .get(session_id)
            .is_some_and(|h| h.has_buffer.load(Ordering::Relaxed))
    }

    /// 派一个事件给该会话的 actor（不存在则创建）。同会话严格 FIFO。
    ///
    /// 永不在调用方阻塞：队满记 ERROR 丢件（此时系统已处于深度异常，
    /// 上游 bus 的丢件告警必然先触发）；actor 恰好死亡/退出则摘除
    /// 登记、重建并重投一次。
    pub(crate) fn dispatch(&self, session_id: &SessionId, job: DeliveryJob) {
        dispatch(&self.actors, &self.ctx, session_id, job);
    }
}

/// dispatch 的自由函数形态：actor 回收臂把余量重投给新 actor 时与外部
/// 派单走同一入口（同一建 actor/重建重投逻辑），不再各写一份。
fn dispatch(
    actors: &Arc<DashMap<SessionId, ActorHandle>>,
    ctx: &Arc<DeliveryCtx>,
    session_id: &SessionId,
    job: DeliveryJob,
) {
    let mut job = Some(job);
    for _ in 0..2 {
        let sender = match actors.entry(session_id.clone()) {
            dashmap::mapref::entry::Entry::Occupied(e) => e.get().tx.clone(),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                let (tx, rx) = mpsc::channel::<DeliveryJob>(SESSION_EVENT_CAPACITY);
                let inflight = Arc::new(AtomicU32::new(0));
                let has_buffer = Arc::new(AtomicBool::new(false));
                e.insert(ActorHandle {
                    tx: tx.clone(),
                    inflight: Arc::clone(&inflight),
                    has_buffer: Arc::clone(&has_buffer),
                });
                let ctx = Arc::clone(ctx);
                let actors = Arc::clone(actors);
                let sid = session_id.clone();
                let own_tx = tx.clone();
                tokio::spawn(async move {
                    run_actor(sid, rx, own_tx, actors, inflight, has_buffer, ctx).await;
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
                actors.remove_if(session_id, |_, h| h.tx.same_channel(&sender));
            }
        }
    }
    error!(
        session_id = %session_id.0,
        "failed to dispatch delivery job after actor respawn"
    );
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

/// actor 登记的退出清理 guard（评审 N1）：任何退出路径（含 panic、
/// 取消）都把登记摘除——登记在而 actor 亡会让 dispatch 拿到死通道多
/// 绕一轮重建，也让 `is_quiet`/`has_buffer` 读到不再更新的幽灵旗标。
/// 仅当登记的还是自己（`same_channel`）才摘：回收臂已完成登记交接时
/// 不得误删新 actor。
struct ActorRegistrationGuard {
    session_id: SessionId,
    actors: Arc<DashMap<SessionId, ActorHandle>>,
    own_tx: mpsc::Sender<DeliveryJob>,
}

impl Drop for ActorRegistrationGuard {
    fn drop(&mut self) {
        self.actors
            .remove_if(&self.session_id, |_, h| h.tx.same_channel(&self.own_tx));
    }
}

/// 回收臂③：通道余量逐条重投（M1——**不**就地执行）。登记已摘除，
/// 第一条重投即经统一 dispatch 入口重建新 actor，FIFO 全序保持；
/// 旧 actor 不产生任何状态，也就不存在"旧 actor 替新 run 建 buffer
/// 再冻 ⏰"的劈 run 窗口。
fn redispatch_remaining(
    actors: &Arc<DashMap<SessionId, ActorHandle>>,
    ctx: &Arc<DeliveryCtx>,
    session_id: &SessionId,
    rx: &mut mpsc::Receiver<DeliveryJob>,
) {
    while let Ok(job) = rx.try_recv() {
        dispatch(actors, ctx, session_id, job);
    }
}

/// 会话投递 actor 主循环。
async fn run_actor(
    session_id: SessionId,
    mut rx: mpsc::Receiver<DeliveryJob>,
    own_tx: mpsc::Sender<DeliveryJob>,
    actors: Arc<DashMap<SessionId, ActorHandle>>,
    own_inflight: Arc<AtomicU32>,
    own_has_buffer: Arc<AtomicBool>,
    ctx: Arc<DeliveryCtx>,
) {
    let _registration = ActorRegistrationGuard {
        session_id: session_id.clone(),
        actors: Arc::clone(&actors),
        own_tx: own_tx.clone(),
    };
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
                    // 保留在原地，下个节拍继续重试）。此守卫只查自身队
                    // 列——上游分派循环无 IO（终审 #1 修复后），背压
                    // 为毫秒级，Stopped 不可能"在上游堵着"（终审 #3）。
                    // 探针在 guarded 之外，自行 catch_unwind：panic 降级
                    // 为"视为存活"（保守不杀 run），actor 不受牵连（三审
                    // should-fix #2）。
                    let dead = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        (ctx.agent_dead)(&session_id)
                    }))
                    .unwrap_or_else(|panic| {
                        error!(
                            session_id = %session_id.0,
                            panic = %panic_msg(&panic),
                            "agent_dead probe panicked, treating session as alive"
                        );
                        false
                    });
                    if dead {
                        let _busy = InflightGuard::new(&own_inflight);
                        guarded(
                            &session_id,
                            "settle",
                            settle_deliver(&session_id, &mut buffer, SettleKind::Timeout, &ctx),
                        )
                        .await;
                        own_has_buffer.store(buffer.is_some(), Ordering::Relaxed);
                    }
                } else if last_job.elapsed() >= ctx.idle_reap {
                    // 空闲回收，三步：① 原子摘除登记（仅当还是自己——
                    // 此后 dispatch 得 Closed 错误会自动走重建重投）；
                    // ② close 通道；③ 余量**重投**而非就地执行（终审
                    // M1：就地执行会为别人的新 run 建 buffer，随后被本
                    // actor 的退出结算劈成两半——旧 actor 把活卡冻成
                    // ⏰Timeout、新 actor 无卡可结）。重投走正常派单：
                    // 登记已空，第一条即重建新 actor，FIFO 全序保持；
                    // 本 actor 不产生任何新状态，buffer 恒空、无需兜底
                    // 结算。残余：摘除登记与 close 之间亚微秒窗口内到达
                    // 的事件会排在重投事件之前（要求恰好此刻到达的新消
                    // 息，可接受）。
                    if actors
                        .remove_if(&session_id, |_, h| h.tx.same_channel(&own_tx))
                        .is_some()
                    {
                        rx.close();
                        redispatch_remaining(&actors, &ctx, &session_id, &mut rx);
                        break;
                    }
                }
            }
            job = rx.recv() => {
                let Some(job) = job else { break };
                last_job = std::time::Instant::now();
                let _busy = InflightGuard::new(&own_inflight);
                run_job(&session_id, job, &mut buffer, &ctx).await;
                own_has_buffer.store(buffer.is_some(), Ordering::Relaxed);
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
    guarded(
        session_id,
        "handle_event",
        handle_event(session_id, job, buffer, ctx),
    )
    .await;
}

/// panic 载荷的可读文本（`Box<dyn Any>` 的 Debug 只打 `Any { .. }`，
/// 丢失 panic 消息——四审 nit）。
fn panic_msg(panic: &dyn std::any::Any) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// 投递操作的统一 panic 安全网（复审残余项）：事件处理与巡检结算
/// 经此——panic 只留 ERROR，不杀 actor、不卡死标记。（回收余量重投
/// 是纯内存派单不走此网；判死探针在 `guarded` 之外，自带
/// catch_unwind 降级为"视为存活"。）
async fn guarded<F>(session_id: &SessionId, op: &'static str, fut: F)
where
    F: std::future::Future<Output = ()>,
{
    use futures::FutureExt as _;
    if let Err(panic) = std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        error!(
            session_id = %session_id.0,
            panic = %panic_msg(&*panic),
            op,
            "delivery actor: operation panicked, actor continues"
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

    // ── reply buffer 记账（纯内存，不占渠道 IO 闸；Running 分支的模型
    // 查询是本地 store 读）──
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

    // 并发 IO 封顶；信号量永不关闭，取不到 permit 时退化为不限流。
    // 闸只罩这段渠道侧 IO：buffer 记账是纯内存，Stopped 结算由
    // settle_deliver 内部自取——本函数若持闸再调它，每个 actor 各持
    // 1 张等第 2 张，16 个 actor 即信号量死锁（评审 S2）。
    let _permit = ctx.io_permits.acquire().await.ok();

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
    // 并发 IO 闸在此单点获取（评审 S2）：本函数被 Stopped 路径
    // （handle_event 内）与巡检/回收兜底（tick 臂，无闸上下文）共
    // 用——闸只能在这里取；交给调用方各自取既漏掉兜底路径，又会与
    // 事件路径嵌套取第二张 permit，构成信号量死锁。
    let _permit = ctx.io_permits.acquire().await.ok();

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

    /// 新测试的搭建辅助：内存 store + 单 feishu 实例（纯文本投递），
    /// 判死探针/节拍/取消令牌可注入。返回 (pool, adapter, sid)。
    async fn setup_pool(
        agent_dead: Box<dyn Fn(&SessionId) -> bool + Send + Sync>,
        settle_interval: std::time::Duration,
        idle_reap: std::time::Duration,
        token: CancellationToken,
    ) -> (DeliveryPool, Arc<MockAdapter>, SessionId) {
        let db = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        run_migrations(&db).await.unwrap();
        let store: Arc<dyn ChannelStore> = Arc::new(SqliteChannelStore::new(db));
        let sid = SessionId::from("sess_pool_test");
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

        let pool = DeliveryPool::with_timing(
            Arc::new(ObsTracker::new()),
            Arc::new(AskCardRegistry::new()),
            store,
            instances,
            std::sync::Weak::new(),
            agent_dead,
            token,
            settle_interval,
            idle_reap,
        );
        (pool, adapter, sid)
    }

    fn run_events(text: &str) -> Vec<Event> {
        vec![
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Running,
            }),
            Event::Model(ModelEvent::End {
                message_id: "m-1".into(),
                content: vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
            }),
            Event::Agent(AgentEvent::Lifecycle {
                state: AgentStatus::Stopped {
                    reason: StopReason::Completed {
                        finish_reason: None,
                    },
                },
            }),
        ]
    }

    async fn wait_delivered(adapter: &MockAdapter, text: &str, what: &str) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if adapter.sent.lock().await.iter().any(|t| t.contains(text)) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "{what}");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// M1/单表重构回归：actor 空闲回收后，下一个 run 必须由新 actor
    /// 正常投递（回收的摘除/交接不得误伤新登记——旗标与发送端同
    /// entry 原子建删）。
    #[tokio::test]
    async fn actor_reap_then_new_run_delivers() {
        let (pool, adapter, sid) = setup_pool(
            Box::new(|_| false),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(60),
            CancellationToken::new(),
        )
        .await;
        let routing = test_routing();

        for event in run_events("第一轮回复") {
            pool.dispatch(
                &sid,
                DeliveryJob {
                    routing: Arc::clone(&routing),
                    event,
                },
            );
        }
        wait_delivered(&adapter, "第一轮回复", "first run not delivered").await;

        // 越过 idle_reap：actor 被回收（登记摘除、通道关闭、退出）。
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(pool.is_quiet(&sid), "reaped actor reads quiet");
        assert!(!pool.has_buffer(&sid), "reaped actor holds no buffer");

        // 第二轮：全新 actor 接管并投递。
        for event in run_events("第二轮回复") {
            pool.dispatch(
                &sid,
                DeliveryJob {
                    routing: Arc::clone(&routing),
                    event,
                },
            );
        }
        wait_delivered(&adapter, "第二轮回复", "post-reap run not delivered").await;
    }

    /// M1 机制直接回归（复审 should-fix）：回收时队列余量必须经
    /// `redispatch_remaining` 重投给**新** actor——投递恰好一次（无
    /// 丢弃、无旧 actor 就地执行后的 ⏰ 重复兜底）。
    #[tokio::test]
    async fn reap_redispatch_hands_queued_jobs_to_fresh_actor() {
        let (pool, adapter, sid) = setup_pool(
            Box::new(|_| false),
            std::time::Duration::from_hours(1),
            std::time::Duration::from_hours(1),
            CancellationToken::new(),
        )
        .await;
        let routing = test_routing();

        // 模拟回收臂现场：登记已摘除（Vacant）、通道已 close、余量待重投。
        let (tx, mut rx) = mpsc::channel::<DeliveryJob>(SESSION_EVENT_CAPACITY);
        for event in run_events("重投回复") {
            tx.try_send(DeliveryJob {
                routing: Arc::clone(&routing),
                event,
            })
            .unwrap();
        }
        rx.close();

        redispatch_remaining(&pool.actors, &pool.ctx, &sid, &mut rx);

        wait_delivered(&adapter, "重投回复", "re-dispatched run not delivered").await;
        assert_eq!(
            adapter
                .sent
                .lock()
                .await
                .iter()
                .filter(|t| t.contains("重投回复"))
                .count(),
            1,
            "exactly one delivery — no drop, no duplicate Timeout settle"
        );
    }

    /// 三审 should-fix #2 回归：判死探针 panic 必须降级为"视为存活"
    /// ——actor 不死、buffer 不被误结算，后续事件正常投递。
    #[tokio::test]
    async fn panicking_probe_is_downgraded_to_alive() {
        let probe_calls = Arc::new(AtomicU32::new(0));
        let probe = {
            let calls = Arc::clone(&probe_calls);
            move |_: &SessionId| -> bool {
                calls.fetch_add(1, Ordering::Relaxed);
                panic!("injected probe panic")
            }
        };
        let (pool, adapter, sid) = setup_pool(
            Box::new(probe),
            std::time::Duration::from_millis(30),
            std::time::Duration::from_hours(1),
            CancellationToken::new(),
        )
        .await;
        let routing = test_routing();

        // ① Running 建 buffer；② 等巡检节拍触发探针（panic → 降级
        // 存活）；③ 补齐 End+Stopped：actor 必须活着并正常投递。
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: Arc::clone(&routing),
                event: Event::Agent(AgentEvent::Lifecycle {
                    state: AgentStatus::Running,
                }),
            },
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while probe_calls.load(Ordering::Relaxed) == 0 {
            assert!(
                std::time::Instant::now() < deadline,
                "watchdog tick never invoked the probe"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        for event in run_events("探针panic后仍投递").into_iter().skip(1) {
            pool.dispatch(
                &sid,
                DeliveryJob {
                    routing: Arc::clone(&routing),
                    event,
                },
            );
        }
        wait_delivered(&adapter, "探针panic后仍投递", "run lost after probe panic").await;
        assert!(
            pool.actors.contains_key(&sid),
            "actor must survive probe panics"
        );
    }

    /// N1 回归：actor 退出（取消路径）时登记必须被退出 guard 摘除——
    /// `is_quiet`/`has_buffer` 不得读到幽灵旗标，后续 dispatch 直接
    /// 拿到新 actor（panic 路径同此 Drop，Rust 保证 unwind 触发）。
    #[tokio::test]
    async fn actor_exit_cleans_registration() {
        let token = CancellationToken::new();
        let (pool, _adapter, sid) = setup_pool(
            Box::new(|_| false),
            std::time::Duration::from_hours(1),
            std::time::Duration::from_hours(1),
            token.clone(),
        )
        .await;

        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: test_routing(),
                event: Event::Agent(AgentEvent::Lifecycle {
                    state: AgentStatus::Running,
                }),
            },
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while !pool.actors.contains_key(&sid) {
            assert!(
                std::time::Instant::now() < deadline,
                "actor never registered"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        token.cancel();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            // 登记被摘除后：无 entry → has_buffer=false（幽灵旗标会
            // 给出 has_buffer=true）。
            if !pool.actors.contains_key(&sid) && !pool.has_buffer(&sid) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "exited actor left a ghost registration"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// S2 回归：并发会话数超过 IO 信号量（16）时全量投递仍完成——
    /// settle 的 permit 单点获取若退化为嵌套获取，此用例会死锁超时。
    #[tokio::test]
    async fn concurrent_sessions_all_deliver_under_io_cap() {
        const SESSIONS: usize = 20;
        let (pool, adapter, _sid) = setup_pool(
            Box::new(|_| false),
            std::time::Duration::from_millis(50),
            std::time::Duration::from_hours(1),
            CancellationToken::new(),
        )
        .await;
        let routing = test_routing();

        for i in 0..SESSIONS {
            // 每个会话独立的 mapping 键（同键 upsert 会互相覆盖，
            // settle 新鲜重读路由时前 19 个会话会查无路由）。
            let sid = SessionId::from(format!("sess_concurrent_{i}"));
            pool.ctx
                .store
                .save_mapping("feishu", &format!("chat-{i}"), &sid, "chat-1", None)
                .await
                .unwrap();
            for event in run_events(&format!("并发回复{i}")) {
                pool.dispatch(
                    &sid,
                    DeliveryJob {
                        routing: Arc::clone(&routing),
                        event,
                    },
                );
            }
        }
        for i in 0..SESSIONS {
            wait_delivered(&adapter, &format!("并发回复{i}"), "concurrent run lost").await;
        }
    }
}
