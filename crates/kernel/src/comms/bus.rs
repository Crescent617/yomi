use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use dashmap::DashMap;
use tokio::sync::{mpsc, Notify};

use crate::types::SessionId;

/// 每个 listener 的默认队列容量。
const LISTENER_CAPACITY: usize = 256;

/// 丢件告警的最小间隔（每 listener）：丢件=数据丢失，必须可见；
/// 但洪峰时逐条 WARN 本身就是日志洪水（2026-08-21 事故：38 分钟
/// 16.9 万条重复 WARN，真正的丢件规模反而被淹没）。
const DROP_ALERT_INTERVAL_MS: u64 = 60_000;

/// 泛型发布-订阅通道。
///
/// 生产者通过 [`PubSubHandle`] 发送，消费者通过 [`PubSubSubscriber`] 接收。
/// 支持按 `K` 过滤的订阅和全局订阅。订阅与生命周期无关：guard Drop 时自动解注册。
///
/// 注册是同步的（直接写入共享注册表，不经中转任务）：`subscribe*` 返回后，
/// 之后才进入事件通道的消息保证能被该 subscriber 看到。事件本身仍由
/// forwarder 单线程按到达顺序派发，保持每个生产者的消息顺序。
///
/// 所有 subscriber 的 `recv()` 均返回 `(K, T)` 对，包括单 key 订阅和全局订阅。
pub struct PubSub<T, K> {
    event_tx: mpsc::Sender<(K, T)>,
    listeners: Arc<DashMap<u64, Listener<T, K>>>,
    /// Set on shutdown/drop: new subscriptions get an immediately-closed
    /// receiver, and all registered listener senders are dropped so
    /// existing subscribers see `recv() -> None` (the registry is shared
    /// with subscribers — without the explicit clear, their own Arc would
    /// keep the senders alive and `recv()` would pend forever).
    closed: Arc<AtomicBool>,
    shutdown: Arc<Notify>,
    forwarder: tokio::task::JoinHandle<()>,
}

impl<T, K> Drop for PubSub<T, K> {
    fn drop(&mut self) {
        self.forwarder.abort();
        self.closed.store(true, Ordering::Relaxed);
        self.listeners.clear();
    }
}

