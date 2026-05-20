use crate::app::{Coordinator, SessionConfig};
use crate::checkpoint::RewindTarget;
use crate::event::{ControlCommand, Event};
use crate::goal::GoalState;
use crate::permissions::Level;
use crate::transport::{recv_frame, send_frame};
use crate::types::{ContentBlock, KernelError, Message, MessageId, Result, SessionId};
use crate::wire::{RequestIdGenerator, RequestMethod, ResponseBody, RpcError, WireMsg};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::{unix::OwnedWriteHalf, UnixStream};
use tokio::sync::{broadcast, Mutex};

/// How long to retry connecting to the daemon on first use.
const CONNECT_RETRY_TIMEOUT: Duration = Duration::from_secs(2);
/// Interval between connection retries.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(10);
/// RPC request timeout.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

type PendingMap = dashmap::DashMap<
    u64,
    tokio::sync::oneshot::Sender<std::result::Result<serde_json::Value, RpcError>>,
>;
type EventRouterMap = dashmap::DashMap<String, broadcast::Sender<Event>>;

/// Unified API for both local (in-process) and remote (IPC) coordinators.
#[async_trait]
pub trait CoordinatorApi: Send + Sync {
    async fn create_session(&self, config: SessionConfig) -> Result<SessionId>;
    async fn restore_session(&self, id: &SessionId, config: SessionConfig) -> Result<SessionId>;
    async fn fork_session(&self, parent: &SessionId, config: SessionConfig) -> Result<SessionId>;
    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()>;
    async fn cancel(&self, session_id: &SessionId) -> Result<()>;
    async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()>;
    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()>;
    async fn compact_session(&self, session_id: &SessionId) -> Result<()>;
    async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        target: RewindTarget,
    ) -> Result<()>;
    async fn start_goal(&self, session_id: &SessionId, state: GoalState) -> Result<()>;
    async fn stop_goal(&self, session_id: &SessionId) -> Result<()>;
    async fn delete_session(&self, session_id: &SessionId) -> Result<()>;
    async fn get_session_messages(&self, session_id: &SessionId) -> Result<Vec<Message>>;
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<broadcast::Receiver<Event>>;
    async fn list_sessions(&self) -> Result<Vec<SessionId>>;
    async fn list_sessions_filtered(
        &self,
        args: crate::storage::session::ListArgs,
    ) -> Result<Vec<crate::storage::session::SessionInfo>>;
    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>>;
    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>>;
    async fn shutdown_session(&self, session_id: &SessionId) -> Result<()>;
}

// ── LocalCoordinator (existing Coordinator wrapped) ──────────────────────

#[async_trait]
impl CoordinatorApi for Coordinator {
    async fn create_session(&self, config: SessionConfig) -> Result<SessionId> {
        self.create_session(config).await
    }

    async fn restore_session(&self, id: &SessionId, config: SessionConfig) -> Result<SessionId> {
        self.restore_session(id, config).await
    }

    async fn fork_session(&self, parent: &SessionId, config: SessionConfig) -> Result<SessionId> {
        self.fork_session(parent, config).await
    }

    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()> {
        self.send_message(session_id, blocks).await
    }

    async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        self.cancel(session_id).await
    }

    async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()> {
        self.send_permission_response(session_id, req_id, approved, remember)
            .await
    }

    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        self.set_permission_level(session_id, level).await
    }

    async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        self.compact_session(session_id).await
    }

    async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        target: RewindTarget,
    ) -> Result<()> {
        self.rewind_session(session_id, message_id, target).await
    }

    async fn start_goal(&self, session_id: &SessionId, state: GoalState) -> Result<()> {
        self.start_goal(session_id, state).await
    }

    async fn stop_goal(&self, session_id: &SessionId) -> Result<()> {
        self.stop_goal(session_id).await
    }

    async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.delete_session(session_id).await
    }

    async fn get_session_messages(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        self.get_session_messages(session_id).await
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<broadcast::Receiver<Event>> {
        self.subscribe_session_events(session_id)
            .await
            .ok_or_else(|| KernelError::session(format!("Session not found: {}", session_id.0)))
    }

    async fn list_sessions(&self) -> Result<Vec<SessionId>> {
        Ok(self.list_sessions().await)
    }

    async fn list_sessions_filtered(
        &self,
        args: crate::storage::session::ListArgs,
    ) -> Result<Vec<crate::storage::session::SessionInfo>> {
        self.list_sessions_filtered(args).await
    }

    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        self.get_checkpoints(session_id).await
    }

    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>> {
        self.get_todos(session_id).await
    }

    async fn shutdown_session(&self, session_id: &SessionId) -> Result<()> {
        self.shutdown_session(session_id).await
    }
}

