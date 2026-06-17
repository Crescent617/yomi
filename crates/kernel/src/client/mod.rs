use crate::app::coordinator::CreateSessionInput;
use crate::app::Coordinator;
use crate::checkpoint::RewindTarget;
use crate::event::{ControlCommand, Event};
use crate::goal::GoalState;
use crate::permissions::Level;
use crate::transport::{recv_frame, send_frame, ReadHalf, SocketAddr, Stream, WriteHalf};
use crate::types::{
    ContentBlock, KernelError, Message, MessageId, Project, ProjectId, Result, SessionError,
    SessionId,
};
use crate::wire::{RequestIdGenerator, RequestMethod, ResponseBody, RpcError, WireMsg};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex};

/// How long to retry connecting to the daemon on first use.
/// Daemon initialisation (storage, provider, skills) can take several
/// seconds, so we allow a generous timeout.
const CONNECT_RETRY_TIMEOUT: Duration = Duration::from_secs(10);
/// Interval between connection retries.
const CONNECT_RETRY_INTERVAL: Duration = Duration::from_millis(10);
/// RPC request timeout.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);
/// Heartbeat interval in seconds.
const HEARTBEAT_INTERVAL_SECS: u64 = 2;
/// Heartbeat timeout in seconds (3 missed heartbeats).
const HEARTBEAT_TIMEOUT_SECS: u64 = 6;

type PendingMap = dashmap::DashMap<
    u64,
    tokio::sync::oneshot::Sender<std::result::Result<serde_json::Value, RpcError>>,
>;
type EventRouterMap = dashmap::DashMap<String, broadcast::Sender<Event>>;

/// Paginated session list result
#[derive(Debug, Clone, serde::Serialize)]
pub struct PaginatedSessions {
    pub sessions: Vec<crate::storage::session::SessionInfo>,
    pub has_more: bool,
}

/// Unified API for both local (in-process) and remote (IPC) coordinators.
#[async_trait]
pub trait CoordinatorApi: Send + Sync {
    // ── Project ──────────────────────────────────────────────────────────
    async fn list_projects(&self) -> Result<Vec<Project>>;
    async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project>;
    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>>;
    async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()>;
    async fn delete_project(&self, id: &ProjectId) -> Result<()>;

    // ── Session ──────────────────────────────────────────────────────────
    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId>;
    async fn restore_session(&self, id: &SessionId) -> Result<SessionId>;
    async fn fork_session(
        &self,
        parent: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId>;
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
    async fn rename_session(&self, session_id: &SessionId, title: String) -> Result<()>;
    async fn start_goal(&self, session_id: &SessionId, state: GoalState) -> Result<()>;
    async fn pause_goal(&self, session_id: &SessionId) -> Result<()>;
    async fn resume_goal(&self, session_id: &SessionId) -> Result<()>;
    async fn get_goal(&self, session_id: &SessionId) -> Result<Option<crate::goal::GoalState>>;
    async fn update_goal(&self, session_id: &SessionId, description: String) -> Result<()>;
    async fn stop_goal(&self, session_id: &SessionId) -> Result<()>;
    async fn delete_session(&self, session_id: &SessionId) -> Result<()>;
    async fn get_session_messages(&self, session_id: &SessionId) -> Result<Vec<Message>>;
    async fn get_session_status(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::types::SessionStatus>;
    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<broadcast::Receiver<Event>>;
    async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<PaginatedSessions>;
    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>>;
    async fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()>;
    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>>;
    async fn shutdown_session(&self, session_id: &SessionId) -> Result<()>;
    async fn reload_agent_config(&self) -> Result<()>;
    async fn send_steer(&self, session_id: &SessionId, content: Vec<ContentBlock>) -> Result<()>;
    async fn send_continue(&self, session_id: &SessionId) -> Result<()>;

    // ── Usage ──────────────────────────────────────────────────────────
    async fn get_usage_summary(&self, days: i64) -> Result<crate::storage::usage::UsageSummary>;
    async fn get_daily_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::DailyUsage>>;

    // ── Cron Job ─────────────────────────────────────────────────────────
    //
    // DESIGN PRINCIPLE: All cron operations MUST go through `CoordinatorApi`.
    // Clients (GUI, TUI, CLI) must never hold a `CronStore` directly, because
    // that would only work in local/in-process mode and break remote IPC mode.
    // By routing every cron call through the coordinator, both `LocalCoordinator`
    // and `RemoteCoordinator` can serve the same interface.
    // ──────────────────────────────────────────────────────────────────────

    async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId>;
    async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>>;
    async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>>;
    async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool>;
    async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool>;
    async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()>;
}

// ── LocalCoordinator (existing Coordinator wrapped) ──────────────────────

#[async_trait]
impl CoordinatorApi for Coordinator {
    async fn list_projects(&self) -> Result<Vec<Project>> {
        Self::list_projects(self).await
    }

    async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        Self::create_project(self, dir, name).await
    }

    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        Self::get_project(self, id).await
    }

