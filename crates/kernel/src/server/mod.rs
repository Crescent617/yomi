use crate::agent::AgentConfig;
use crate::app::{Coordinator, SessionConfig};
use crate::transport::{recv_frame, send_frame};
use crate::types::{Result, SessionId};
use crate::wire::{RequestMethod, ResponseBody, RpcError, WireMsg};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

/// Kernel daemon server. Bridges external connections to the local Coordinator.
#[derive(Clone)]
pub struct KernelServer {
    coordinator: Arc<Coordinator>,
    agent_config: AgentConfig,
    data_dir: PathBuf,
    connections: Arc<dashmap::DashMap<u64, ConnectionHandle>>,
    next_conn_id: Arc<std::sync::atomic::AtomicU64>,
}

struct ConnectionHandle {
    cancel: tokio_util::sync::CancellationToken,
}

impl KernelServer {
    pub fn new(
        coordinator: Arc<Coordinator>,
        agent_config: AgentConfig,
        data_dir: PathBuf,
    ) -> Self {
        Self {
            coordinator,
            agent_config,
            data_dir,
            connections: Arc::new(dashmap::DashMap::new()),
            next_conn_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    /// Run the server on a Unix socket (binds and listens).
    pub async fn serve_unix(
        &self,
        path: &Path,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        let listener = crate::transport::unix::bind_socket(path).await?;
        tracing::info!("KernelServer listening on {}", path.display());
        self.serve_listener(listener, shutdown).await
    }

    /// Run the server on an already-bound Unix listener.
    /// Stops accepting new connections when `shutdown` is cancelled.
    pub async fn serve_listener(
        &self,
        listener: tokio::net::UnixListener,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<()> {
        loop {
            tokio::select! {
                biased;
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
                    let coordinator = Arc::clone(&self.coordinator);
                    let agent_config = self.agent_config.clone();
                    let data_dir = self.data_dir.clone();
                    let connections = Arc::clone(&self.connections);
                    tokio::spawn(async move {
                        let cancel = tokio_util::sync::CancellationToken::new();
                        connections.insert(conn_id, ConnectionHandle { cancel: cancel.clone() });

                        if let Err(e) = handle_connection(
                            stream,
                            coordinator,
                            agent_config,
                            data_dir,
                            cancel.clone(),
                        )
                        .await
                        {
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

    /// Gracefully close all connections.
    pub async fn shutdown(&self) {
        for entry in self.connections.iter() {
            entry.value().cancel.cancel();
        }
    }

    pub fn connection_count(&self) -> usize {
        self.connections.len()
    }
}

/// Handle a single client connection.
async fn handle_connection(
    stream: tokio::net::UnixStream,
    coordinator: Arc<Coordinator>,
    agent_config: AgentConfig,
    data_dir: PathBuf,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<()> {
    let (mut read_half, mut write_half) = stream.into_split();

    // Channel for serializing outbound messages.
    const OUTBOUND_CHANNEL_SIZE: usize = 4096;
    let (send_tx, mut send_rx) = mpsc::channel::<WireMsg>(OUTBOUND_CHANNEL_SIZE);

    // Spawn writer task.
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

    // Track subscriptions: session_id -> broadcast receiver task handle.
    let subscriptions: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>> =
        Arc::new(RwLock::new(HashMap::new()));

    // Main recv loop.
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
                let body = dispatch_request(
                    Arc::clone(&coordinator),
                    agent_config.clone(),
                    data_dir.clone(),
                    Arc::clone(&subscriptions),
                    send_tx.clone(),
                    cancel.clone(),
                    method,
                )
                .await;
                if let Err(e) = send_tx.send(WireMsg::Response { id, body }).await {
                    tracing::debug!("Outbound channel closed, dropping response for id={id}: {e}");
                    break;
                }
            }
            WireMsg::Event { .. } | WireMsg::Response { .. } => {
                tracing::warn!("Unexpected message from client: {:?}", msg);
            }
            WireMsg::Pong => {
                // No-op.
            }
        }
    }

    // Clean up subscriptions.
    let subs = subscriptions.write().await;
    for (_, handle) in subs.iter() {
        handle.abort();
    }
    drop(subs);

    // Cancel writer and wait for it to finish.
    cancel.cancel();
    let _ = writer.await;

    Ok(())
}

async fn dispatch_request(
    coordinator: Arc<Coordinator>,
    agent_config: AgentConfig,
    data_dir: PathBuf,
    subscriptions: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
    send_tx: mpsc::Sender<WireMsg>,
    cancel: tokio_util::sync::CancellationToken,
    method: RequestMethod,
) -> ResponseBody {
    match method {
        RequestMethod::CreateSession {
            project_path,
            auto_approve_level,
        } => {
            let config = SessionConfig {
                agent: agent_config,
                project_path: project_path.into(),
                auto_approve_level,
                data_dir,
            };
            rpc_body(
                "create_session_failed",
                coordinator.create_session(config).await.map(|sid| sid.0),
            )
        }
        RequestMethod::RestoreSession {
            session_id,
            project_path,
            auto_approve_level,
        } => {
            let sid = SessionId(session_id);
            let config = SessionConfig {
                agent: agent_config,
                project_path: project_path.into(),
                auto_approve_level,
                data_dir,
            };
            rpc_body(
                "restore_session_failed",
                coordinator
                    .restore_session(&sid, config)
                    .await
                    .map(|sid| sid.0),
            )
        }
        RequestMethod::ForkSession {
            parent_id,
            project_path,
            auto_approve_level,
        } => {
            let parent = SessionId(parent_id);
            let config = SessionConfig {
                agent: agent_config,
                project_path: project_path.into(),
                auto_approve_level,
                data_dir,
            };
            rpc_body(
                "fork_session_failed",
                coordinator
                    .fork_session(&parent, config)
                    .await
                    .map(|sid| sid.0),
            )
        }
        RequestMethod::SendMessage { session_id, blocks } => rpc_body(
            "send_message_failed",
            coordinator
                .send_message(&SessionId(session_id), blocks)
                .await
                .map(|()| serde_json::Value::Null),
        ),
        RequestMethod::Command { session_id, cmd } => {
            let sid = SessionId(session_id);
            rpc_body(
                "command_failed",
                dispatch_command(&coordinator, &sid, cmd)
                    .await
                    .map(|()| serde_json::Value::Null),
            )
        }
        RequestMethod::Subscribe { session_id } => {
            let sid = SessionId(session_id.clone());
            match coordinator.subscribe_session_events(&sid).await {
                Some(rx) => {
                    let session_id_for_task = session_id.clone();
                    let send_tx2 = send_tx.clone();
                    let cancel2 = cancel.clone();

                    let mut subs = subscriptions.write().await;
                    if let Some(old) = subs.remove(&session_id) {
                        old.abort();
                    }

                    let handle = tokio::spawn(async move {
                        let mut rx = rx;
                        loop {
                            let event = tokio::select! {
                                biased;
                                () = cancel2.cancelled() => break,
                                result = rx.recv() => match result {
                                    Ok(ev) => ev,
                                    Err(_) => break,
                                },
                            };
                            let msg = WireMsg::Event {
                                session_id: session_id_for_task.clone(),
                                event,
                            };
                            if let Err(e) = send_tx2.try_send(msg) {
                                match e {
                                    tokio::sync::mpsc::error::TrySendError::Full(_) => {
                                        tracing::warn!(
                                            "Outbound channel full, dropping event for session={}",
                                            session_id_for_task
                                        );
                                    }
                                    tokio::sync::mpsc::error::TrySendError::Closed(_) => break,
                                }
                            }
                        }
                    });

                    subs.insert(session_id, handle);
                    ResponseBody::Ok {
                        result: serde_json::Value::Null,
                    }
                }
                None => ResponseBody::Err {
                    error: RpcError {
                        code: "session_not_found".to_string(),
                        message: format!("Session {} not found", sid.0),
                    },
                },
            }
        }
        RequestMethod::Unsubscribe { session_id } => {
            if let Some(handle) = subscriptions.write().await.remove(&session_id) {
                handle.abort();
            }
            ResponseBody::Ok {
                result: serde_json::Value::Null,
            }
        }
        RequestMethod::ListSessions => {
            let sessions = coordinator.list_sessions().await;
            rpc_body(
                "list_sessions_failed",
                Ok(sessions.into_iter().map(|s| s.0).collect::<Vec<String>>()),
            )
        }
        RequestMethod::GetSessionMessages { session_id } => rpc_body(
            "get_messages_failed",
            coordinator
                .get_session_messages(&SessionId(session_id))
                .await,
        ),
        RequestMethod::DeleteSession { session_id } => rpc_body(
            "delete_failed",
            coordinator
                .delete_session(&SessionId(session_id))
                .await
                .map(|()| serde_json::Value::Null),
        ),
        RequestMethod::ListSessionsFiltered(args) => rpc_body(
            "list_sessions_failed",
            coordinator.list_sessions_filtered(args).await,
        ),
        RequestMethod::GetCheckpoints { session_id } => rpc_body(
            "get_checkpoints_failed",
            coordinator.get_checkpoints(&SessionId(session_id)).await,
        ),
        RequestMethod::GetTodos { session_id } => rpc_body(
            "get_todos_failed",
            coordinator.get_todos(&SessionId(session_id)).await,
        ),
        RequestMethod::ShutdownSession { session_id } => rpc_body(
            "shutdown_failed",
            coordinator
                .shutdown_session(&SessionId(session_id))
                .await
                .map(|()| serde_json::Value::Null),
        ),
    }
}

async fn dispatch_command(
    coordinator: &Coordinator,
    sid: &SessionId,
    cmd: crate::event::ControlCommand,
) -> Result<()> {
    use crate::event::ControlCommand;
    match cmd {
        ControlCommand::Cancel => coordinator.cancel(sid).await?,
        ControlCommand::Response {
            req_id,
            approved,
            remember,
        } => {
            coordinator
                .send_permission_response(sid, &req_id, approved, remember)
                .await?;
        }
        ControlCommand::SetLevel(level) => {
            coordinator.set_permission_level(sid, level).await?;
        }
        ControlCommand::Compact => {
            coordinator.compact_session(sid).await?;
        }
        ControlCommand::StartGoal(state) => {
            coordinator.start_goal(sid, state).await?;
        }
        ControlCommand::StopGoal => {
            coordinator.stop_goal(sid).await?;
        }
        ControlCommand::Rewind { message_id, target } => {
            coordinator.rewind_session(sid, message_id, target).await?;
        }
    }
    Ok(())
}

fn rpc_body<T: serde::Serialize>(code: &str, result: crate::types::Result<T>) -> ResponseBody {
    match result {
        Ok(val) => match serde_json::to_value(val) {
            Ok(v) => ResponseBody::Ok { result: v },
            Err(e) => ResponseBody::Err {
                error: RpcError {
                    code: "serialize_error".to_string(),
                    message: e.to_string(),
                },
            },
        },
        Err(e) => ResponseBody::Err {
            error: RpcError {
                code: code.to_string(),
                message: e.to_string(),
            },
        },
    }
}