// ── RemoteCoordinator (IPC client with lazy connect) ─────────────────────

struct Connection {
    write_half: Arc<Mutex<OwnedWriteHalf>>,
    pending: Arc<PendingMap>,
    _reader: tokio::task::JoinHandle<()>,
}

/// Client-side coordinator proxy that talks to a kernel daemon over a Unix socket.
/// Uses lazy connect: the socket connection is established on the first API call.
pub struct RemoteCoordinator {
    socket_path: PathBuf,
    req_id: RequestIdGenerator,
    connection: Arc<Mutex<Option<Connection>>>,
    /// Persistent local event routers: `session_id` -> broadcast sender.
    /// Lifetime is independent of individual connections so that receivers
    /// survive reconnects.
    event_routers: Arc<EventRouterMap>,
}

impl RemoteCoordinator {
    /// Create a lazy coordinator that connects on first use.
    pub fn new(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            req_id: RequestIdGenerator::new(),
            connection: Arc::new(Mutex::new(None)),
            event_routers: Arc::new(EventRouterMap::new()),
        }
    }

    /// Connect immediately and return a ready coordinator.
    pub async fn connect(path: &std::path::Path) -> Result<Self> {
        let stream = UnixStream::connect(path).await?;
        Self::from_stream(stream, path).await
    }

    /// Wrap an already-connected stream.
    pub async fn from_stream(stream: UnixStream, socket_path: &std::path::Path) -> Result<Self> {
        let (read_half, write_half) = stream.into_split();
        let write_half = Arc::new(Mutex::new(write_half));
        let pending: Arc<PendingMap> = Arc::new(PendingMap::new());
        let event_routers: Arc<EventRouterMap> = Arc::new(EventRouterMap::new());

        let reader = Self::spawn_reader(
            read_half,
            Arc::clone(&write_half),
            Arc::clone(&pending),
            Arc::clone(&event_routers),
        );

        let this = Self {
            socket_path: socket_path.to_path_buf(),
            req_id: RequestIdGenerator::new(),
            connection: Arc::new(Mutex::new(None)),
            event_routers,
        };
        *this.connection.lock().await = Some(Connection {
            write_half,
            pending,
            _reader: reader,
        });
        Ok(this)
    }

    fn spawn_reader(
        mut read_half: tokio::net::unix::OwnedReadHalf,
        write_half: Arc<Mutex<OwnedWriteHalf>>,
        pending: Arc<PendingMap>,
        event_routers: Arc<EventRouterMap>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let msg = match recv_frame(&mut read_half).await {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!("Remote reader error: {e}");
                        break;
                    }
                };

                match msg {
                    WireMsg::Response { id, body } => {
                        let result = match body {
                            ResponseBody::Ok { result } => Ok(result),
                            ResponseBody::Err { error } => Err(error),
                        };
                        if let Some((_, tx)) = pending.remove(&id) {
                            let _ = tx.send(result);
                        }
                    }
                    WireMsg::Event { session_id, event } => {
                        if let Some(entry) = event_routers.get(&session_id) {
                            let _ = entry.value().send(event);
                        }
                    }
                    WireMsg::Ping => {
                        let mut guard = write_half.lock().await;
                        let _ = send_frame(&mut *guard, &WireMsg::Pong).await;
                    }
                    WireMsg::Pong => {
                        // No-op.
                    }
                    WireMsg::Request { .. } => {
                        tracing::warn!("Unexpected message from server: {:?}", msg);
                    }
                }
            }

            let keys: Vec<u64> = pending.iter().map(|e| *e.key()).collect();
            for key in keys {
                if let Some((_, tx)) = pending.remove(&key) {
                    let _ = tx.send(Err(RpcError {
                        code: "connection_closed".to_string(),
                        message: "Connection to kernel daemon closed".to_string(),
                    }));
                }
            }
        })
    }

    /// Ensure the connection is established (lazy on first call).
    /// Retries for up to 2s to allow the daemon to finish spawning.
    /// On reconnect, re-subscribes all sessions in the persistent router.
    async fn ensure_connected(&self) -> Result<()> {
        let mut guard = self.connection.lock().await;
        if let Some(ref conn) = *guard {
            if !conn._reader.is_finished() {
                return Ok(());
            }
        }
        if let Some(old) = guard.take() {
            old._reader.abort();
        }
        let sock = &self.socket_path;
        let start = tokio::time::Instant::now();
        let stream = loop {
            match UnixStream::connect(sock).await {
                Ok(s) => break s,
                Err(_) if start.elapsed() < CONNECT_RETRY_TIMEOUT => {
                    tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
                }
                Err(e) => {
                    return Err(KernelError::session(format!(
                        "Failed to connect to daemon: {e}"
                    )));
                }
            }
        };
        let (read_half, write_half) = stream.into_split();
        let write_half = Arc::new(Mutex::new(write_half));
        let pending: Arc<PendingMap> = Arc::new(PendingMap::new());
        let reader = Self::spawn_reader(
            read_half,
            Arc::clone(&write_half),
            Arc::clone(&pending),
            Arc::clone(&self.event_routers),
        );

        *guard = Some(Connection {
            write_half: Arc::clone(&write_half),
            pending,
            _reader: reader,
        });

        // Re-subscribe sessions that still have active local receivers.
        // We re-check receiver_count inside the loop because a concurrent
        // unsubscribe may have dropped the count to zero between collection
        // and sending.
        let sessions_to_resub: Vec<String> = self
            .event_routers
            .iter()
            .filter(|e| e.value().receiver_count() > 0)
            .map(|e| e.key().clone())
            .collect();
        if !sessions_to_resub.is_empty() {
            let mut w = write_half.lock().await;
            for sid in sessions_to_resub {
                if let Some(entry) = self.event_routers.get(&sid) {
                    if entry.value().receiver_count() > 0 {
                        let req = WireMsg::Request {
                            id: self.req_id.next(),
                            method: RequestMethod::Subscribe { session_id: sid },
                        };
                        let _ = send_frame(&mut *w, &req).await;
                    }
                }
            }
        }

        Ok(())
    }

    async fn call(&self, method: RequestMethod) -> Result<serde_json::Value> {
        self.ensure_connected().await?;
        let id = self.req_id.next();

        // Grab write_half and install pending oneshot, then drop the
        // connection lock so we don't hold it across the network await.
        let (write_half, rx) = {
            let guard = self.connection.lock().await;
            let conn = guard.as_ref().ok_or_else(|| {
                KernelError::session("Connection lost during operation".to_string())
            })?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            conn.pending.insert(id, tx);
            (Arc::clone(&conn.write_half), rx)
        };

        let msg = WireMsg::Request { id, method };
        {
            let mut w = write_half.lock().await;
            send_frame(&mut *w, &msg).await?;
        }

        match tokio::time::timeout(RPC_TIMEOUT, rx).await {
            Ok(Ok(Ok(val))) => Ok(val),
            Ok(Ok(Err(e))) => Err(KernelError::session(format!(
                "RPC error [{}]: {}",
                e.code, e.message
            ))),
            Ok(Err(_)) => Err(KernelError::session("Request cancelled".to_string())),
            Err(_) => {
                // Clean up the stale pending entry so it doesn't leak
                // memory for the lifetime of the connection.
                if let Ok(guard) = self.connection.try_lock() {
                    if let Some(ref conn) = *guard {
                        conn.pending.remove(&id);
                    }
                }
                Err(KernelError::session(
                    "RPC request timed out (30s)".to_string(),
                ))
            }
        }
    }

    async fn subscribe_events_internal(
        &self,
        session_id: &SessionId,
    ) -> Result<broadcast::Receiver<Event>> {
        use dashmap::mapref::entry::Entry;

        // Fast path: already subscribed locally with active receivers.
        if let Some(entry) = self.event_routers.get(&session_id.0) {
            if entry.value().receiver_count() > 0 {
                return Ok(entry.value().subscribe());
            }
        }

        // Slow path: atomically insert or re-activate a stale sender.
        let tx = match self.event_routers.entry(session_id.0.clone()) {
            Entry::Occupied(entry) => {
                // receiver_count == 0 here (fast path already handled > 0).
                // Reuse the sender so any late receivers still get events,
                // but we will re-send Subscribe below.
                entry.get().clone()
            }
            Entry::Vacant(entry) => {
                let (tx, _rx) = broadcast::channel(256);
                entry.insert(tx.clone());
                tx
            }
        };

        let result = self
            .call(RequestMethod::Subscribe {
                session_id: session_id.0.clone(),
            })
            .await;
        if let Err(e) = result {
            self.event_routers.remove(&session_id.0);
            return Err(e);
        }
        Ok(tx.subscribe())
    }
}