    async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()> {
        Self::rename_project(self, id, name).await
    }

    async fn delete_project(&self, id: &ProjectId) -> Result<()> {
        Self::delete_project(self, id).await
    }

    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        Self::create_session(self, input).await
    }

    async fn restore_session(&self, id: &SessionId) -> Result<SessionId> {
        Self::restore_session(self, id).await
    }

    async fn fork_session(
        &self,
        parent: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        Self::fork_session(self, parent, auto_approve_level).await
    }

    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()> {
        Self::send_message(self, session_id, blocks).await
    }

    async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        Self::cancel(self, session_id).await
    }

    async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()> {
        Self::send_permission_response(self, session_id, req_id, approved, remember).await
    }

    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        Self::set_permission_level(self, session_id, level).await
    }

    async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        Self::compact_session(self, session_id).await
    }

    async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: MessageId,
        target: RewindTarget,
    ) -> Result<()> {
        Self::rewind_session(self, session_id, message_id, target).await
    }

    async fn rename_session(&self, session_id: &SessionId, title: String) -> Result<()> {
        Self::rename_session(self, session_id, title).await
    }

    async fn start_goal(&self, session_id: &SessionId, state: GoalState) -> Result<()> {
        Self::start_goal(self, session_id, state).await
    }

    async fn pause_goal(&self, session_id: &SessionId) -> Result<()> {
        Self::pause_goal(self, session_id).await
    }

    async fn resume_goal(&self, session_id: &SessionId) -> Result<()> {
        Self::resume_goal(self, session_id).await
    }

    async fn get_goal(&self, session_id: &SessionId) -> Result<Option<crate::goal::GoalState>> {
        Self::get_goal(self, session_id).await
    }

    async fn update_goal(&self, session_id: &SessionId, description: String) -> Result<()> {
        Self::update_goal(self, session_id, description).await
    }

    async fn stop_goal(&self, session_id: &SessionId) -> Result<()> {
        Self::stop_goal(self, session_id).await
    }

    async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        Self::delete_session(self, session_id).await
    }

    async fn get_session_messages(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        Self::get_session_messages(self, session_id).await
    }

    async fn get_session_status(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::types::SessionStatus> {
        Self::get_session_status(self, session_id).await
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<broadcast::Receiver<Event>> {
        Self::subscribe_session_events(self, session_id).ok_or_else(|| {
            SessionError::NotFound {
                session_id: session_id.0.clone(),
            }
            .into()
        })
    }

    async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<PaginatedSessions> {
        let (sessions, has_more) = Self::list_sessions(self, project_id, before, limit).await?;
        Ok(PaginatedSessions { sessions, has_more })
    }

    async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        Self::get_checkpoints(self, session_id).await
    }

    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>> {
        Self::get_todos(self, session_id).await
    }

    async fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()> {
        Self::send_ask_user_response(self, session_id, req_id, response).await
    }

    async fn shutdown_session(&self, session_id: &SessionId) -> Result<()> {
        Self::shutdown_session(self, session_id).await
    }

    async fn reload_agent_config(&self) -> Result<()> {
        let working_dir = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let config_file = crate::config::Config::discover_file();
        self.reload(config_file.as_ref(), &working_dir).await
    }

    async fn send_steer(&self, session_id: &SessionId, content: Vec<ContentBlock>) -> Result<()> {
        Self::send_steer(self, session_id, content).await
    }

    async fn send_continue(&self, session_id: &SessionId) -> Result<()> {
        Self::send_continue(self, session_id).await
    }

    async fn get_usage_summary(&self, days: i64) -> Result<crate::storage::usage::UsageSummary> {
        Self::get_usage_summary(self, days).await
    }

    async fn get_daily_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::DailyUsage>> {
        Self::get_daily_usage(self, days).await
    }

    async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId> {
        Self::create_cron_job(self, input).await
    }

    async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>> {
        Self::list_cron_jobs(self, status, limit).await
    }

    async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>> {
        Self::get_cron_job(self, id).await
    }

    async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool> {
        Self::update_cron_job(self, id, input).await
    }

    async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool> {
        Self::delete_cron_job(self, id).await
    }

    async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()> {
        Self::trigger_cron_job(self, id).await
    }
}

