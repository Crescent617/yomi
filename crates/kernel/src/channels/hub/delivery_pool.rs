//! 每会话投递 actor：把一个会话的**全部投递状态与平台 IO** 收拢到一个
//! 任务里。全局事件循环只做分派（见 `hub.rs`），不再碰任何网络调用。
//!
//! 背景（2026-08-21 `EventBus` 洪峰事故）：旧架构由单一全局循环内联
//! 执行所有会话的飞书 API 调用，消费速度被网络延迟锁死；91 会话并发
//! 时 bus 队列被打满，投递事件（含 `Stopped`/回复正文）被静默丢弃。
//!
//! 设计（2026-08-22 第二半场）：**actor 机制整体换芯为
//! `utils::keyed_pool` 共享池**（per-key FIFO、entry 锁内 dispatch、
//! 到达驱动 TTL、单 outstanding 未了账、`guarded_call` panic 网——语
//! 义与手绘版逐条对应，行为不变）；本文件只留投递业务：
//! - worker 状态 `S = Option<RunReplyBuffer>`（"run 在飞"标记），事件
//!   处理（记账/ask/obs/typing）与 `Stopped` 结算经 handler 钩；
//! - `Stopped` 被 bus 丢弃时，tick 钩判死兜底投递残余回复（延迟
//!   60s → 30s）；钩返回 `hold = buffer.is_some()`——**buffer 在飞
//!   ⟹ 本拍 TTL 摘牌让步**（防劈 run 第一防线，与手绘版 `else if`
//!   分支语义逐行等价）；
//! - panic 安全是**双层网**：本文件的 `guarded`（内层，panic 时
//!   buffer 按 `&mut` 原样保留，行为与手绘版一致）+ 池的
//!   `guarded_call`（机制兜底，近乎不可达；state 损失重置）；
//! - 判死探针在 `guarded` 之外自带 `catch_unwind` 降级"视为存活"
//!   （保守不杀 run）；
//! - 全局信号量限制并发平台 IO 上限，避免洪峰期 API 洪泛。

use std::sync::Arc;

use dashmap::{DashMap, DashSet};
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;
use tracing::{error, trace, warn};

use crate::event::{AgentEvent, AgentStatus, Event, ModelEvent, ToolEvent};
use crate::kernel::Kernel;
use crate::types::SessionId;

use crate::channels::ask::AskCardRegistry;
use crate::channels::hub::ChannelInstance;
use crate::channels::hub_deliver::{
    deliver_reply, notify_run_subscribers, RunEndStatus, SettleKind,
};
use crate::channels::obs::ObsTracker;
use crate::channels::reply::RunReplyBuffer;
use crate::channels::{ChannelStore, SessionRouting};
use crate::utils::keyed_pool::{panic_msg, Handler, KeyedPool, TickHook};

/// 每会话事件通道容量。与上游 bus 的全局队列（hub 侧 4096）相配：
/// 单会话 256 足够吸收正常突发，又限制洪峰期的内存占用（91 会话全满
/// ≈ 2.3 万事件）；真打满时 ERROR 告警（此时上游 bus 的丢件告警必然
/// 早已触发）。
const SESSION_EVENT_CAPACITY: usize = 256;

/// 全局并发平台 IO 上限（feishu 限流按 chat 计，worker 粒度≈chat 粒度，
/// 但总量仍需封顶以防 API 洪泛）。
const MAX_CONCURRENT_IO: usize = 16;

/// agent 死亡探测节拍：buffer 里有残余回复但 session 已不在运行
/// （`Stopped` 被 bus 丢弃），以 Timeout 形态兜底送出。与 TTL 摘牌
/// 共用同一节拍（池的 `tick_interval`）。
const SELF_SETTLE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);

/// worker 空闲 TTL：无 buffer 且闲置超过此时长即自我过期（池的锁内
/// 复核摘牌，见 `keyed_pool` 模块文档）。
const WORKER_IDLE_TTL: std::time::Duration = std::time::Duration::from_mins(15);