#[async_trait]
impl CoordinatorApi for RemoteCoordinator {
    async fn create_session(&self, config: SessionConfig) -> Result<SessionId> {
        let result = self
            .call(RequestMethod::CreateSession {
                project_path: config.project_path.to_string_lossy().to_string(),
                auto_approve_level: config.auto_approve_level,
            })
            .await?;
        let sid: String = serde_json::from_value(result)?;
        Ok(SessionId(sid))
    }

    async fn restore_session(&self, id: &SessionId, config: SessionConfig) -> Result<SessionId> {
        let result = self
            .call(RequestMethod::RestoreSession {
                session_id: id.0.clone(),
                project_path: config.project_path.to_string_lossy().to_string(),
                auto_approve_level: config.auto_approve_level,
            })
            .await?;
        let sid: String = serde_json::from_value(result)?;
        Ok(SessionId(sid))
    }

    async fn fork_session(&self, parent: &SessionId, config: SessionConfig) -> Result<SessionId> {
        let result = self
            .call(RequestMethod::ForkSession {
                parent_id: parent.0.clone(),
                project_path: config.project_path.to_string_lossy().to_string(),
                auto_approve_level: config.auto_approve_level,
            })
            .await?;
        let sid: String = serde_json::from_value(result)?;
        Ok(SessionId(sid))
    }

    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()> {
        self.call(RequestMethod::SendMessage {
            session_id: session_id.0.clone(),
            blocks,
        })
        .await?;
        Ok(())
    }