impl<T, K> PubSub<T, K>
where
    K: Eq + std::hash::Hash + Clone + Send + Sync + 'static,
    T: Clone + Send + Sync + 'static,
{
    /// 创建总线。
    pub fn new() -> Arc<Self> {
        let (event_tx, event_rx) = mpsc::channel::<(K, T)>(10_000);
        let listeners: Arc<DashMap<u64, Listener<T, K>>> = Arc::new(DashMap::new());
        let shutdown = Arc::new(Notify::new());

        let forwarder = tokio::spawn(run_forwarder(
            event_rx,
            Arc::clone(&listeners),
            Arc::clone(&shutdown),
        ));

        Arc::new(Self {
            event_tx,
            listeners,
            closed: Arc::new(AtomicBool::new(false)),
            shutdown,
            forwarder,
        })
    }

    /// 获取绑定到某个 key 的生产者句柄。
    pub fn handle(&self, key: K) -> PubSubHandle<K, T> {
        PubSubHandle {
            event_tx: self.event_tx.clone(),
            key,
        }
    }

    /// 订阅单个 key 的消息（默认接收全部）。
    pub fn subscribe(&self, key: K) -> PubSubSubscriber<T, K> {
        self.subscribe_filtered(key, |_| true)
    }

    /// 订阅单个 key 的消息，支持自定义过滤。
    pub fn subscribe_filtered<F>(&self, key: K, filter: F) -> PubSubSubscriber<T, K>
    where
        F: Fn(&T) -> bool + Send + Sync + 'static,
    {
        self.add_listener(Some(key), filter)
    }

    /// 订阅所有消息（默认接收全部）。
    pub fn subscribe_all(&self) -> PubSubSubscriber<T, K> {
        self.subscribe_all_filtered(|_| true)
    }

    /// 订阅所有消息，支持自定义过滤。
    pub fn subscribe_all_filtered<F>(&self, filter: F) -> PubSubSubscriber<T, K>
    where
        F: Fn(&T) -> bool + Send + Sync + 'static,
    {
        self.add_listener(None, filter)
    }

    /// 订阅所有消息（自定义过滤），并显式指定该 listener 的队列容量。
    ///
    /// 默认容量是 [`LISTENER_CAPACITY`]（256）：对高吞吐、且消费者可能
    /// 短暂阻塞在下游 IO 的关键订阅者（如渠道投递器），应显式调大，
    /// 把"突发丢件"换成"短暂排队"。
    pub fn subscribe_all_filtered_with_capacity<F>(
        &self,
        capacity: usize,
        filter: F,
    ) -> PubSubSubscriber<T, K>
    where
        F: Fn(&T) -> bool + Send + Sync + 'static,
    {
        self.add_listener_with_capacity(None, filter, capacity)
    }

    /// 同步注册一个 listener：`None` 为全局订阅。
    fn add_listener<F>(&self, session: Option<K>, filter: F) -> PubSubSubscriber<T, K>
    where
        F: Fn(&T) -> bool + Send + Sync + 'static,
    {
        self.add_listener_with_capacity(session, filter, LISTENER_CAPACITY)
    }

    /// 同步注册一个 listener（显式容量）。
    fn add_listener_with_capacity<F>(
        &self,
        session: Option<K>,
        filter: F,
        capacity: usize,
    ) -> PubSubSubscriber<T, K>
    where
        F: Fn(&T) -> bool + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel::<(K, T)>(capacity);
        let id = next_listener_id();
        if self.closed.load(Ordering::Relaxed) {
            // Bus already shut down: skip registration — `tx` drops here,
            // so the subscriber's first `recv()` returns None immediately.
            return PubSubSubscriber {
                listeners: Arc::clone(&self.listeners),
                id,
                rx,
            };
        }
        self.listeners.insert(
            id,
            Listener {
                id,
                session,
                tx,
                filter: Arc::new(filter),
                dropped: AtomicU64::new(0),
                last_drop_alert_ms: AtomicU64::new(0),
            },
        );
        if self.closed.load(Ordering::Relaxed) {
            // Raced with shutdown(): undo the registration — dropping the
            // listener (and its sender) makes the subscriber's first
            // recv() return None instead of pending forever.
            self.listeners.remove(&id);
        }

        PubSubSubscriber {
            listeners: Arc::clone(&self.listeners),
            id,
            rx,
        }
    }

    pub fn shutdown(&self) {
        self.closed.store(true, Ordering::Relaxed);
        // Drop all listener senders so subscribers see recv() -> None.
        self.listeners.clear();
        // notify_one 会留存一个 permit：即使 forwarder 还没在 await，
        // 它下一轮 select 也会立刻醒来退出。
        self.shutdown.notify_one();
    }

    /// 直接发送一条消息，无需先创建句柄。
    /// 返回 `Err(TrySendError::Full)` 当通道满时，调用者应处理或重试。
    pub fn publish(&self, key: K, payload: T) -> Result<(), mpsc::error::TrySendError<(K, T)>> {
        self.event_tx.try_send((key, payload))
    }

    /// 某个 listener 的累计丢件数（队列满被 drop 的事件总数）。诊断用。
    pub fn listener_dropped(&self, id: u64) -> Option<u64> {
        self.listeners
            .get(&id)
            .map(|l| l.dropped.load(Ordering::Relaxed))
    }
}

/// 生产者句柄。绑定 `key`，发送时不需要再传。
/// 手动实现 Clone：通道句柄的克隆与 `T` 无关，不应要求 `T: Clone`。
pub struct PubSubHandle<K, T> {
    event_tx: mpsc::Sender<(K, T)>,
    key: K,
}