/// 派给会话 actor 的一条事件。`routing` 是分派时的快照（TTL 缓存，Arc
/// 共享避免每事件克隆），用于门禁、obs 展示以及 ask/typing 的目标定位
/// （≤2s 陈旧可接受：锚点漂移慢）；**投递路径（Deliver/兜底）总是新鲜
/// 重读**，回复锚点绝不用陈旧的（评审复核修正注释）。
pub(crate) struct DeliveryJob {
    pub(crate) routing: Arc<SessionRouting>,
    pub(crate) event: Event,
}

/// actor 共享上下文（机制归池，业务留此）。
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
    /// 持有未投递回复 buffer 的会话集合（obs 死亡清扫的让步谓词——
    /// 手绘版的 per-entry `has_buffer` 旗标换载体：旗标随 handler/
    /// tick 钩同步维护，worker 生灭与旗标一致性的论证见
    /// `sync_buffer_flag`）。
    buffer_holders: DashSet<SessionId>,
}

/// 会话 → 投递 actor 的分派器（机制在池，本结构只兜对外 API）。
#[derive(Clone)]
pub(crate) struct DeliveryPool {
    pool: KeyedPool<SessionId, DeliveryJob, Option<RunReplyBuffer>>,
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
        let ctx = Arc::new(DeliveryCtx {
            obs,
            ask,
            store,
            instances,
            kernel,
            agent_dead,
            io_permits: Semaphore::new(MAX_CONCURRENT_IO),
            buffer_holders: DashSet::new(),
        });
        // drain_on_cancel=false：投递是实时业务，关停时余量丢弃（与
        // 手绘版 cancel 即 break 同款；耐久语义属于持久化池）。
        let pool = KeyedPool::new(
            SESSION_EVENT_CAPACITY,
            settle_interval,
            idle_ttl,
            false,
            token,
            build_handler(&ctx),
            Some(build_tick_hook(&ctx)),
        );
        Self { pool, ctx }
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

    /// 该会话的投递是否安静（无 actor，或无未了账）。
    ///
    /// 供 obs 死亡清扫的判活谓词使用（发版终审 #2）：`Stopped` 发出时
    /// conductor 的状态镜像即刻翻为 Idle，但它可能还排在 actor 队列
    /// 里（IO 信号量后）或正在投递中——此时把卡片错冻成 ⏰ 会产生
    /// "既丢又成"的矛盾 UX。（池的 outstanding 口径 = queued + 在飞，
    /// 比手绘版的"队列空且无在飞"更严，判活方向保守。）
    pub(crate) fn is_quiet(&self, session_id: &SessionId) -> bool {
        self.pool.is_quiet(session_id)
    }

    /// 该会话的 actor 是否持有未投递的回复 buffer。清扫谓词对它让步：
    /// 卡片+回复的结算权威是 actor，清扫只收真正的孤儿卡。
    pub(crate) fn has_buffer(&self, session_id: &SessionId) -> bool {
        self.ctx.buffer_holders.contains(session_id)
    }

    /// 派一个事件给该会话的 worker（不存在则创建）。同会话严格
    /// FIFO；entry 锁内 spawn-or-send（机制见 `keyed_pool`）。
    pub(crate) fn dispatch(&self, session_id: &SessionId, job: DeliveryJob) {
        self.pool.dispatch(session_id, job);
    }

    /// 测试谓词：该会话的 worker 是否在册（过期/换代路径的观测缝，
    /// 手绘版直读 `actors` 的等价物）。
    #[cfg(test)]
    fn has_worker(&self, session_id: &SessionId) -> bool {
        self.pool.has_worker(session_id)
    }
}

/// buffer 持有旗标同步（handler/tick 钩每次返回后调用；tick 钩为
/// 无条件——池级 panic 重置后归位只此一路）：旗标表是业务侧对
/// "worker 状态外化"的载体（池的 S 不可触）。一致性论证：
/// - 正常生灭：旗标随每次钩返回同步，worker 摘牌只发生在 buffer
///   为空（hold=false）且 TTL 到期时——摘牌前旗标已是 remove 态；
/// - panic：内层 `guarded` 吞掉（buffer 保留、旗标照同步）；池级
///   `guarded_call` 吞掉时 state 重置为 None，下一拍 tick（≤一节
///   拍）把旗标归位 remove——残余窗口内清扫让步一次，保守方向；
/// - cancel：全池关停，清扫随之停止，旗标不再被读。
fn sync_buffer_flag(ctx: &DeliveryCtx, sid: &SessionId, buffer: Option<&RunReplyBuffer>) {
    if buffer.is_some() {
        ctx.buffer_holders.insert(sid.clone());
    } else {
        ctx.buffer_holders.remove(sid);
    }
}

