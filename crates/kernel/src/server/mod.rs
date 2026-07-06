use crate::event::{AgentEvent, AgentStatus, Event, InternalEvent};
use crate::kernel::Kernel;
use crate::transport::{recv_frame, send_frame};
use crate::types::Result;
use crate::wire::WireMsg;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast;
use tokio::sync::mpsc;
mod dispatcher;
mod event_buffer;
use event_buffer::{EventBuffer, SessionSubscribers};

/// Kernel daemon server. Bridges external connections to the local Kernel.
#[derive(Clone)]
pub struct KernelServer {
    pub(crate) kernel: Arc<Kernel>,
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
}

impl KernelServer {
    pub fn new(kernel: Arc<Kernel>) -> Self {
        Self {
            kernel,
            connections: Arc::new(dashmap::DashMap::new()),
            next_conn_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cron_scheduler: Arc::new(std::sync::Mutex::new(None)),
            shutdown: tokio_util::sync::CancellationToken::new(),
            event_buffer: Arc::new(EventBuffer::new(10_000)),
            session_subscribers: Arc::new(SessionSubscribers::new()),
        }
    }

    pub async fn start(&self, configs: Vec<crate::channels::ChannelConfig>) {
        self.kernel.start();

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
            if let Err(e) = mgr.start_all(self.shutdown.clone(), configs, weak).await {
                tracing::warn!(error = %e, "some channels failed to start");
            }
        }

        // Start the global event-forwarder task that assigns event IDs,
        // buffers events, and forwards them to real-time subscribers.
        self.start_event_forwarder(self.shutdown.child_token());
    }

    fn start_event_forwarder(&self, cancel: tokio_util::sync::CancellationToken) {
        let event_buffer = Arc::clone(&self.event_buffer);
        let session_subscribers = Arc::clone(&self.session_subscribers);
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
                        // Buffer first, then forward.
                        event_buffer.push(envelope.clone());

                        match &envelope.event {
                            Event::Internal(InternalEvent::MessageAdded { .. }) => {
                                event_buffer.clear(&sid);
                            }
                            Event::Agent(AgentEvent::Lifecycle { state: AgentStatus::Stopped { .. } }) => {
                                event_buffer.remove(&sid);
                            }
                            _ => {}
                        }

                        session_subscribers.try_send(&sid, &envelope);
                    }
                }
            }
            tracing::info!("event forwarder exited");
        });
    }

    /// Run the server on an IPC endpoint (Unix socket or TCP).
    pub async fn serve_ipc(
        &self,
        addr: &crate::transport::SocketAddr,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        let listener = crate::transport::bind(addr).await?;
        tracing::info!("KernelServer listening on {addr}");
        self.serve_listener(listener, shutdown).await
    }

    /// Run the server on an already-bound listener.
    pub async fn serve_listener(
        &self,
        listener: crate::transport::Listener,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        loop {
            tokio::select! {
                biased;
                () = self.shutdown.cancelled() => {
                    tracing::info!("Server shutting down, stopping accept loop");
                    break;
                }
                () = shutdown.cancelled() => {
                    tracing::info!("Server shutting down, stopping accept loop");
                    break;
                }
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
                                    tracing::debug!("Send frame error: {e}");
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
                                if let Err(e) = notification_send_tx.send(WireMsg::Noti(noti)).await {
                                    tracing::debug!("Notification send error: {e}");
                                    break;
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
                        tracing::debug!("Recv frame error: {e}");
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
                            method,
                        )
                        .await;
                    if let Err(e) = send_tx.send(WireMsg::Response { id, body }).await {
                        tracing::debug!(
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

        cancel.cancel();
        let _ = writer.await;

        Ok(())
    }
}

#[cfg(test)]
mod event_buffer_test;
