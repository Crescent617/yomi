//! 每会话投递 actor：把一个会话的**全部投递状态与平台 IO** 收拢到一个
//! 任务里。全局事件循环只做分派（见 `hub.rs`），不再碰任何网络调用。
//!
//! 背景（2026-08-21 `EventBus` 洪峰事故）：旧架构由单一全局循环内联
//! 执行所有会话的飞书 API 调用，消费速度被网络延迟锁死；91 会话并发
//! 时 bus 队列被打满，投递事件（含 `Stopped`/回复正文）被静默丢弃。
//!
//! 设计（actor 模型 + Redis TTL 式过期，2026-08-22 定稿）：
//! - 每个经路由的会话拥有**一个** worker，事件经一条 FIFO 通道按序
//!   到达；reply buffer、obs/ask/typing、回复投递全部在 worker 内
//!   闭环——同会话保序与旧单循环语义完全一致，跨会话天然并行；
//! - agent 死亡但 `Stopped` 丢失（bus 丢件）时，worker 靠自己的
//!   巡检节拍兜底投递残余回复（替代旧的全局 watchdog 回复扫描，
//!   延迟 60s → 30s）；判死前确认自身队列为空，杜绝与在队
//!   `Stopped` 的双重结算；
//! - **过期回收（惰性+主动，Redis TTL 同款双机制）**：TTL 由事件
//!   到达驱动（dispatch 在 entry 锁内刷新 `last_activity`）；worker
//!   闲置超 TTL 时在**同一把锁内**复核确认零到达后摘牌退出——余
//!   量结构性不存在（复核通过 ⟺ TTL 全程零到达），故无换代分支，
//!   摘了就走、下次 dispatch 惰性重建。dispatch 的 `try_send` 同在
//!   此锁内（同步 µs 段，零 await），重试循环/身份比对/re-dispatch
//!   全部成为过去式（**临界区不变式：锁内零 await、零 IO、零二次
//!   map 访问**）。panic/关停的猝死由 dispatch 的 Closed 分支原地
//!   换代兜底（近乎不可达，余量随 rx 丢弃、有日志），janitor 周期
//!   收尸（旗标留痕+摘 entry，纯保洁）；
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

/// worker 空闲 TTL：无 buffer 且闲置超过此时长即自我过期（锁内了断，
/// 见模块文档）。
const WORKER_IDLE_TTL: std::time::Duration = std::time::Duration::from_mins(15);

/// janitor 收尸节拍（异常死亡 worker 的旗标复位与 entry 清理——纯保
/// 洁，正确性不依赖它）。
const JANITOR_INTERVAL: std::time::Duration = std::time::Duration::from_mins(1);

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
    /// 巡检节拍（测试可缩短）。
    settle_interval: std::time::Duration,
    /// worker 空闲 TTL（测试可缩短）。
    idle_ttl: std::time::Duration,
}

