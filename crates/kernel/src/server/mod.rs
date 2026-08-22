use crate::event::{AgentEvent, AgentStatus, Envelope, Event, InternalEvent};
use crate::kernel::Kernel;
use crate::transport::{recv_frame, send_frame};
use crate::types::{Result, Role};
use crate::wire::WireMsg;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
mod dispatcher;
mod event_buffer;
use event_buffer::{EventBuffer, SessionSubscribers};

fn should_clear_event_buffer(event: &Event) -> bool {
    matches!(
        event,
        Event::Internal(InternalEvent::MessageAdded { message })
            if matches!(message.role, Role::System | Role::User | Role::Assistant)
    )
}

/// Handle one envelope from the kernel event bus: buffer it for replay and
/// fan it out to real-time wire subscribers.
///
/// `Event::Internal` is never put on the wire: those events are for
/// in-process consumers (e.g. Conductor persistence) only. In particular,
/// `InternalEvent::MessageReplaced` carries the full message history, which
/// can exceed the wire frame limit and kill the connection — and every
/// subsequent replay on re-subscribe. Wire clients learn about replaced
/// history via the lightweight `AgentEvent::MessageReplaced` re-published by
/// the conductor and reload messages on demand.
fn forward_envelope(
    sid: &crate::types::SessionId,
    envelope: &Envelope,
    event_buffer: &EventBuffer,
    session_subscribers: &SessionSubscribers,
    all_subscribers: &broadcast::Sender<Envelope>,
) {
    if should_clear_event_buffer(&envelope.event) {
        event_buffer.clear(sid);
        return;
    }
    if matches!(envelope.event, Event::Internal(_)) {
        return;
    }

    event_buffer.push(envelope.clone());

    if matches!(
        &envelope.event,
        Event::Agent(AgentEvent::Lifecycle {
            state: AgentStatus::Stopped { .. }
        })
    ) {
        event_buffer.remove(sid);
    }

    session_subscribers.publish(sid, envelope);
    // Cross-session live stream (SubscribeAll); lagging receivers just
    // lose events — a global feed must never block the forwarder.
    let _ = all_subscribers.send(envelope.clone());
}

/// Short human-readable summary for error logs — avoids dumping full payloads.
fn wire_msg_summary(msg: &WireMsg) -> String {
    match msg {
        WireMsg::Request { id, .. } => format!("request id={id}"),
        WireMsg::Response { id, .. } => format!("response id={id}"),
        WireMsg::Event(envelope) => format!(
            "event session_id={} event_id={}",
            envelope.session_id, envelope.event_id
        ),
        WireMsg::Noti(_) => "notification".to_owned(),
        WireMsg::Ping => "ping".to_owned(),
        WireMsg::Pong => "pong".to_owned(),
    }
}

/// Kernel daemon server. Bridges external connections to the local Kernel.
#[derive(Clone)]
pub struct KernelServer {
    pub(crate) kernel: Arc<Kernel>,
    pub(crate) config_path: Option<std::path::PathBuf>,
    pub(crate) restart_tx: Option<mpsc::Sender<()>>,
    pub(crate) instance_id: Arc<str>,
    pub(crate) connections: Arc<dashmap::DashMap<u64, tokio_util::sync::CancellationToken>>,
    pub(crate) next_conn_id: Arc<std::sync::atomic::AtomicU64>,
    /// Cron scheduler.  Held here because the `KernelServer` owns the lifecycle
    /// of the cron subsystem (start / reload / shutdown) independently of the
    /// `Kernel`, which only provides the data layer (`CronStore`).
    pub(crate) cron_scheduler: Arc<std::sync::Mutex<Option<Arc<crate::cron::CronScheduler>>>>,
    pub(crate) shutdown: tokio_util::sync::CancellationToken,
    /// Per-session event buffer for replay on re-subscribe.
    pub(crate) event_buffer: Arc<EventBuffer>,
    /// Real-time event subscribers per session.
    pub(crate) session_subscribers: Arc<SessionSubscribers>,
    /// Cross-session live event stream (real-time only, no replay).
    pub(crate) all_subscribers: broadcast::Sender<Envelope>,
}

