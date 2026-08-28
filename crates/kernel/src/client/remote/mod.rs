//! Remote (IPC) kernel client: connection management, heartbeat, lazy
//! reconnect, and the raw RPC plumbing. The typed `KernelApi` impl lives
//! in [`api`].

mod api;

use crate::notification::Notification;
use crate::transport::{recv_frame, send_frame, ReadHalf, SocketAddr, Stream, WriteHalf};
use crate::types::{KernelError, Result, SessionError, SessionId};
use crate::wire::{Envelope, ReqMethod, RequestIdGenerator, RespBody, RpcError, WireMsg};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc, Mutex};

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
type EventRouterMap = dashmap::DashMap<String, broadcast::Sender<Envelope>>;

/// Router key collecting events from **all** sessions (used by
/// `subscribe_all_events`; session IDs are ULIDs, so "*" never collides).
const ALL_EVENTS_ROUTER_KEY: &str = "*";

/// Resolve the effective socket auth token for a connect: a non-empty
/// explicit token wins over the `YOMI_SOCKET_AUTH` env value; a missing
/// or whitespace-only explicit token falls back to the env value (the
/// GUI submits the mask field empty when the user leaves it blank).
fn resolve_auth_token(explicit: Option<String>, env_token: Option<String>) -> Option<String> {
    explicit.filter(|t| !t.trim().is_empty()).or(env_token)
}

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

/// Client-side kernel proxy that talks to a kernel daemon over IPC.
/// Uses lazy connect: the connection is established on the first API call.
pub struct RemoteKernel {
    addr: SocketAddr,
    /// Socket auth token sent on ws/wss connects (and reconnects).
    /// Resolved from `YOMI_SOCKET_AUTH` unless an explicit token was
    /// passed to [`Self::connect_with_auth`].
    auth_token: Option<String>,
    req_id: RequestIdGenerator,
    connection: Arc<Mutex<Option<Connection>>>,
    /// Persistent local event routers: `session_id` -> broadcast sender.
    /// Lifetime is independent of individual connections so that receivers
    /// survive reconnects.
    event_routers: Arc<EventRouterMap>,
    /// Local broadcast channel for notifications received from the wire.
    notification_tx: broadcast::Sender<Notification>,
}

impl RemoteKernel {
    /// Create a lazy kernel that connects on first use.
    pub fn new(addr: SocketAddr) -> Self {
        let (notification_tx, _) = broadcast::channel(256);
        Self {
            addr,
            auth_token: crate::transport::socket_auth_token(),
            req_id: RequestIdGenerator::new(),
            connection: Arc::new(Mutex::new(None)),
            event_routers: Arc::new(EventRouterMap::new()),
            notification_tx,
        }
    }

    /// Connect immediately and return a ready kernel.
    ///
    /// Uses the socket auth token from `YOMI_SOCKET_AUTH`, if set.
    pub async fn connect(addr: &SocketAddr) -> Result<Self> {
        Self::connect_with_auth(addr, None).await
    }

    /// Connect immediately with an explicit socket auth token. A
    /// non-empty token overrides `YOMI_SOCKET_AUTH`; when absent or
    /// blank, the env value is used instead. The resolved token is
    /// reused on reconnects.
    pub async fn connect_with_auth(addr: &SocketAddr, auth_token: Option<String>) -> Result<Self> {
        let auth_token = resolve_auth_token(auth_token, crate::transport::socket_auth_token());
        let stream = crate::transport::connect_with_token(addr, auth_token.as_deref()).await?;
        let mut this = Self::from_stream(stream, addr).await?;
        this.auth_token = auth_token;
        this.validate_wire_protocol().await?;
        Ok(this)
    }

    /// Wrap an already-connected stream.
    pub async fn from_stream(stream: Stream, addr: &SocketAddr) -> Result<Self> {
        let (read_half, write_half) = stream.into_split();
        let write_half = Arc::new(Mutex::new(write_half));
        let pending: Arc<PendingMap> = Arc::new(PendingMap::new());
        let event_routers: Arc<EventRouterMap> = Arc::new(EventRouterMap::new());
        let cancel = tokio_util::sync::CancellationToken::new();
        let last_pong = Arc::new(std::sync::Mutex::new(tokio::time::Instant::now()));
        let (notification_tx, _) = broadcast::channel(256);

        let reader = Self::spawn_reader(
            read_half,
            Arc::clone(&write_half),
            Arc::clone(&pending),
            Arc::clone(&event_routers),
            notification_tx.clone(),
            Arc::clone(&last_pong),
            cancel.clone(),
        );
        let heartbeat = Self::spawn_heartbeat(Arc::clone(&write_half), last_pong, cancel.clone());

        let this = Self {
            addr: addr.clone(),
            auth_token: crate::transport::socket_auth_token(),
            req_id: RequestIdGenerator::new(),
            connection: Arc::new(Mutex::new(None)),
            event_routers,
            notification_tx,
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
        notification_tx: broadcast::Sender<Notification>,
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
                                if e.kind() == std::io::ErrorKind::InvalidData {
                                    // Daemon sent an oversized or malformed frame.
                                    tracing::warn!("Inbound frame rejected: {e}");
                                } else {
                                    tracing::warn!("Remote reader error: {e}");
                                }
                                break;
                            }
                        };