    async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::Cancel,
        })
        .await?;
        Ok(())
    }

    async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::Response {
                req_id: req_id.to_string(),
                approved,
                remember,
            },
        })
        .await?;
        Ok(())
    }

    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::SetLevel(level),
        })
        .await?;
        Ok(())
    }

    async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::Compact,
        })
        .await?;
        Ok(())
    }

    async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        target: RewindTarget,
    ) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::Rewind { message_id, target },
        })
        .await?;
        Ok(())
    }

    async fn start_goal(&self, session_id: &SessionId, state: GoalState) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::StartGoal(state),
        })
        .await?;
        Ok(())
    }

    async fn stop_goal(&self, session_id: &SessionId) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::StopGoal,
        })
        .await?;
        Ok(())
    }

    async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.call(RequestMethod::DeleteSession {
            session_id: session_id.0.clone(),
        })
        .await?;
        Ok(())
    }

    async fn get_session_messages(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        let result = self
            .call(RequestMethod::GetSessionMessages {
                session_id: session_id.0.clone(),
            })
            .await?;
        let msgs: Vec<Message> = serde_json::from_value(result)?;
        Ok(msgs)
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<broadcast::Receiver<Event>> {
        self.subscribe_events_internal(session_id).await
    }

    async fn list_sessions(&self) -> Result<Vec<SessionId>> {
        let result = self.call(RequestMethod::ListSessions).await?;
        let ids: Vec<String> = serde_json::from_value(result)?;
        Ok(ids.into_iter().map(SessionId).collect())
    }

    async fn list_sessions_filtered(
        &self,
        args: crate::storage::session::ListArgs,
    ) -> Result<Vec<crate::storage::session::SessionInfo>> {
        let result = self.call(RequestMethod::ListSessionsFiltered(args)).await?;
        let sessions = serde_json::from_value(result)?;
        Ok(sessions)
    }

    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        let result = self
            .call(RequestMethod::GetCheckpoints {
                session_id: session_id.0.clone(),
            })
            .await?;
        let checkpoints = serde_json::from_value(result)?;
        Ok(checkpoints)
    }

    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>> {
        let result = self
            .call(RequestMethod::GetTodos {
                session_id: session_id.0.clone(),
            })
            .await?;
        let todos = serde_json::from_value(result)?;
        Ok(todos)
    }

    async fn shutdown_session(&self, session_id: &SessionId) -> Result<()> {
        self.call(RequestMethod::ShutdownSession {
            session_id: session_id.0.clone(),
        })
        .await?;
        Ok(())
    }
}