impl KernelServer {
    /// Create a server without lifecycle restart support.
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self::with_lifecycle(kernel, crate::config::Config::discover_file(), None)
    }

    /// Create a server with an explicit config path and restart request sink.
    pub fn with_lifecycle(
        kernel: Arc<Kernel>,
        config_path: Option<std::path::PathBuf>,
        restart_tx: Option<mpsc::Sender<()>>,
    ) -> Self {
        // Adopt the kernel's slot so the RPC dispatcher and agents (cron
        // tool) notify the same scheduler instance.
        let cron_scheduler = kernel.cron_scheduler_slot();
        kernel
            .restart_slot()
            .lock()
            .unwrap()
            .clone_from(&restart_tx);
        Self {
            kernel,
            config_path,
            restart_tx,
            instance_id: Arc::from(ulid::Ulid::new().to_string()),
            connections: Arc::new(dashmap::DashMap::new()),
            next_conn_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cron_scheduler,
            shutdown: tokio_util::sync::CancellationToken::new(),
            event_buffer: Arc::new(EventBuffer::new(10_000)),
            session_subscribers: Arc::new(SessionSubscribers::new(4096)),
            all_subscribers: broadcast::channel(4096).0,
        }
    }

    pub async fn start(&self, config: &crate::config::Config) {
        self.kernel.start();

        // supervised 扩展进程：跟随 daemon 拉起，随 daemon 死亡。
        if !config.extensions.is_empty() {
            let log_dir = self.kernel.data_dir().await.join("logs");
            crate::extension::supervisor::start_supervisor(
                config.extensions.clone(),
                log_dir,
                self.shutdown.child_token(),
            );
        }

        if let Some(store) = self.kernel.cron_store.as_ref() {
            let (task_tx, task_rx) = mpsc::channel(64);
            let scheduler = Arc::new(crate::cron::CronScheduler::new(Arc::clone(store), task_tx));

            let sched_clone = Arc::clone(&scheduler);
            let cron_token = self.shutdown.child_token();
            tokio::spawn(async move { sched_clone.run(cron_token).await });

            let worker = crate::cron::CronWorker::new(
                Arc::clone(&self.kernel) as Arc<dyn crate::cron::CronExecutor>,
                task_rx,
                Arc::clone(store),
                Some(Arc::clone(&scheduler)),
            );
            let worker_token = self.shutdown.child_token();
            tokio::spawn(async move { worker.run(worker_token).await });

            *self.cron_scheduler.lock().unwrap() = Some(scheduler);
        }

        if let Some(ref mgr) = self.kernel.channel_manager {
            let weak = Arc::downgrade(&self.kernel);
            if let Err(e) = mgr
                .start_all(self.shutdown.clone(), config.channels.clone(), weak)
                .await
            {
                tracing::warn!(error = %e, "some channels failed to start");
            }
        }

        // Start the global event-forwarder task that assigns event IDs,
        // buffers events, and forwards them to real-time subscribers.
        self.start_event_forwarder(self.shutdown.child_token());
        self.start_subscriber_sweeper(self.shutdown.child_token());
    }

    fn start_event_forwarder(&self, cancel: tokio_util::sync::CancellationToken) {
        let event_buffer = Arc::clone(&self.event_buffer);
        let session_subscribers = Arc::clone(&self.session_subscribers);
        let all_subscribers = self.all_subscribers.clone();
        let bus = match self.kernel.event_bus() {
            Some(b) => b,
            None => return,
        };

        tokio::spawn(async move {
            let mut subscriber = bus.subscribe_all();
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    Some((sid, envelope)) = subscriber.recv() => {
                        forward_envelope(&sid, &envelope, &event_buffer, &session_subscribers, &all_subscribers);
                    }
                }
            }
            tracing::info!("event forwarder exited");
        });
    }

    fn start_subscriber_sweeper(&self, cancel: tokio_util::sync::CancellationToken) {
        let session_subscribers = Arc::clone(&self.session_subscribers);

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_mins(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    _ = interval.tick() => session_subscribers.prune_idle(),
                }
            }

            tracing::info!("event subscriber sweeper exited");
        });
    }

    /// Run the server on an already-bound listener.
    ///
    /// Returns after either `shutdown` or the server's internal token is
    /// cancelled. All background tasks and the kernel are stopped before
    /// returning, so callers only need to wait for connections to drain.
    pub async fn serve(
        &self,
        listener: crate::transport::Listener,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        loop {
            tokio::select! {
                biased;
                () = self.shutdown.cancelled() => break,
                () = shutdown.cancelled() => break,
                result = listener.accept() => {
                    let (stream, _) = match result {
                        Ok(pair) => pair,
                        Err(e) => {
                            tracing::warn!("Accept error: {e}");
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            continue;
                        }
                    };
                    let conn_id = self
                        .next_conn_id
                        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let server = Arc::new(self.clone());
                    let connections = Arc::clone(&self.connections);
                    let cancel = self.shutdown.child_token();
                    connections.insert(conn_id, cancel.clone());
                    tokio::spawn(async move {
                        if let Err(e) = server.handle_connection(stream, cancel).await {
                            tracing::warn!("Connection {conn_id} error: {e}");
                        }
                        connections.remove(&conn_id);
                        tracing::debug!("Connection {conn_id} closed");
                    });
                }
            }
        }
        tracing::info!("Server shutting down, accept loop stopped");
        // 幂等 cancel（`shutdown` 的另一半）+ 等持久化排空再退出——
        // daemon 重启路径上已入队的落盘写必须走完（复审 must-fix）。
        self.shutdown.cancel();
        self.kernel.graceful_stop().await;
        Ok(())
    }

    pub fn shutdown(&self) {
        self.shutdown.cancel();
        self.kernel.stop();
    }

    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }

    async fn handle_connection(
        self: Arc<Self>,
        stream: crate::transport::Stream,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        const OUTBOUND_CHANNEL_SIZE: usize = 4096;

        // 连接的扩展身份：ext_* 方法的归属键，断开时 RAII 回收的边界。
        let conn_id = format!("conn_{}", ulid::Ulid::new().to_string().to_lowercase());

        let (mut read_half, mut write_half) = stream.into_split();
        let (send_tx, mut send_rx) = mpsc::channel::<WireMsg>(OUTBOUND_CHANNEL_SIZE);

        let cancel_writer = cancel.clone();
        let writer = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = cancel_writer.cancelled() => break,
                    maybe_msg = send_rx.recv() => {
                        match maybe_msg {
                            Some(msg) => {
                                if let Err(e) = send_frame(&mut write_half, &msg).await {
                                    if e.kind() == std::io::ErrorKind::InvalidData {
                                        // Our own frame exceeded MAX_FRAME_SIZE or
                                        // failed to serialize: a payload bug on our
                                        // side, not a network issue. Log loudly —
                                        // a silent drop here is how an oversized
                                        // event kills connections unnoticed.
                                        tracing::error!(
                                            "Outbound frame rejected ({}): {e}",
                                            wire_msg_summary(&msg)
                                        );
                                    } else {
                                        tracing::warn!(
                                            "Send frame error, closing connection: {e}"
                                        );
                                    }
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                }
            }
        });

        let notification_cancel = cancel.clone();
        let notification_send_tx = send_tx.clone();
        let bus = self.kernel.notification_bus();
        let mut rx = bus.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = notification_cancel.cancelled() => break,
                    res = rx.recv() => {
                        match res {
                            Ok(noti) => {
                                if let Err(e) = notification_send_tx.try_send(WireMsg::Noti(noti)) {
                                    match e {
                                        mpsc::error::TrySendError::Full(_) => {
                                            tracing::warn!("Outbound channel full, dropping notification");
                                        }
                                        mpsc::error::TrySendError::Closed(_) => break,
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!("Notification subscriber lagged, dropped {n} messages");
                            }
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });

        let subscriptions: std::sync::Arc<
            tokio::sync::RwLock<std::collections::HashMap<String, tokio::task::JoinHandle<()>>>,
        > = std::sync::Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

        loop {
            let msg = tokio::select! {
                biased;
                () = cancel.cancelled() => break,
                result = recv_frame(&mut read_half) => match result {
                    Ok(m) => m,
                    Err(e) => {
                        match e.kind() {
                            // Peer closed the connection without a protocol-level
                            // goodbye — a routine client disconnect, not an error.
                            std::io::ErrorKind::UnexpectedEof => {
                                tracing::debug!("Client disconnected: {e}");
                            }
                            // Peer sent an oversized or malformed frame.
                            std::io::ErrorKind::InvalidData => {
                                tracing::warn!("Inbound frame rejected: {e}");
                            }
                            _ => {
                                tracing::warn!("Recv frame error, closing connection: {e}");
                            }
                        }
                        break;
                    }
                },
            };

            match msg {
                WireMsg::Ping => {
                    if let Err(e) = send_tx.try_send(WireMsg::Pong) {
                        match e {
                            tokio::sync::mpsc::error::TrySendError::Full(_) => {
                                tracing::warn!("Outbound channel full, dropping pong");
                            }
                            tokio::sync::mpsc::error::TrySendError::Closed(_) => break,
                        }
                    }
                }
                WireMsg::Request { id, method } => {
                    let body = self
                        .dispatch_request(
                            std::sync::Arc::clone(&subscriptions),
                            send_tx.clone(),
                            cancel.clone(),
                            &conn_id,
                            method,
                        )
                        .await;
                    if let Err(e) = send_tx.send(WireMsg::Response { id, body }).await {
                        tracing::warn!(
                            "Outbound channel closed, dropping response for id={id}: {e}"
                        );
                        break;
                    }
                }
                WireMsg::Event { .. } | WireMsg::Response { .. } | WireMsg::Noti { .. } => {
                    tracing::warn!("Unexpected message from client: {:?}", msg);
                }
                WireMsg::Pong => {}
            }
        }

        let subs = subscriptions.write().await;
        for (_, handle) in subs.iter() {
            handle.abort();
        }
        drop(subs);

        // RAII：连接终止 = 该连接的全部扩展能力回收（代理 tool 摘除、
        // pending 工作项以 disconnected 报错）。
        self.kernel.extension_registry().sweep(&conn_id);

        cancel.cancel();
        let _ = writer.await;

        Ok(())
    }
}

#[cfg(test)]
mod event_buffer_test;
#[cfg(test)]
mod mod_test;