// ── RemoteCoordinator (IPC client with lazy connect) ─────────────────────

struct Connection {
    write_half: Arc<Mutex<WriteHalf>>,
    pending: Arc<PendingMap>,
    _reader: tokio::task::JoinHandle<()>,
    _heartbeat: tokio::task::JoinHandle<()>,
    /// Cancelled when the connection is dead (reader or heartbeat
    /// detected an error, or the caller explicitly killed the old
    /// connection).  `ensure_connected()` checks this to decide
    /// whether a reconnect is needed.
    cancel: tokio_util::sync::CancellationToken,
}

/// Client-side coordinator proxy that talks to a kernel daemon over IPC.
/// Uses lazy connect: the connection is established on the first API call.
pub struct RemoteCoordinator {
    addr: SocketAddr,
    req_id: RequestIdGenerator,
    connection: Arc<Mutex<Option<Connection>>>,
    /// Persistent local event routers: `session_id` -> broadcast sender.
    /// Lifetime is independent of individual connections so that receivers
    /// survive reconnects.
    event_routers: Arc<EventRouterMap>,
}

impl RemoteCoordinator {
    /// Create a lazy coordinator that connects on first use.
    pub fn new(addr: SocketAddr) -> Self {
        Self {
            addr,
            req_id: RequestIdGenerator::new(),
            connection: Arc::new(Mutex::new(None)),
            event_routers: Arc::new(EventRouterMap::new()),
        }
    }

    /// Connect immediately and return a ready coordinator.
    pub async fn connect(addr: &SocketAddr) -> Result<Self> {
        let stream = crate::transport::connect(addr).await?;
        Self::from_stream(stream, addr).await
    }

