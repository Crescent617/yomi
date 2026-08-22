//! 按键有序 worker 池（per-key FIFO worker pool）——`delivery_pool`
//! 与 `persist_pool` 共用的并发机制层（2026-08-22，hrli 定调"在
//! utils 抽共用并发库，两个地方复用"）。
//!
//! 语义（机制在此只此一份，persist/delivery 两处复用）：
//! - `dispatch`：**entry 锁内** spawn-or-send——同 key 严格 FIFO，跨
//!   key 天然并行；锁内零 await/零 IO/零二次 map 访问（不变式）；
//! - worker：每条 FIFO 通道一个任务，`S` 为随 worker 生灭的状态
//!   （`Default` 创建）；`handler(key, job, S) -> S` 值进值出串行调
//!   用（类型擦除下 `&mut S` 进不了 `'static` future，故 worker 把
//!   state 移交 future、await 后收回）；
//! - `on_tick`：节拍钩（队列空时触发，钩本身记在 outstanding
//!   账上）——业务侧的巡检/兜底挂点；返回的 `hold` 是本拍 TTL
//!   摘牌的业务否决（"还有活不过期"，如回复 buffer 在飞）；
//! - TTL 过期：到达驱动（dispatch 锁内刷新 `last_activity`），worker
//!   在**同一把锁内**复核零到达后摘牌退出——余量结构性不存在；
//! - `wait_idle`：排空屏障——等待该 key 未了计数（queued + 在飞，
//!   dispatch 时即记账，无观察窗口）归零；业务侧的"处理完 X 再读
//!   结果"不变式由它显式兑现，而不是靠时序猜；
//! - 关停：`drain_on_cancel` 选择 cancel 时排空已入队的活再走
//!   （持久化）还是立即退（投递）。
//!
//! panic/关停的猝死由 dispatch 的 Closed 分支原地换代兜底（观察即
//! 证据、锁即身份；近乎不可达——业务钩调用点统一裹
//! `catch_unwind`（`guarded_call`）：panic 降级为 ERROR、state
//! 重置，worker 存活、账照销，`wait_idle` 不挂）。

