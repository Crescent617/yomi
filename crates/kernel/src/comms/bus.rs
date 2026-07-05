use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::event::Event;
use crate::types::SessionId;

const CMD_CHAN_SIZE: usize = 256;

/// 泛型发布-订阅通道。
///
/// 生产者通过 [`PubSubHandle`] 发送，消费者通过 [`PubSubSubscriber`] 接收。
/// 支持按 `K` 过滤的订阅和全局订阅。订阅与生命周期无关：guard Drop 时自动解注册。
///
/// 所有 subscriber 的 `recv()` 均返回 `(K, T)` 对，包括单 key 订阅和全局订阅。
pub struct PubSub<T, K> {
    event_tx: mpsc::Sender<(K, T)>,
    cmd_tx: mpsc::Sender<Command<T, K>>,
    forwarder: tokio::task::JoinHandle<()>,
}

impl<T, K> Drop for PubSub<T, K> {
    fn drop(&mut self) {
        self.forwarder.abort();
    }
}

impl<T, K> PubSub<T, K>
where
    K: Eq + std::hash::Hash + Clone + Send + 'static,
    T: Clone + Send + 'static,
{
    /// 创建总线。
    pub fn new() -> Arc<Self> {
        let (event_tx, event_rx) = mpsc::channel::<(K, T)>(256);
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command<T, K>>(CMD_CHAN_SIZE);

        let forwarder = tokio::spawn(run_forwarder(event_rx, cmd_rx));

        Arc::new(Self {
            event_tx,
            cmd_tx,
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

    /// 订阅单个 key 的消息。
    pub fn subscribe(&self, key: K) -> PubSubSubscriber<T, K> {
        let (tx, rx) = mpsc::channel::<(K, T)>(256);
        let id = next_listener_id();

        let listener = Listener { id, tx };
        if let Err(e) = self.cmd_tx.try_send(Command::SubscribeSession {
            session_id: key.clone(),
            listener,
        }) {
            tracing::error!(error = %e, "pubsub subscribe command dropped (channel full)");
        }

        PubSubSubscriber {
            cmd_tx: self.cmd_tx.clone(),
            id,
            session_id: Some(key),
            rx,
        }
    }

    /// 订阅所有消息。
    pub fn subscribe_all(&self) -> PubSubSubscriber<T, K> {
        let (tx, rx) = mpsc::channel::<(K, T)>(256);
        let id = next_listener_id();

        let listener = Listener { id, tx };
        if let Err(e) = self.cmd_tx.try_send(Command::SubscribeGlobal { listener }) {
            tracing::error!(error = %e, "pubsub subscribe_all command dropped (channel full)");
        }

        PubSubSubscriber {
            cmd_tx: self.cmd_tx.clone(),
            id,
            session_id: None,
            rx,
        }
    }

    /// 直接发送一条消息，无需先创建句柄。
    /// 返回 `Err(TrySendError::Full)` 当通道满时，调用者应处理或重试。
    pub fn publish(&self, key: K, payload: T) -> Result<(), mpsc::error::TrySendError<(K, T)>> {
        self.event_tx.try_send((key, payload))
    }
}

/// 生产者句柄。绑定 `key`，发送时不需要再传。
#[derive(Clone)]
pub struct PubSubHandle<K, T> {
    event_tx: mpsc::Sender<(K, T)>,
    key: K,
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
}

/// 消费者 Guard。Drop 时自动从 bus 解注册。
/// 所有 subscriber（单 key / 全局）统一返回 `(K, T)` 对。
pub struct PubSubSubscriber<T, K> {
    cmd_tx: mpsc::Sender<Command<T, K>>,
    id: u64,
    session_id: Option<K>,
    rx: mpsc::Receiver<(K, T)>,
}

/// Sentinel for bridge subscribers created via `from_receiver`.
/// They do not have a real listener in the `PubSub` forwarder.
const BRIDGE_LISTENER_ID: u64 = u64::MAX;

impl<T, K> PubSubSubscriber<T, K> {
    pub async fn recv(&mut self) -> Option<(K, T)> {
        self.rx.recv().await
    }

    /// 从外部 channel 构造 subscriber（用于远程模式桥接）。
    /// id 为 `BRIDGE_LISTENER_ID` 以避免与正常 listener id 冲突。
    pub fn from_receiver(rx: mpsc::Receiver<(K, T)>) -> Self {
        let (cmd_tx, _) = mpsc::channel(1);
        Self {
            cmd_tx,
            id: BRIDGE_LISTENER_ID,
            session_id: None,
            rx,
        }
    }
}

impl<T, K> Drop for PubSubSubscriber<T, K> {
    fn drop(&mut self) {
        // Bridge subscribers created via `from_receiver` do not have a real
        // listener in the PubSub forwarder, so there is nothing to
        // unsubscribe.
        if self.id == BRIDGE_LISTENER_ID {
            return;
        }
        let session_id = self.session_id.take();
        if let Err(e) = self.cmd_tx.try_send(Command::Unsubscribe {
            id: self.id,
            session_id,
        }) {
            tracing::warn!(
                "Failed to send unsubscribe command: {} (listener may leak)",
                e
            );
        }
    }
}

struct Listener<T, K> {
    id: u64,
    tx: mpsc::Sender<(K, T)>,
}

enum Command<T, K> {
    SubscribeSession {
        session_id: K,
        listener: Listener<T, K>,
    },
    SubscribeGlobal {
        listener: Listener<T, K>,
    },
    Unsubscribe {
        id: u64,
        session_id: Option<K>,
    },
}

fn next_listener_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

async fn run_forwarder<T, K>(
    mut event_rx: mpsc::Receiver<(K, T)>,
    mut cmd_rx: mpsc::Receiver<Command<T, K>>,
) where
    K: Eq + std::hash::Hash + Clone,
    T: Clone,
{
    let mut session_listeners: HashMap<K, Vec<Listener<T, K>>> = HashMap::new();
    let mut global_listeners: Vec<Listener<T, K>> = Vec::new();

    loop {
        tokio::select! {
            biased;

            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    Command::SubscribeSession { session_id, listener } => {
                        session_listeners
                            .entry(session_id)
                            .or_default()
                            .push(listener);
                    }
                    Command::SubscribeGlobal { listener } => {
                        global_listeners.push(listener);
                    }
                    Command::Unsubscribe { id, session_id } => {
                        if let Some(sid) = session_id {
                            if let Some(ls) = session_listeners.get_mut(&sid) {
                                ls.retain(|l| l.id != id);
                                if ls.is_empty() {
                                    session_listeners.remove(&sid);
                                }
                            }
                        } else {
                            global_listeners.retain(|l| l.id != id);
                        }
                    }
                }
            }

            Some((key, ev)) = event_rx.recv() => {
                // 发给该 key 的所有 listener
                if let Some(ls) = session_listeners.get_mut(&key) {
                    try_send_to_listeners(ls, &key, &ev);
                    if ls.is_empty() {
                        session_listeners.remove(&key);
                    }
                }

                // 发给所有全局 listener
                try_send_to_listeners(&mut global_listeners, &key, &ev);
            }

            else => break,
        }
    }
}

/// 尝试向 listener 列表发送事件，移除已关闭的 listener。
fn try_send_to_listeners<T, K>(listeners: &mut Vec<Listener<T, K>>, key: &K, ev: &T)
where
    K: Clone,
    T: Clone,
{
    let mut to_remove = Vec::new();
    for l in &*listeners {
        match l.tx.try_send((key.clone(), ev.clone())) {
            Err(mpsc::error::TrySendError::Closed(_)) => {
                to_remove.push(l.id);
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(
                    "EventBus channel full for listener {}, dropping event",
                    l.id
                );
            }
            Ok(()) => {}
        }
    }
    if !to_remove.is_empty() {
        let remove_set: std::collections::HashSet<_> = to_remove.into_iter().collect();
        listeners.retain(|l| !remove_set.contains(&l.id));
    }
}

// ── Type aliases ─────────────────────────────────────────────────────

pub type EventBus = PubSub<Event, SessionId>;
pub type EventBusHandle = PubSubHandle<SessionId, Event>;
pub type EventBusSubscriber = PubSubSubscriber<Event, SessionId>;