    /// Wrap an already-connected stream.
    pub async fn from_stream(stream: Stream, addr: &SocketAddr) -> Result<Self> {
        let (read_half, write_half) = stream.into_split();
        let write_half = Arc::new(Mutex::new(write_half));
        let pending: Arc<PendingMap> = Arc::new(PendingMap::new());
        let event_routers: Arc<EventRouterMap> = Arc::new(EventRouterMap::new());
        let cancel = tokio_util::sync::CancellationToken::new();
        let last_pong = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));

        let reader = Self::spawn_reader(
            read_half,
            Arc::clone(&write_half),
            Arc::clone(&pending),
            Arc::clone(&event_routers),
            Arc::clone(&last_pong),
            cancel.clone(),
        );
        let heartbeat = Self::spawn_heartbeat(Arc::clone(&write_half), last_pong, cancel.clone());

        let this = Self {
            addr: addr.clone(),
            req_id: RequestIdGenerator::new(),
            connection: Arc::new(Mutex::new(None)),
            event_routers,
        };
        *this.connection.lock().await = Some(Connection {
            write_half,
            pending,
            _reader: reader,
            _heartbeat: heartbeat,
            cancel,
        });
        Ok(this)
    }

    fn spawn_reader(
        mut read_half: ReadHalf,
        write_half: Arc<Mutex<WriteHalf>>,
        pending: Arc<PendingMap>,
        event_routers: Arc<EventRouterMap>,
        last_pong: Arc<std::sync::Mutex<tokio::time::Instant>>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    result = recv_frame(&mut read_half) => {
                        let msg = match result {
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
                                let mut guard = last_pong
                                    .lock()
                                    .unwrap_or_else(|e| e.into_inner());
                                *guard = tokio::time::Instant::now();
                            }
                            WireMsg::Request { .. } => {
                                tracing::warn!("Unexpected message from server: {:?}", msg);
                            }
                        }
                    }
                }
            }

            cancel.cancel();
            // Notify pending RPCs.
            let keys: Vec<u64> = pending.iter().map(|e| *e.key()).collect();
            for key in keys {
                if let Some((_, tx)) = pending.remove(&key) {
                    let _ = tx.send(Err(RpcError {
                        code: "connection_closed".to_string(),
                        message: "Connection to kernel daemon closed".to_string(),
                        detail: None,
                    }));
                }
            }
            // Notify all local event subscribers that the connection is
            // dead, then drop the senders so receivers become Closed.
            // This forces the UI to re-subscribe (and re-establish the
            // server-side forwarding task) instead of hanging forever
            // on an empty channel.
            let keys: Vec<String> = event_routers.iter().map(|e| e.key().clone()).collect();
            for key in &keys {
                if let Some((_, tx)) = event_routers.remove(key) {
                    let _ = tx.send(Event::System(crate::event::SystemEvent::ConnectionLost {
                        session_id: SessionId(key.clone()),
                    }));
                }
            }
        })
    }

    fn spawn_heartbeat(
        write_half: Arc<Mutex<WriteHalf>>,
        last_pong: Arc<std::sync::Mutex<tokio::time::Instant>>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                if cancel.is_cancelled() {
                    break;
                }
                let elapsed = last_pong
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .elapsed();
                if elapsed > Duration::from_secs(HEARTBEAT_TIMEOUT_SECS) {
                    tracing::warn!(
                        "Heartbeat timeout (no pong for {:?}), disconnecting",
                        elapsed
                    );
                    cancel.cancel();
                    break;
                }
                let mut w = write_half.lock().await;
                match tokio::time::timeout(
                    Duration::from_secs(3),
                    send_frame(&mut *w, &WireMsg::Ping),
                )
                .await
                {
                    Ok(Ok(())) => {}
                    Ok(Err(e)) => {
                        tracing::debug!("Heartbeat send_frame failed: {e}");
                        cancel.cancel();
                        break;
                    }
                    Err(_) => {
                        tracing::warn!("Heartbeat send_frame timed out (3s)");
                        cancel.cancel();
                        break;
                    }
                }
            }
        })
    }

    /// Ensure the connection is established (lazy on first call).
    /// Retries for up to 10 s to allow the daemon to finish spawning.
    /// On reconnect, re-subscribes all sessions in the persistent router.
    async fn ensure_connected(&self) -> Result<()> {
        let mut guard = self.connection.lock().await;
        if let Some(ref conn) = *guard {
            if !conn.cancel.is_cancelled() {
                return Ok(());
            }
        }
        if let Some(old) = guard.take() {
            // Cancel the old connection so tasks exit naturally and run
            // cleanup (notify pending RPCs, send Shutdown events, drop
            // local event router senders so receivers become Closed).
            old.cancel.cancel();
            // We do NOT abort here: abort() skips the cleanup code at
            // the end of the reader task, which means TUI receivers
            // never learn the connection is dead.
        }
        let start = tokio::time::Instant::now();
        let stream = loop {
            match crate::transport::connect(&self.addr).await {
                Ok(s) => break s,
                Err(_) if start.elapsed() < CONNECT_RETRY_TIMEOUT => {
                    tokio::time::sleep(CONNECT_RETRY_INTERVAL).await;
                }
                Err(e) => {
                    return Err(
                        SessionError::Other(format!("Failed to connect to daemon: {e}")).into(),
                    );
                }
            }
        };
        let (read_half, write_half) = stream.into_split();
        let write_half = Arc::new(Mutex::new(write_half));
        let pending: Arc<PendingMap> = Arc::new(PendingMap::new());
        let cancel = tokio_util::sync::CancellationToken::new();
        let last_pong = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));

        let reader = Self::spawn_reader(
            read_half,
            Arc::clone(&write_half),
            Arc::clone(&pending),
            Arc::clone(&self.event_routers),
            Arc::clone(&last_pong),
            cancel.clone(),
        );
        let heartbeat = Self::spawn_heartbeat(Arc::clone(&write_half), last_pong, cancel.clone());

        *guard = Some(Connection {
            write_half: Arc::clone(&write_half),
            pending,
            _reader: reader,
            _heartbeat: heartbeat,
            cancel,
        });

        // Collect sessions that still have active local receivers.
        // We drop the lock here so that `call()` (which also calls
        // `ensure_connected`) can acquire it.
        let sessions_to_resub: Vec<String> = self
            .event_routers
            .iter()
            .filter(|e| e.value().receiver_count() > 0)
            .map(|e| e.key().clone())
            .collect();
        drop(guard);

        // Re-subscribe sessions that still have active local receivers.
        // We do NOT remove stale routers here: doing so would drop the
        // `broadcast::Sender`, causing the UI's `event_rx` to become
        // `Closed` and the TUI to exit immediately.  Instead we leave
        // the router in place; the UI will learn that the session is
        // gone when subsequent `send_message` calls return
        // `session_not_found`.
        for sid in sessions_to_resub {
            if let Err(e) = Box::pin(self.call(RequestMethod::Subscribe { session_id: sid })).await
            {
                tracing::warn!("Re-subscribe failed: {e}");
            }
        }

        // Wire protocol version handshake.
        match self.call_raw(RequestMethod::Hello).await {
            Ok(val) => {
                let server_proto = val
                    .get("proto")
                    .and_then(|v| v.as_u64())
                    .map_or(0, |n| n as u32);
                let client_proto = crate::wire::WIRE_PROTOCOL_VERSION;
                if server_proto != client_proto {
                    tracing::error!(
                        "Wire protocol version mismatch: server v{}, client v{}",
                        server_proto,
                        client_proto,
                    );
                    self.invalidate_connection().await;
                    return Err(SessionError::WireProtocolMismatch.into());
                }
            }
            Err(e) => {
                // Old daemon that doesn't recognise `Hello` will close the
                // connection (serde unknown variant). Treat this as a fatal
                // mismatch rather than silently degrading.
                tracing::error!("Hello handshake failed (old daemon?): {e}");
                self.invalidate_connection().await;
                return Err(SessionError::WireProtocolMismatch.into());
            }
        }

        Ok(())
    }

    async fn invalidate_connection(&self) {
        let mut guard = self.connection.lock().await;
        if let Some(ref conn) = guard.take() {
            conn.cancel.cancel();
        }
    }

    async fn call_raw(&self, method: RequestMethod) -> Result<serde_json::Value> {
        let id = self.req_id.next();

        // Grab write_half and install pending oneshot, then drop the
        // connection lock so we don't hold it across the network await.
        let (write_half, rx) = {
            let guard = self.connection.lock().await;
            let conn = guard
                .as_ref()
                .ok_or_else(|| KernelError::from(SessionError::ConnectionLost))?;
            let (tx, rx) = tokio::sync::oneshot::channel();
            conn.pending.insert(id, tx);
            (Arc::clone(&conn.write_half), rx)
        };

        let msg = WireMsg::Request { id, method };
        {
            let mut w = write_half.lock().await;
            match tokio::time::timeout(Duration::from_secs(5), send_frame(&mut *w, &msg)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    drop(w);
                    self.invalidate_connection().await;
                    return Err(SessionError::SendFailed(e.to_string()).into());
                }
                Err(_) => {
                    drop(w);
                    self.invalidate_connection().await;
                    return Err(SessionError::SendFailed("write timeout (5s)".to_string()).into());
                }
            }
        }

        match tokio::time::timeout(RPC_TIMEOUT, rx).await {
            Ok(Ok(Ok(val))) => Ok(val),
            Ok(Ok(Err(e))) => {
                // If the server sent a structured session error, try to
                // reconstruct it exactly instead of losing the variant.
                if e.code == "session_error" {
                    if let Some(ref d) = e.detail {
                        if let Ok(se) = serde_json::from_value::<SessionError>(d.clone()) {
                            return Err(KernelError::from(se));
                        }
                    }
                    return Err(SessionError::Other(format!(
                        "RPC session error [{}]: {}",
                        e.code, e.message
                    ))
                    .into());
                }
                Err(SessionError::Other(format!("RPC error [{}]: {}", e.code, e.message)).into())
            }
            Ok(Err(_)) => Err(SessionError::Cancelled.into()),
            Err(_) => {
                // RPC timeout usually means the reader task is stuck or
                // the server is dead.  Force a reconnect on the next
                // call by dropping the connection.
                self.invalidate_connection().await;
                Err(SessionError::RequestTimeout.into())
            }
        }
    }

    async fn call(&self, method: RequestMethod) -> Result<serde_json::Value> {
        self.ensure_connected().await?;
        self.call_raw(method).await
    }

    async fn subscribe_events_internal(
        &self,
        session_id: &SessionId,
    ) -> Result<broadcast::Receiver<Event>> {
        use dashmap::mapref::entry::Entry;

        let tx = match self.event_routers.entry(session_id.0.clone()) {
            Entry::Occupied(entry) => entry.get().clone(),
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
        if let Err(ref e) = result {
            // Only remove the local router when the server explicitly
            // says the session is gone.  Transient errors (timeout, write
            // failure) should leave the router in place so that a later
            // re-subscribe can reuse the same sender.
            if e.is_session_not_found() {
                self.event_routers.remove(&session_id.0);
            }
            return Err(result.unwrap_err());
        }
        Ok(tx.subscribe())
    }
}