impl<K: Clone, T> Clone for PubSubHandle<K, T> {
    fn clone(&self) -> Self {
        Self {
            event_tx: self.event_tx.clone(),
            key: self.key.clone(),
        }
    }
}

impl<K, T> PubSubHandle<K, T>
where
    K: Clone,
{
    pub async fn send(&self, event: T) -> Result<(), mpsc::error::SendError<(K, T)>> {
        self.event_tx.send((self.key.clone(), event)).await
    }

    #[allow(clippy::result_large_err)]
    pub fn try_send(&self, event: T) -> Result<(), mpsc::error::TrySendError<(K, T)>> {
        self.event_tx.try_send((self.key.clone(), event))
    }

    /// Create a guard that emits `event` when dropped, however the current
    /// scope exits. See [`EmitOnDrop`].
    pub fn emit_on_drop(&self, event: T) -> EmitOnDrop<K, T>
    where
        K: Send + Sync + 'static,
        T: Send + 'static,
    {
        EmitOnDrop::new(self.clone(), event)
    }
}

/// RAII guard that emits an event when dropped.
///
/// Interactive flows (permission prompts, ask-user questions) pair a request
/// event with a terminal ack; tying the ack to the guard's lifetime covers
/// every exit path — response, timeout, cancellation, task abort, panic —
/// exactly once, so subscribers never wait on a reply that will never come.
pub struct EmitOnDrop<K, T>
where
    K: Clone + Send + Sync + 'static,
    T: Send + 'static,
{
    handle: PubSubHandle<K, T>,
    event: Option<T>,
}

impl<K, T> EmitOnDrop<K, T>
where
    K: Clone + Send + Sync + 'static,
    T: Send + 'static,
{
    pub const fn new(handle: PubSubHandle<K, T>, event: T) -> Self {
        Self {
            handle,
            event: Some(event),
        }
    }
}

impl<K, T> Drop for EmitOnDrop<K, T>
where
    K: Clone + Send + Sync + 'static,
    T: Send + 'static,
{
    fn drop(&mut self) {
        let Some(event) = self.event.take() else {
            return;
        };
        match self.handle.try_send(event) {
            Ok(()) | Err(mpsc::error::TrySendError::Closed(_)) => {}
            // Channel momentarily full: hand off to a task that awaits
            // capacity so the terminal event is not lost with the producer.
            Err(mpsc::error::TrySendError::Full((_, event))) => {
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    let handle = self.handle.clone();
                    rt.spawn(async move {
                        let _ = handle.send(event).await;
                    });
                }
            }
        }
    }
}

/// 消费者 Guard。Drop 时自动从 bus 解注册。
/// 所有 subscriber（单 key / 全局）统一返回 `(K, T)` 对。
pub struct PubSubSubscriber<T, K> {
    listeners: Arc<DashMap<u64, Listener<T, K>>>,
    id: u64,
    rx: mpsc::Receiver<(K, T)>,
}

/// Sentinel for bridge subscribers created via `from_receiver`.
/// They do not have a real listener in the `PubSub` registry.
const BRIDGE_LISTENER_ID: u64 = u64::MAX;

impl<T, K> PubSubSubscriber<T, K> {
    pub async fn recv(&mut self) -> Option<(K, T)> {
        self.rx.recv().await
    }

    /// 该订阅者在 bus 注册表中的 id（配合 [`PubSub::listener_dropped`] 做诊断）。
    pub fn id(&self) -> u64 {
        self.id
    }

    /// 队列是否已排空。供 select! 守卫让 tick 分支让位于待处理事件。
    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }

    /// 从外部 channel 构造 subscriber（用于远程模式桥接）。
    /// id 为 `BRIDGE_LISTENER_ID` 以避免与正常 listener id 冲突。
    pub fn from_receiver(rx: mpsc::Receiver<(K, T)>) -> Self {
        Self {
            // 桥接 subscriber 不入注册表；空表只为满足字段类型。
            listeners: Arc::new(DashMap::new()),
            id: BRIDGE_LISTENER_ID,
            rx,
        }
    }
}