use std::hash::Hash;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use tokio::sync::{mpsc, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{error, warn};

/// 任务处理器：值进值出接管 state（见模块文档）。
pub(crate) type Handler<K, J, S> =
    Arc<dyn Fn(K, J, S) -> futures::future::BoxFuture<'static, S> + Send + Sync>;
/// 节拍钩：同 handler 的值进值出约定。返回 `(S, hold)`——
/// `hold=true` 时本拍 TTL 摘牌让步（业务侧的"还有活"防线，如
/// delivery 的 buffer 在飞不过期）。
pub(crate) type TickHook<K, S> =
    Arc<dyn Fn(K, S) -> futures::future::BoxFuture<'static, (S, bool)> + Send + Sync>;

struct PoolCtx<K, J, S> {
    handler: Handler<K, J, S>,
    on_tick: Option<TickHook<K, S>>,
    token: CancellationToken,
    tick_interval: Duration,
    idle_ttl: Duration,
    /// 关停语义：true = cancel 时排空已入队的活再退（持久化等耐
    /// 久场景）；false = 立即退（投递等实时场景，队列余量丢弃）。
    drain_on_cancel: bool,
    /// `wait_idle` 的唤醒点（每处理完一条/worker 退出时 notify）。
    idle_notify: Arc<Notify>,
}

struct WorkerEntry<J> {
    tx: mpsc::Sender<J>,
    /// 持有仅为测试可观察（`is_finished`）；drop 即 detach，不影响
    /// worker 存活，生产路径从不读它。
    #[allow(dead_code)]
    worker: tokio::task::JoinHandle<()>,
    last_activity: Instant,
    /// 未了计数（已派发未处理完 = queued + 在飞；dispatch 锁内先
    /// 记账再入队，worker 接手后由 guard 销账——任何瞬间都覆盖
    /// 全部未完成活，`is_quiet` 无观察窗口；worker 与 entry 共享）。
    outstanding: Arc<AtomicU32>,
}

/// 见模块文档。约束：`K: Eq + Hash + Clone + Send + Sync + 'static`，
/// `J: Send + 'static`，`S: Default + Send + 'static`。
pub(crate) struct KeyedPool<K, J, S> {
    workers: Arc<DashMap<K, WorkerEntry<J>>>,
    ctx: Arc<PoolCtx<K, J, S>>,
    channel_capacity: usize,
}

/// 共享全部内部状态（Arc）——clone 的是同一座池（derive 会强加了
/// `K/J/S: Clone`，实际上一个都不需要）。
impl<K, J, S> Clone for KeyedPool<K, J, S> {
    fn clone(&self) -> Self {
        Self {
            workers: Arc::clone(&self.workers),
            ctx: Arc::clone(&self.ctx),
            channel_capacity: self.channel_capacity,
        }
    }
}

impl<K, J, S> KeyedPool<K, J, S>
where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    J: Send + 'static,
    S: Default + Send + 'static,
{
    pub(crate) fn new(
        channel_capacity: usize,
        tick_interval: Duration,
        idle_ttl: Duration,
        drain_on_cancel: bool,
        token: CancellationToken,
        handler: Handler<K, J, S>,
        on_tick: Option<TickHook<K, S>>,
    ) -> Self {
        Self {
            workers: Arc::new(DashMap::new()),
            ctx: Arc::new(PoolCtx {
                handler,
                on_tick,
                token,
                tick_interval,
                idle_ttl,
                drain_on_cancel,
                idle_notify: Arc::new(Notify::new()),
            }),
            channel_capacity,
        }
    }

    /// 派一条任务给该 key 的 worker（不存在则创建）。**entry 锁内**
    /// spawn-or-send；先记未了账再入队（worker 侧只销不记，观察无
    /// 窗口）；队满销账记 ERROR 丢件（深度异常）；Closed 仅意味
    ///  worker 猝死（panic/关停——正常过期由 worker 自己持锁摘牌，
    /// 锁内永远不该见到 Closed）——销旧账、原地换代、记新账重投。
    pub(crate) fn dispatch(&self, key: &K, job: J) {
        use dashmap::mapref::entry::Entry;
        match self.workers.entry(key.clone()) {
            Entry::Occupied(mut e) => {
                e.get().outstanding.fetch_add(1, Ordering::SeqCst);
                match e.get().tx.try_send(job) {
                    Ok(()) => {
                        e.get_mut().last_activity = Instant::now();
                    }
                    Err(mpsc::error::TrySendError::Full(returned)) => {
                        e.get().outstanding.fetch_sub(1, Ordering::SeqCst);
                        error!("keyed pool: queue full, dropping job (deep overload)");
                        let _ = returned;
                    }
                    Err(mpsc::error::TrySendError::Closed(returned)) => {
                        e.get().outstanding.fetch_sub(1, Ordering::SeqCst);
                        if self.ctx.token.is_cancelled() {
                            // 关停中**不换代**（fresh-eyes 终审
                            // must-fix）：旧 worker 正 `rx.close()` 后
                            // drain——换代会让新 worker（同 token，
                            // 也进 drain 臂）与旧 worker 并发写同 key
                            // （persist 即两份 jsonl append 交错坏行）
                            // 且破 FIFO；旧 entry 被换走后其未了账
                            // 还会孤立于 `wait_all_idle`，排空承诺被
                            // 截断。保留 entry（旧账可见），丢件留痕。
                            error!("keyed pool: job lost — pool shutting down, not respawning");
                            let _ = returned;
                        } else {
                            warn!("keyed pool: worker died abnormally, respawning");
                            let fresh = self.spawn_entry(key.clone());
                            let tx = fresh.tx.clone();
                            let outstanding = Arc::clone(&fresh.outstanding);
                            *e.get_mut() = fresh;
                            send_accounted(&tx, &outstanding, returned);
                        }
                    }
                }
            }
            Entry::Vacant(e) => {
                if self.ctx.token.is_cancelled() {
                    // 关停中不新建（同 Closed 臂的 must-fix 论证：
                    // cancel 后 dispatch 源应已停，此路径本即异常）。
                    error!("keyed pool: job lost — pool shutting down, not spawning");
                    return;
                }
                let entry = self.spawn_entry(key.clone());
                let tx = entry.tx.clone();
                let outstanding = Arc::clone(&entry.outstanding);
                e.insert(entry);
                send_accounted(&tx, &outstanding, job);
            }
        }
    }

    fn spawn_entry(&self, key: K) -> WorkerEntry<J> {
        let (tx, rx) = mpsc::channel::<J>(self.channel_capacity);
        let outstanding = Arc::new(AtomicU32::new(0));
        WorkerEntry {
            tx,
            worker: tokio::spawn(run_worker(
                key,
                rx,
                Arc::clone(&self.workers),
                Arc::clone(&self.ctx),
                Arc::clone(&outstanding),
            )),
            last_activity: Instant::now(),
            outstanding,
        }
    }

    /// 该 key 是否安静（无 entry，或未了计数归零）。
    pub(crate) fn is_quiet(&self, key: &K) -> bool {
        self.workers
            .get(key)
            .is_none_or(|e| e.outstanding.load(Ordering::SeqCst) == 0)
    }

    /// 排空屏障：等待该 key 队列清空且无在飞任务。业务侧的"处理完
    /// 再读"不变式入口（如持久化的"Stopped 前必落盘"）。
    pub(crate) async fn wait_idle(&self, key: &K) {
        self.wait_until(|| self.is_quiet(key)).await;
    }

    /// 全池排空屏障（优雅关停用）：等所有 key 未了账归零。调用方
    /// 须先停 dispatch 源（如 conductor 停后再调），否则不保证收敛。
    pub(crate) async fn wait_all_idle(&self) {
        self.wait_until(|| {
            self.workers
                .iter()
                .all(|e| e.outstanding.load(Ordering::SeqCst) == 0)
        })
        .await;
    }

    /// 唤醒循环的共用底（Notify 快醒 + 5ms poll 兜底——通知可能错过）。
    async fn wait_until(&self, quiet: impl Fn() -> bool) {
        loop {
            if quiet() {
                return;
            }
            tokio::select! {
                () = self.ctx.idle_notify.notified() => {}
                () = tokio::time::sleep(Duration::from_millis(5)) => {}
            }
        }
    }

    /// 测试谓词：该 key 的 worker 是否在册。
    #[cfg(test)]
    pub(crate) fn has_worker(&self, key: &K) -> bool {
        self.workers.contains_key(key)
    }

    /// 测试缝：强杀该 key 的 worker（`abort`）——构造真实的
    /// Closed 通道（猝死路径在生产被双层 panic 网兜到近乎不可达，
    /// 换代 Ok 臂只能这样注入；复审 should-fix）。
    #[cfg(test)]
    pub(crate) fn abort_worker(&self, key: &K) {
        if let Some(e) = self.workers.get(key) {
            e.worker.abort();
        }
    }

    #[cfg(test)]
    pub(crate) fn worker_count(&self) -> usize {
        self.workers.len()
    }
}

/// 锁内"记账→入队"（dispatch 的 Vacant/Closed 换代分支共用）：
/// 先记未了账再发送；失败销账留 ERROR（仅关停边缘可达——新通
/// 道即闭，如 pool 已 cancel）。
fn send_accounted<J>(tx: &mpsc::Sender<J>, outstanding: &Arc<AtomicU32>, job: J) {
    outstanding.fetch_add(1, Ordering::SeqCst);
    if tx.try_send(job).is_err() {
        outstanding.fetch_sub(1, Ordering::SeqCst);
        error!("keyed pool: job lost — worker queue already closed (pool shutting down?)");
    }
}

/// 未了计数 guard：Drop 自减 + 唤醒 `wait_idle`（panic 安全）。
struct OutstandingGuard(Arc<AtomicU32>, Arc<Notify>);

impl OutstandingGuard {
    /// 记账新建（+1）——tick 钩等就地起活用。
    fn counted(counter: &Arc<AtomicU32>, notify: &Arc<Notify>) -> Self {
        counter.fetch_add(1, Ordering::SeqCst);
        Self(Arc::clone(counter), Arc::clone(notify))
    }

    /// 销账接手（不 +1）——dispatch 已记账，worker 接过销账职责。
    fn handed(counter: &Arc<AtomicU32>, notify: &Arc<Notify>) -> Self {
        Self(Arc::clone(counter), Arc::clone(notify))
    }
}

impl Drop for OutstandingGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
        self.1.notify_one();
    }
}