#[async_trait]
impl CoordinatorApi for RemoteCoordinator {
    async fn list_projects(&self) -> Result<Vec<Project>> {
        let result = self.call(RequestMethod::ListProjects).await?;
        let projects: Vec<Project> = serde_json::from_value(result)?;
        Ok(projects)
    }

    async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        let result = self
            .call(RequestMethod::CreateProject {
                dir: dir.to_string_lossy().to_string(),
                name,
            })
            .await?;
        let project: Project = serde_json::from_value(result)?;
        Ok(project)
    }

    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        let result = self
            .call(RequestMethod::GetProject {
                project_id: id.0.clone(),
            })
            .await?;
        let project: Option<Project> = serde_json::from_value(result)?;
        Ok(project)
    }

    async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()> {
        self.call(RequestMethod::RenameProject {
            project_id: id.0.clone(),
            name,
        })
        .await?;
        Ok(())
    }

    async fn delete_project(&self, id: &ProjectId) -> Result<()> {
        self.call(RequestMethod::DeleteProject {
            project_id: id.0.clone(),
        })
        .await?;
        Ok(())
    }

    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        let result = self
            .call(RequestMethod::CreateSession {
                project_id: input.project_id.map(|p| p.0),
                working_dir: input.working_dir.map(|p| p.to_string_lossy().to_string()),
                auto_approve_level: input.auto_approve_level,
            })
            .await?;
        let sid: String = serde_json::from_value(result)?;
        Ok(SessionId(sid))
    }

    async fn restore_session(&self, id: &SessionId) -> Result<SessionId> {
        let result = self
            .call(RequestMethod::RestoreSession {
                session_id: id.0.clone(),
            })
            .await?;
        let sid: String = serde_json::from_value(result)?;
        Ok(SessionId(sid))
    }

    async fn fork_session(
        &self,
        parent: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        let result = self
            .call(RequestMethod::ForkSession {
                parent_id: parent.0.clone(),
                auto_approve_level,
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

    async fn rename_session(&self, session_id: &SessionId, title: String) -> Result<()> {
        self.call(RequestMethod::RenameSession {
            session_id: session_id.0.clone(),
            title,
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

    async fn pause_goal(&self, session_id: &SessionId) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::PauseGoal,
        })
        .await?;
        Ok(())
    }

    async fn resume_goal(&self, session_id: &SessionId) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::ResumeGoal,
        })
        .await?;
        Ok(())
    }

    async fn get_goal(&self, session_id: &SessionId) -> Result<Option<crate::goal::GoalState>> {
        let result = self
            .call(RequestMethod::Command {
                session_id: session_id.0.clone(),
                cmd: ControlCommand::GetGoal,
            })
            .await?;
        let goal: Option<crate::goal::GoalState> = serde_json::from_value(result)?;
        Ok(goal)
    }

    async fn update_goal(&self, session_id: &SessionId, description: String) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::EditGoal { description },
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

    async fn get_session_status(
        &self,
        session_id: &SessionId,
    ) -> Result<crate::types::SessionStatus> {
        let result = self
            .call(RequestMethod::GetSessionStatus {
                session_id: session_id.0.clone(),
            })
            .await?;
        let status: crate::types::SessionStatus = serde_json::from_value(result)?;
        Ok(status)
    }

    async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Result<broadcast::Receiver<Event>> {
        self.subscribe_events_internal(session_id).await
    }

    async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<PaginatedSessions> {
        let result = self
            .call(RequestMethod::ListSessions {
                project_id: project_id.map(|p| p.0.clone()),
                before,
                limit,
            })
            .await?;
        let sessions: Vec<crate::storage::session::SessionInfo> = serde_json::from_value(result)?;
        // Remote server doesn't return has_more separately in this version;
        // we infer from the result length.
        let has_more = sessions.len() > limit;
        let sessions = sessions.into_iter().take(limit).collect();
        Ok(PaginatedSessions { sessions, has_more })
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

    async fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::AskUserResponse {
                req_id: req_id.to_string(),
                answers: response.answers.into_iter().collect(),
            },
        })
        .await?;
        Ok(())
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

    async fn reload_agent_config(&self) -> Result<()> {
        self.call(RequestMethod::ReloadAgentConfig).await?;
        Ok(())
    }

    async fn send_steer(&self, session_id: &SessionId, content: Vec<ContentBlock>) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::Steer { content },
        })
        .await?;
        Ok(())
    }

    async fn send_continue(&self, session_id: &SessionId) -> Result<()> {
        self.call(RequestMethod::Command {
            session_id: session_id.0.clone(),
            cmd: ControlCommand::Continue,
        })
        .await?;
        Ok(())
    }

    async fn get_usage_summary(&self, days: i64) -> Result<crate::storage::usage::UsageSummary> {
        let result = self
            .call(RequestMethod::GetUsageSummary { days: Some(days) })
            .await?;
        let summary = serde_json::from_value(result)?;
        Ok(summary)
    }

    async fn get_daily_usage(&self, days: i64) -> Result<Vec<crate::storage::usage::DailyUsage>> {
        let result = self.call(RequestMethod::GetDailyUsage { days }).await?;
        let daily: Vec<crate::storage::usage::DailyUsage> = serde_json::from_value(result)?;
        Ok(daily)
    }

    async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId> {
        let result = self
            .call(RequestMethod::CreateCronJob {
                name: input.name,
                schedule: input.schedule,
                action: input.action,
                max_runs: input.max_runs,
                expires_at: input.expires_at,
            })
            .await?;
        let job_id = result
            .get("job_id")
            .and_then(|v| v.as_str())
            .map(|s| crate::cron::CronJobId(s.to_string()))
            .ok_or_else(|| SessionError::Other("Missing job_id in response".to_string()))?;
        Ok(job_id)
    }

    async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>> {
        let result = self
            .call(RequestMethod::ListCronJobs {
                status: status.map(|s| s.as_str().to_string()),
                limit,
            })
            .await?;
        let jobs: Vec<crate::cron::CronJob> = serde_json::from_value(result)?;
        Ok(jobs)
    }

    async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>> {
        let result = self
            .call(RequestMethod::GetCronJob {
                job_id: id.0.clone(),
            })
            .await?;
        if result.is_null() {
            return Ok(None);
        }
        let job = serde_json::from_value(result)?;
        Ok(Some(job))
    }

    async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool> {
        let result = self
            .call(RequestMethod::UpdateCronJob {
                job_id: id.0.clone(),
                name: input.name,
                schedule: input.schedule,
                action: input.action,
                status: input.status.map(|s| s.as_str().to_string()),
                max_runs: input.max_runs,
                expires_at: input.expires_at,
            })
            .await?;
        let updated: bool = serde_json::from_value(result)?;
        Ok(updated)
    }

    async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool> {
        let result = self
            .call(RequestMethod::DeleteCronJob {
                job_id: id.0.clone(),
            })
            .await?;
        let deleted: bool = serde_json::from_value(result)?;
        Ok(deleted)
    }

    async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()> {
        self.call(RequestMethod::TriggerCronJob {
            job_id: id.0.clone(),
        })
        .await?;
        Ok(())
    }
}