impl<T, K> Drop for PubSubSubscriber<T, K> {
    fn drop(&mut self) {
        // Bridge subscribers created via `from_receiver` do not have a real
        // listener in the registry, so there is nothing to unsubscribe.
        if self.id == BRIDGE_LISTENER_ID {
            return;
        }
        self.listeners.remove(&self.id);
    }
}

struct Listener<T, K> {
    id: u64,
    /// `None` = 全局订阅，接收所有 key。
    session: Option<K>,
    tx: mpsc::Sender<(K, T)>,
    filter: Arc<dyn Fn(&T) -> bool + Send + Sync>,
    /// 该 listener 队列满导致的累计丢件数（诊断/告警用）。
    dropped: AtomicU64,
    /// 上次丢件 ERROR 告警的时间戳（ms since epoch），用于限频。
    last_drop_alert_ms: AtomicU64,
}

fn now_millis() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn next_listener_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

async fn run_forwarder<T, K>(
    mut event_rx: mpsc::Receiver<(K, T)>,
    listeners: Arc<DashMap<u64, Listener<T, K>>>,
    shutdown: Arc<Notify>,
) where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
    T: Clone + Send + Sync,
{
    loop {
        tokio::select! {
            biased;

            () = shutdown.notified() => break,

            Some((key, ev)) = event_rx.recv() => {
                dispatch(&listeners, &key, &ev);
            }

            else => break,
        }
    }
}

/// 派发一条事件给所有匹配的 listener（该 key 的 + 全局的），应用 filter，
/// 并移除已关闭的 listener。
fn dispatch<T, K>(listeners: &DashMap<u64, Listener<T, K>>, key: &K, ev: &T)
where
    K: Eq + std::hash::Hash + Clone + Send + Sync,
    T: Clone + Send + Sync,
{
    let mut closed = Vec::new();
    for entry in listeners {
        let l = entry.value();
        if l.session.as_ref().is_some_and(|s| s != key) {
            continue;
        }
        if !(l.filter)(ev) {
            continue;
        }
        match l.tx.try_send((key.clone(), ev.clone())) {
            Err(mpsc::error::TrySendError::Closed(_)) => {
                closed.push(l.id);
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                let n = l.dropped.fetch_add(1, Ordering::Relaxed) + 1;
                let now_ms = now_millis();
                let last = l.last_drop_alert_ms.load(Ordering::Relaxed);
                if (n == 1 || now_ms.saturating_sub(last) >= DROP_ALERT_INTERVAL_MS)
                    && l.last_drop_alert_ms
                        .compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    // 丢件 = 数据丢失（回复投递事件也在其中）。ERROR 级、
                    // 按 listener 限频聚合并带累计数：第一次丢和之后每分钟
                    // 最多一条，洪峰不会刷日志，监控 grep ERROR 即可发现。
                    tracing::error!(
                        listener = l.id,
                        dropped_total = n,
                        "EventBus listener queue full, dropping events (data loss — consumer is too slow)"
                    );
                } else {
                    tracing::debug!(
                        listener = l.id,
                        dropped_total = n,
                        "EventBus event dropped (alert suppressed by rate limit)"
                    );
                }
            }
            Ok(()) => {}
        }
    }
    for id in closed {
        listeners.remove(&id);
    }
}

// ── Type aliases ─────────────────────────────────────────────────────

pub type EventBus = PubSub<crate::event::Envelope, SessionId>;
pub type EventBusHandle = PubSubHandle<SessionId, crate::event::Envelope>;
pub type EventBusSubscriber = PubSubSubscriber<crate::event::Envelope, SessionId>;