/// handler 钩：事件处理（记账 → `Stopped` 结算 → ask/obs/typing）。
/// 内层 `guarded` 保持手绘版语义（panic 时 buffer 按 `&mut` 保留）。
fn build_handler(
    ctx: &Arc<DeliveryCtx>,
) -> Handler<SessionId, DeliveryJob, Option<RunReplyBuffer>> {
    let ctx = Arc::clone(ctx);
    Arc::new(move |sid, job, mut buffer| {
        let ctx = Arc::clone(&ctx);
        Box::pin(async move {
            guarded(
                &sid,
                "handle_event",
                handle_event(&sid, job, &mut buffer, &ctx),
            )
            .await;
            sync_buffer_flag(&ctx, &sid, buffer.as_ref());
            buffer
        })
    })
}

/// tick 钩（队列空时的巡检）：buffer 在飞且 agent 判死 → Timeout 兜
/// 底投递；`hold = buffer.is_some()` 是本拍摘牌的业务否决（防劈
/// run 第一防线——手绘版 `else if try_expire_self` 的等价表达）。
/// 判死探针在 `guarded` 之外自带 `catch_unwind` 降级"视为存活"
/// （panic 不杀 run，actor 不受牵连——三审 should-fix #2）。
fn build_tick_hook(ctx: &Arc<DeliveryCtx>) -> TickHook<SessionId, Option<RunReplyBuffer>> {
    let ctx = Arc::clone(ctx);
    Arc::new(move |sid, mut buffer| {
        let ctx = Arc::clone(&ctx);
        Box::pin(async move {
            if buffer.is_some() {
                // agent 已死但 Stopped 丢失：兜底投递（kernel 消失的
                // 误判由 agent_dead 探针内部挡住；瞬时失败时 buffer
                // 保留在原地，下个节拍继续重试）。
                let dead = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    (ctx.agent_dead)(&sid)
                }))
                .unwrap_or_else(|panic| {
                    error!(
                        session_id = %sid.0,
                        panic = %panic_msg(&panic),
                        "agent_dead probe panicked, treating session as alive"
                    );
                    false
                });
                if dead {
                    guarded(
                        &sid,
                        "settle",
                        settle_deliver(&sid, &mut buffer, SettleKind::Timeout, &ctx),
                    )
                    .await;
                }
            }
            // 无条件同步（复审 should-fix）：池级 panic 重置 state
            // 后唯有本钩每拍必经——旗标归位只此一路，残留窗口≤一拍。
            sync_buffer_flag(&ctx, &sid, buffer.as_ref());
            let hold = buffer.is_some();
            (buffer, hold)
        })
    })
}

/// 投递操作的统一 panic 安全网（复审残余项）：事件处理与巡检结算
/// 经此——panic 只留 ERROR，不杀 actor、不丢 buffer（`&mut` 传入
/// 按原样保留）。（判死探针在 `guarded` 之外，自带 `catch_unwind`
/// 降级为"视为存活"；`panic_msg` 与池的 `guarded_call` 共用一份。）
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
            let text = crate::channels::blocks_to_text(content);
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
    // （handle_event 内）与巡检兜底（tick 钩，无闸上下文）共
    // 用——闸只能在这里取；交给调用方各自取既漏掉兜底路径，又会与
    // 事件路径嵌套取第二张 permit，构成信号量死锁。
    let _permit = ctx.io_permits.acquire().await.ok();
    trace!(session_id = %session_id.0, "settle_deliver: permit acquired");

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
    trace!(session_id = %session_id.0, "settle_deliver: calling deliver_reply");
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
    trace!(
        session_id = %session_id.0,
        delivered = reply_msg_id.is_some(),
        "settle_deliver: deliver_reply returned"
    );
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
    adapter: Arc<dyn crate::channels::PlatformAdapter>,
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
#[path = "delivery_pool_test.rs"]
mod tests;