/// panic 载荷提取（delivery 的 `guarded` 与本池 `guarded_call` 共
/// 用——`Box<dyn Any>` 的 Debug 只打 `Any { .. }`，丢失 panic 消息）。
pub(crate) fn panic_msg(panic: &dyn std::any::Any) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// 业务钩调用点的统一 panic 安全网（2026-08-22 复审 must-fix）：
/// panic 只留 ERROR、返回 `None`（调用侧 state 保持 `take` 后的
/// `Default`，即损失重置），worker 存活——一个 key 的恐慌不拖死
/// worker；账由 `OutstandingGuard` 照销，`wait_idle` 不挂。
async fn guarded_call<S>(
    label: &'static str,
    fut: impl std::future::Future<Output = S>,
) -> Option<S> {
    use futures::FutureExt as _;
    match std::panic::AssertUnwindSafe(fut).catch_unwind().await {
        Ok(state) => Some(state),
        Err(panic) => {
            error!(
                panic = %panic_msg(&*panic),
                label,
                "keyed pool: hook panicked, state reset and worker continues"
            );
            None
        }
    }
}

async fn run_worker<K, J, S>(
    key: K,
    mut rx: mpsc::Receiver<J>,
    workers: Arc<DashMap<K, WorkerEntry<J>>>,
    ctx: Arc<PoolCtx<K, J, S>>,
    own_outstanding: Arc<AtomicU32>,
) where
    K: Eq + Hash + Clone + Send + Sync + 'static,
    J: Send + 'static,
    S: Default + Send + 'static,
{
    let mut state = S::default();
    let mut tick = tokio::time::interval(ctx.tick_interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            () = ctx.token.cancelled() => {
                if ctx.drain_on_cancel {
                    // 先关门再排（复审 should-fix）：迟到 send 落
                    // Closed 由 dispatch 换代记账，杜绝"排空后悄悄
                    // 进队"的账漏（wait_idle 永等同型）。
                    rx.close();
                    // 关停排空：已记账入队的活做完再走（耐久场景）。
                    while let Ok(job) = rx.try_recv() {
                        let _done =
                            OutstandingGuard::handed(&own_outstanding, &ctx.idle_notify);
                        if let Some(s) = guarded_call(
                            "handler",
                            (ctx.handler)(key.clone(), job, std::mem::take(&mut state)),
                        )
                        .await
                        {
                            state = s;
                        }
                    }
                }
                break;
            }
            _ = tick.tick() => {
                if !rx.is_empty() {
                    continue;
                }
                let mut hold = false;
                if let Some(on_tick) = &ctx.on_tick {
                    // tick 钩本身记在账上：`wait_idle` 与 TTL 过期
                    // 的"安静"含钩内活。
                    let _busy =
                        OutstandingGuard::counted(&own_outstanding, &ctx.idle_notify);
                    if let Some((s, h)) = guarded_call(
                        "tick hook",
                        on_tick(key.clone(), std::mem::take(&mut state)),
                    )
                    .await
                    {
                        state = s;
                        hold = h;
                    }
                    // 钩 panic（state 已重置）：hold 保持 false，过期
                    // 照 TTL 走——钩反复 panic 时宁可摘牌也不留漏。
                }
                if !hold && try_expire(&key, &workers, ctx.idle_ttl) {
                    // 摘牌退出：entry 已删，wait_idle/is_quiet 自然安静。
                    ctx.idle_notify.notify_one();
                    return;
                }
            }
            job = rx.recv() => {
                let Some(job) = job else { break };
                let _done = OutstandingGuard::handed(&own_outstanding, &ctx.idle_notify);
                if let Some(s) = guarded_call(
                    "handler",
                    (ctx.handler)(key.clone(), job, std::mem::take(&mut state)),
                )
                .await
                {
                    state = s;
                }
            }
        }
    }
    ctx.idle_notify.notify_one();
}

/// TTL 复核（entry 锁内）：确超 TTL 且无未了账才摘牌（返回 true）。
/// 复核通过 ⟺ TTL 全程零到达 ⟺ 队列必空——余量结构性不存在。
fn try_expire<K, J>(key: &K, workers: &Arc<DashMap<K, WorkerEntry<J>>>, idle_ttl: Duration) -> bool
where
    K: Eq + Hash + Clone,
{
    use dashmap::mapref::entry::Entry;
    let Entry::Occupied(e) = workers.entry(key.clone()) else {
        return true; // 已不在表中：无牵无挂
    };
    if e.get().last_activity.elapsed() < idle_ttl || e.get().outstanding.load(Ordering::SeqCst) != 0
    {
        return false;
    }
    e.remove();
    true
}

#[cfg(test)]
#[path = "keyed_pool_test.rs"]
mod tests;