/// 每会话 actor 登记：发送端、worker 句柄与对外旗标一体。
struct ActorHandle {
    tx: mpsc::Sender<DeliveryJob>,
    /// worker 任务句柄（janitor 的 `is_finished` 收尸谓词用；换代即
    /// 换新的，旧句柄 drop 仅 detach）。
    worker: tokio::task::JoinHandle<()>,
    /// 最后一次事件到达时刻（**仅在 entry 锁内读写，无需原子**）：
    /// dispatch 投件成功时刷新；worker 过期判定在同一把锁内复核——
    /// 复核通过 ⟺ TTL 全程零到达 ⟺ 队列必然为空，余量结构性不存在。
    last_activity: std::time::Instant,
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
            WORKER_IDLE_TTL,
        )
    }

    /// 完整构造（判死探针与节拍可注入——测试用短节拍覆盖巡检/过期路径）。
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
        idle_ttl: std::time::Duration,
    ) -> Self {
        let pool = Self {
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
                idle_ttl,
            }),
        };
        pool.spawn_janitor();
        pool
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

    /// 派一个事件给该会话的 worker（不存在则创建）。同会话严格 FIFO。
    ///
    /// **全程在 entry 锁内**（同步 µs 段，零 await）：`try_send` 成功即
    /// 止；队满记 ERROR 丢件（深度异常，上游 bus 告警必然先触发）；
    /// Closed 仅意味着 worker 猝死（panic/关停——正常过期由 worker
    /// 自己持锁摘牌退出，锁内永远不该见到 Closed）——观察即证据，
    /// 锁即身份，原地换代重投，无需重试与身份比对。
    pub(crate) fn dispatch(&self, session_id: &SessionId, job: DeliveryJob) {
        use dashmap::mapref::entry::Entry;
        match self.actors.entry(session_id.clone()) {
            Entry::Occupied(mut e) => match e.get().tx.try_send(job) {
                Ok(()) => {
                    // 到达即活动：刷新 TTL（锁内普通字段写）。
                    e.get_mut().last_activity = std::time::Instant::now();
                }
                Err(mpsc::error::TrySendError::Full(returned)) => {
                    error!(
                        session_id = %session_id.0,
                        "session delivery queue full, dropping event (deep overload)"
                    );
                    let _ = returned;
                }
                Err(mpsc::error::TrySendError::Closed(returned)) => {
                    warn!(
                        session_id = %session_id.0,
                        "delivery worker died abnormally, respawning (queued events lost)"
                    );
                    *e.get_mut() = spawn_handle(&self.actors, &self.ctx, session_id.clone());
                    // 新通道必收（空队列、容量满额）。
                    let _ = e.get().tx.try_send(returned);
                }
            },
            Entry::Vacant(e) => {
                let handle = spawn_handle(&self.actors, &self.ctx, session_id.clone());
                let tx = handle.tx.clone();
                e.insert(handle);
                // 同上：新通道必收。
                let _ = tx.try_send(job);
            }
        }
    }

    /// janitor 单趟收尸（测试可直接调用）：摘除 worker 已死亡的
    /// entry（panic/关停的尸体；正常过期的 worker 已在锁内自我了
    /// 断，不会产生尸体）。脏旗标随 entry 一并消失——此处留痕是为
    /// 了 forensic（幻影 buffer 曾让 obs 清扫永远让步的风险随摘除
    /// 关闭）。本函数纯属保洁，正确性不依赖它。
    pub(crate) fn janitor_sweep(&self) {
        self.actors.retain(|sid, h| {
            if !h.worker.is_finished() {
                return true;
            }
            if h.has_buffer.load(Ordering::Relaxed) || h.inflight.load(Ordering::Relaxed) != 0 {
                warn!(
                    session_id = %sid.0,
                    "janitor: collecting a dead delivery worker with stale flags"
                );
            }
            false
        });
    }

    /// janitor 周期任务（Redis 主动过期同款：慢节奏、纯内存保洁）。
    fn spawn_janitor(&self) {
        let token = self.ctx.token.clone();
        let pool = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(JANITOR_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    () = token.cancelled() => break,
                    _ = tick.tick() => pool.janitor_sweep(),
                }
            }
        });
    }
}

/// 建一条会话通道并拉起 worker，返回完整登记。调用方必须持有
/// entry 锁（spawn 是同步 fire-and-forget，不违例）。
fn spawn_handle(
    actors: &Arc<DashMap<SessionId, ActorHandle>>,
    ctx: &Arc<DeliveryCtx>,
    session_id: SessionId,
) -> ActorHandle {
    let (tx, rx) = mpsc::channel::<DeliveryJob>(SESSION_EVENT_CAPACITY);
    let worker = {
        let ctx = Arc::clone(ctx);
        let actors = Arc::clone(actors);
        let inflight = Arc::new(AtomicU32::new(0));
        let has_buffer = Arc::new(AtomicBool::new(false));
        let h_inflight = Arc::clone(&inflight);
        let h_buffer = Arc::clone(&has_buffer);
        let worker = tokio::spawn(async move {
            run_worker(session_id, rx, actors, inflight, has_buffer, ctx).await;
        });
        ActorHandle {
            tx,
            worker,
            last_activity: std::time::Instant::now(),
            inflight: h_inflight,
            has_buffer: h_buffer,
        }
    };
    worker
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

/// 会话投递 worker 主循环。
async fn run_worker(
    session_id: SessionId,
    mut rx: mpsc::Receiver<DeliveryJob>,
    actors: Arc<DashMap<SessionId, ActorHandle>>,
    own_inflight: Arc<AtomicU32>,
    own_has_buffer: Arc<AtomicBool>,
    ctx: Arc<DeliveryCtx>,
) {
    // 本 run 的回复缓冲：存在即"run 在飞"的标记（与原全局循环的
    // reply_buffers 语义一致），在 Stopped/兜底时取走投递。
    let mut buffer: Option<RunReplyBuffer> = None;
    let mut settle_tick = tokio::time::interval(ctx.settle_interval);
    settle_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            () = ctx.token.cancelled() => break,
            _ = settle_tick.tick() => {
                // 队列里还有未处理事件时，判死与过期都让步——Stopped
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
                } else if try_expire_self(&session_id, &mut rx, &actors, ctx.idle_ttl) {
                    // 空闲 TTL 到期且锁内复核确认零到达：entry 已摘，
                    // 本 worker 退出（设计定稿见模块文档）。
                    return;
                }
            }
            job = rx.recv() => {
                let Some(job) = job else { break };
                let _busy = InflightGuard::new(&own_inflight);
                run_job(&session_id, job, &mut buffer, &ctx).await;
                own_has_buffer.store(buffer.is_some(), Ordering::Relaxed);
            }
        }
    }
}