                        match msg {
                            WireMsg::Response { id, body } => {
                                let result = match body {
                                    RespBody::Ok { result } => Ok(result),
                                    RespBody::Err { error } => Err(error),
                                };
                                if let Some((_, tx)) = pending.remove(&id) {
                                    let _ = tx.send(result);
                                }
                            }
                            WireMsg::Event(envelope) => {
                                if let Some(entry) = event_routers.get(envelope.session_id.as_str()) {
                                    let _ = entry.value().send(envelope.clone());
                                }
                                if let Some(entry) = event_routers.get(ALL_EVENTS_ROUTER_KEY) {
                                    let _ = entry.value().send(envelope);
                                }
                            }
                            WireMsg::Noti(noti) => {
                                let _ = notification_tx.send(noti);
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
            // Keep persistent routers alive across reconnects. Existing receivers
            // continue consuming from the same broadcast senders after the new
            // connection re-subscribes server-side.
            for entry in event_routers.iter() {
                let _ = notification_tx.send(Notification::ConnectionLost {
                    session_id: SessionId::from(entry.key().clone()),
                });
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
                        tracing::warn!("Heartbeat send_frame failed: {e}");
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

    pub async fn check_ready(&self) -> Result<()> {
        self.ensure_connected().await
    }

    async fn server_instance_id(&self) -> Result<String> {
        self.ensure_connected().await?;
        let value = self.call_raw(ReqMethod::Hello).await?;
        value
            .get("instance_id")
            .and_then(|value| value.as_str())
            .map(ToOwned::to_owned)
            .ok_or_else(|| SessionError::WireProtocolMismatch.into())
    }

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
            match crate::transport::connect_with_token(&self.addr, self.auth_token.as_deref()).await
            {
                Ok(s) => break s,
                // Auth rejection (missing/wrong token) is deterministic:
                // retrying would only delay the user-facing error, and
                // each failed handshake costs the daemon's serialized
                // accept loop a throttling delay.
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    return Err(
                        SessionError::Other(format!("Failed to connect to daemon: {e}")).into(),
                    );
                }
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
            self.notification_tx.clone(),
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
            if let Err(e) = Box::pin(self.call(ReqMethod::Subscribe {
                session_id: sid,
                after_event_id: None,
            }))
            .await
            {
                tracing::warn!("Re-subscribe failed: {e}");
            }
        }

        // Wire protocol version handshake.
        self.validate_wire_protocol().await?;

        Ok(())
    }

    async fn validate_wire_protocol(&self) -> Result<()> {
        // Wire protocol version handshake.
        match self.call_raw(ReqMethod::Hello).await {
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

    async fn call_raw(&self, method: ReqMethod) -> Result<serde_json::Value> {
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

    /// Send a raw wire request and return the untyped result value.
    ///
    /// Escape hatch for tooling (e.g. `yomi rpc`) that talks the wire
    /// protocol directly without a typed `KernelApi` wrapper. Prefer the
    /// typed trait methods for anything permanent. Streaming methods
    /// (`Subscribe`/`SubscribeAll`) only return an ack here — use
    /// `subscribe_session_events`/`subscribe_all_events` to follow events.
    pub async fn call(&self, method: ReqMethod) -> Result<serde_json::Value> {
        self.ensure_connected().await?;
        self.call_raw(method).await
    }

    /// RPC call with a typed JSON response — the common case for the
    /// `KernelApi` impl in [`api`].
    async fn call_json<T: serde::de::DeserializeOwned>(&self, method: ReqMethod) -> Result<T> {
        Ok(serde_json::from_value(self.call(method).await?)?)
    }

    /// RPC call whose response value is discarded.
    async fn call_unit(&self, method: ReqMethod) -> Result<()> {
        self.call(method).await?;
        Ok(())
    }

    async fn subscribe_events_internal(
        &self,
        session_id: &SessionId,
        after_event_id: Option<crate::types::EventId>,
    ) -> Result<crate::comms::EventBusSubscriber> {
        use dashmap::mapref::entry::Entry;

        let tx = match self.event_routers.entry(session_id.0.to_string()) {
            Entry::Occupied(entry) => entry.get().clone(),
            Entry::Vacant(entry) => {
                let (tx, _rx) = broadcast::channel(256);
                entry.insert(tx.clone());
                tx
            }
        };

        let result = self
            .call(ReqMethod::Subscribe {
                session_id: session_id.0.to_string(),
                after_event_id,
            })
            .await;
        if let Err(ref e) = result {
            // Only remove the local router when the server explicitly
            // says the session is gone.  Transient errors (timeout, write
            // failure) should leave the router in place so that a later
            // re-subscribe can reuse the same sender.
            if e.is_session_not_found() {
                self.event_routers.remove(session_id.0.as_str());
            }
            return Err(result.unwrap_err());
        }

        let mut broadcast_rx = tx.subscribe();
        let (mpsc_tx, mpsc_rx) = mpsc::channel::<(SessionId, crate::wire::Envelope)>(256);
        let sid = session_id.clone();
        tokio::spawn(async move {
            while let Ok(ev) = broadcast_rx.recv().await {
                if mpsc_tx.send((sid.clone(), ev)).await.is_err() {
                    break;
                }
            }
        });

        Ok(crate::comms::EventBusSubscriber::from_receiver(mpsc_rx))
    }
}

#[cfg(test)]
#[path = "mod_test.rs"]
mod tests;
