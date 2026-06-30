use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::event::Event;
use crate::types::SessionId;

const CMD_CHAN_SIZE: usize = 256;

/// 全局事件总线。
///
/// 所有 Agent / Session 事件统一走这里。生产者通过 [`EventBusHandle`] 发送，
/// 消费者通过 [`EventBusSubscriber`] 接收。订阅与 session 生命周期无关：
/// 任何时候都可以订阅任意 session，guard Drop 时自动解注册。
///
/// 所有 subscriber 的 `recv()` 均返回 `(SessionId, Event)` 对，
/// 包括单 session 订阅和全局订阅。
pub struct EventBus {
    event_tx: mpsc::Sender<(SessionId, Event)>,
    cmd_tx: mpsc::Sender<Command>,
    _forwarder: tokio::task::JoinHandle<()>,
}

impl EventBus {
    /// 创建总线。
    pub fn new() -> Arc<Self> {
        let (event_tx, event_rx) = mpsc::channel::<(SessionId, Event)>(256);
        let (cmd_tx, cmd_rx) = mpsc::channel::<Command>(CMD_CHAN_SIZE);

        let _forwarder = tokio::spawn(run_forwarder(event_rx, cmd_rx));

        Arc::new(Self {
            event_tx,
            cmd_tx,
            _forwarder,
        })
    }

    /// 获取某个 session 的生产者句柄。
    pub fn handle(&self, session_id: SessionId) -> EventBusHandle {
        EventBusHandle {
            event_tx: self.event_tx.clone(),
            session_id,
        }
    }

    /// 订阅单个 session 的事件。
    pub fn subscribe(&self, session_id: SessionId) -> EventBusSubscriber {
        let (tx, rx) = mpsc::channel::<(SessionId, Event)>(256);
        let id = next_listener_id();

        let listener = Listener { id, tx };
        if let Err(e) = self.cmd_tx.try_send(Command::SubscribeSession {
            session_id: session_id.clone(),
            listener,
        }) {
            tracing::error!(error = %e, "event_bus subscribe command dropped (channel full)");
        }

        EventBusSubscriber {
            cmd_tx: self.cmd_tx.clone(),
            id,
            session_id: Some(session_id),
            rx,
        }
    }

    /// 订阅所有事件。
    pub fn subscribe_all(&self) -> EventBusSubscriber {
        let (tx, rx) = mpsc::channel::<(SessionId, Event)>(256);
        let id = next_listener_id();

        let listener = Listener { id, tx };
        if let Err(e) = self.cmd_tx.try_send(Command::SubscribeGlobal { listener }) {
            tracing::error!(error = %e, "event_bus subscribe_all command dropped (channel full)");
        }

        EventBusSubscriber {
            cmd_tx: self.cmd_tx.clone(),
            id,
            session_id: None,
            rx,
        }
    }
}

/// 生产者句柄。绑定 `session_id`，发送时不需要再传。
#[derive(Clone)]
pub struct EventBusHandle {
    event_tx: mpsc::Sender<(SessionId, Event)>,
    session_id: SessionId,
}

impl EventBusHandle {
    pub async fn send(
        &self,
        event: Event,
    ) -> Result<(), mpsc::error::SendError<(SessionId, Event)>> {
        self.event_tx.send((self.session_id.clone(), event)).await
    }

    #[allow(clippy::result_large_err)]
    pub fn try_send(
        &self,
        event: Event,
    ) -> Result<(), mpsc::error::TrySendError<(SessionId, Event)>> {
        self.event_tx.try_send((self.session_id.clone(), event))
    }
}

/// 消费者 Guard。Drop 时自动从 bus 解注册。
/// 所有 subscriber（单 session / 全局）统一返回 `(SessionId, Event)` 对。
pub struct EventBusSubscriber {
    cmd_tx: mpsc::Sender<Command>,
    id: u64,
    session_id: Option<SessionId>,
    rx: mpsc::Receiver<(SessionId, Event)>,
}

impl EventBusSubscriber {
    pub async fn recv(&mut self) -> Option<(SessionId, Event)> {
        self.rx.recv().await
    }

    /// 从外部 channel 构造 subscriber（用于远程模式桥接）。
    /// id 为 `u64::MAX` 以避免与正常 listener id 冲突。
    pub fn from_receiver(rx: mpsc::Receiver<(SessionId, Event)>) -> Self {
        let (cmd_tx, _) = mpsc::channel(1);
        Self {
            cmd_tx,
            id: u64::MAX,
            session_id: None,
            rx,
        }
    }
}

impl Drop for EventBusSubscriber {
    fn drop(&mut self) {
        let _ = self.cmd_tx.try_send(Command::Unsubscribe {
            id: self.id,
            session_id: self.session_id.clone(),
        });
    }
}

struct Listener {
    id: u64,
    tx: mpsc::Sender<(SessionId, Event)>,
}

enum Command {
    SubscribeSession {
        session_id: SessionId,
        listener: Listener,
    },
    SubscribeGlobal {
        listener: Listener,
    },
    Unsubscribe {
        id: u64,
        session_id: Option<SessionId>,
    },
}

fn next_listener_id() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

async fn run_forwarder(
    mut event_rx: mpsc::Receiver<(SessionId, Event)>,
    mut cmd_rx: mpsc::Receiver<Command>,
) {
    let mut session_listeners: HashMap<SessionId, Vec<Listener>> = HashMap::new();
    let mut global_listeners: Vec<Listener> = Vec::new();

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

            Some((sid, ev)) = event_rx.recv() => {
                // 发给该 session 的所有 listener
                if let Some(ls) = session_listeners.get_mut(&sid) {
                    try_send_to_listeners(ls, &sid, &ev);
                    if ls.is_empty() {
                        session_listeners.remove(&sid);
                    }
                }

                // 发给所有全局 listener
                try_send_to_listeners(&mut global_listeners, &sid, &ev);
            }

            else => break,
        }
    }
}

/// 尝试向 listener 列表发送事件，移除已关闭的 listener。
fn try_send_to_listeners(
    listeners: &mut Vec<Listener>,
    sid: &SessionId,
    ev: &Event,
) {
    let mut to_remove = Vec::new();
    for l in &*listeners {
        match l.tx.try_send((sid.clone(), ev.clone())) {
            Err(mpsc::error::TrySendError::Closed(_)) => {
                to_remove.push(l.id);
            }
            Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
        }
    }
    for id in &to_remove {
        listeners.retain(|l| l.id != *id);
    }
}