/// worker 过期判定：**entry 锁内**复核 `last_activity` 确超 TTL 才
/// 摘除 entry（返回 true）。TTL 由事件到达驱动（dispatch 在同一把
/// 锁内刷新）——复核通过 ⟺ TTL 全程零到达 ⟺ 队列必然为空，余量
/// 结构性不存在（故无换代分支：摘了就走，下次 dispatch 惰性重建）。
/// 全程同步（不变式：锁内零 await、零 IO、零二次 map 访问）。
fn try_expire_self(
    session_id: &SessionId,
    rx: &mut mpsc::Receiver<DeliveryJob>,
    actors: &Arc<DashMap<SessionId, ActorHandle>>,
    idle_ttl: std::time::Duration,
) -> bool {
    use dashmap::mapref::entry::Entry;
    let Entry::Occupied(e) = actors.entry(session_id.clone()) else {
        return true; // 已不在表中：无牵无挂，退出
    };
    if e.get().last_activity.elapsed() < idle_ttl {
        return false; // 期间有事件到达：放弃过期，继续服务
    }
    // 防御：余量按构造必空（复核通过 ⟺ 零到达）；若有，必是设计不
    // 变量被破坏——留痕并照常摘除（rx 随 worker 退出丢弃，warn 可见）。
    if rx.try_recv().is_ok() {
        warn!(
            session_id = %session_id.0,
            "expiry found leftover despite fresh-TTL re-check (invariant broken?)"
        );
    }
    e.remove();
    true
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
/// 经此——panic 只留 ERROR，不杀 actor、不卡死标记。（判死探针在
/// `guarded` 之外，自带 `catch_unwind` 降级为"视为存活"。）
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
    // （handle_event 内）与巡检兜底（tick 臂，无闸上下文）共
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
    /// 判死探针/节拍/TTL/取消令牌可注入。返回 (pool, adapter, sid)。
    async fn setup_pool(
        agent_dead: Box<dyn Fn(&SessionId) -> bool + Send + Sync>,
        settle_interval: std::time::Duration,
        idle_ttl: std::time::Duration,
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
            idle_ttl,
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

    /// 过期设计回归（2026-08-22 定稿）：闲置超 TTL 的 worker 自我过
    /// 期（entry 摘除，真回收）；后续 dispatch 惰性重建，正常投递。
    #[tokio::test]
    async fn idle_worker_expires_and_respawns_on_demand() {
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

        // 越过 TTL+数个节拍：worker 已自我过期，entry 被摘除。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while pool.actors.contains_key(&sid) {
            assert!(
                std::time::Instant::now() < deadline,
                "idle worker was not expired"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(pool.is_quiet(&sid), "expired session reads quiet");

        // 第二轮：dispatch 惰性重建 worker 并正常投递。
        for event in run_events("第二轮回复") {
            pool.dispatch(
                &sid,
                DeliveryJob {
                    routing: Arc::clone(&routing),
                    event,
                },
            );
        }
        wait_delivered(&adapter, "第二轮回复", "post-expiry run not delivered").await;
    }

    /// TTL 由事件到达驱动：判定节拍之间有事件到达（即便不产生
    /// buffer 的事件），worker 不得过期——entry 时间戳在锁内被刷
    /// 新，过期复核必然放弃。
    #[tokio::test]
    async fn event_arrival_defers_expiry() {
        let (pool, adapter, sid) = setup_pool(
            Box::new(|_| false),
            std::time::Duration::from_millis(20),
            std::time::Duration::from_millis(120),
            CancellationToken::new(),
        )
        .await;
        let routing = test_routing();

        // 每 50ms 来一条不产生 buffer 的事件（ModelEvent::Request），
        // 总时长 300ms 远超 TTL=120ms——worker 必须始终存活。
        for _ in 0..6 {
            pool.dispatch(
                &sid,
                DeliveryJob {
                    routing: Arc::clone(&routing),
                    event: Event::Model(ModelEvent::Request {
                        message_id: crate::types::MessageId::new(),
                        message_count: 1,
                    }),
                },
            );
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(
            pool.actors.contains_key(&sid),
            "worker expired despite fresh event arrivals"
        );

        // 静默超过 TTL：必须过期。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while pool.actors.contains_key(&sid) {
            assert!(
                std::time::Instant::now() < deadline,
                "worker not expired after silence"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let _ = adapter;
    }

    /// Closed 分支（worker 猝死的保险丝）：entry 残留死通道时，
    /// dispatch 原地换代并重投——投递不受影响。
    #[tokio::test]
    async fn closed_branch_respawns_after_abnormal_death() {
        let (pool, adapter, sid) = setup_pool(
            Box::new(|_| false),
            std::time::Duration::from_hours(1),
            std::time::Duration::from_hours(1),
            CancellationToken::new(),
        )
        .await;
        let routing = test_routing();

        // 植入尸体：tx 存活但 rx 已弃（channel 立闭），worker 句柄已完结。
        let (tx, rx) = mpsc::channel::<DeliveryJob>(SESSION_EVENT_CAPACITY);
        drop(rx);
        pool.actors.insert(
            sid.clone(),
            ActorHandle {
                tx,
                worker: tokio::spawn(async {}),
                last_activity: std::time::Instant::now(),
                inflight: Arc::new(AtomicU32::new(0)),
                has_buffer: Arc::new(AtomicBool::new(false)),
            },
        );

        for event in run_events("换代后投递") {
            pool.dispatch(
                &sid,
                DeliveryJob {
                    routing: Arc::clone(&routing),
                    event,
                },
            );
        }
        wait_delivered(&adapter, "换代后投递", "run not delivered after respawn").await;
        // 换代后 entry 的时间戳必须是新的（spawn_handle 创建即写入），
        // 新 worker 不会因陈旧时间戳立刻过期（复审 should-fix pin）。
        let fresh = pool
            .actors
            .get(&sid)
            .is_some_and(|h| h.last_activity.elapsed() < std::time::Duration::from_secs(1));
        assert!(fresh, "respawned worker has a stale last_activity");
        // janitor 不得误收活 worker。
        pool.janitor_sweep();
        assert!(pool.actors.contains_key(&sid));
    }

    /// janitor 收尸：worker 已完结的 entry（含脏旗标）被摘除；活
    /// worker 不动。
    #[tokio::test]
    async fn janitor_collects_corpses_keeps_living() {
        let (pool, _adapter, sid) = setup_pool(
            Box::new(|_| false),
            std::time::Duration::from_hours(1),
            std::time::Duration::from_hours(1),
            CancellationToken::new(),
        )
        .await;

        // 活 worker（正常 dispatch 建）。
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
                "worker never registered"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // 尸体：另一个 session，worker 已完结且旗标脏。
        let corpse = SessionId::from("sess_corpse");
        // 植入尸体：tx 存活但 rx 已弃（channel 立闭），worker 句柄已完结。
        let (tx, rx) = mpsc::channel::<DeliveryJob>(SESSION_EVENT_CAPACITY);
        drop(rx);
        let corpse_worker = tokio::spawn(async {});
        while !corpse_worker.is_finished() {
            tokio::task::yield_now().await;
        }
        pool.actors.insert(
            corpse.clone(),
            ActorHandle {
                tx,
                worker: corpse_worker,
                last_activity: std::time::Instant::now(),
                inflight: Arc::new(AtomicU32::new(2)),
                has_buffer: Arc::new(AtomicBool::new(true)),
            },
        );

        pool.janitor_sweep();
        assert!(
            !pool.actors.contains_key(&corpse),
            "corpse entry must be collected"
        );
        assert!(
            pool.actors.contains_key(&sid),
            "living worker must survive the janitor"
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

    /// 关停路径回归：token 取消后 worker 退出（句柄完结），尸体由
    /// janitor 收殓；其后的 dispatch 重建登记（此 pool 的 token 已
    /// 死，新 worker 即生即灭属预期——此处只钉"收尸→重建"链路）。
    #[tokio::test]
    async fn cancelled_worker_is_collected_and_respawned() {
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
                "worker never registered"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        token.cancel();

        // worker 退出（句柄完结）；entry 尚在（尸体），janitor 收殓。
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let finished = pool.actors.get(&sid).is_none_or(|h| h.worker.is_finished());
            if finished {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "worker did not exit on cancel"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        pool.janitor_sweep();
        assert!(
            !pool.actors.contains_key(&sid),
            "janitor must collect the cancelled worker's corpse"
        );

        // 后续 dispatch 经 Vacant 路径重建登记。
        pool.dispatch(
            &sid,
            DeliveryJob {
                routing: test_routing(),
                event: Event::Agent(AgentEvent::Lifecycle {
                    state: AgentStatus::Running,
                }),
            },
        );
        assert!(
            pool.actors.contains_key(&sid),
            "dispatch must respawn after corpse collection"
        );
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
