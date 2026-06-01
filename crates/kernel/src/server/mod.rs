use crate::agent::AgentConfig;
use crate::app::coordinator::CreateSessionInput;
use crate::app::Coordinator;
use crate::config::Config;
use crate::skill::{deduplicate_skills, SkillLoader};
use crate::transport::{recv_frame, send_frame};
use crate::types::{KernelError, ProjectId, Result, SessionError, SessionId};
use crate::wire::{RequestMethod, ResponseBody, RpcError, WireMsg};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

/// Load skills from disk and build a complete `AgentConfig` from a `Config`.
/// `base_dir` is used to resolve relative skill folder paths.
pub fn build_agent_config(config: &Config, base_dir: &Path) -> AgentConfig {
    let skill_folders = config
        .skill_folders()
        .iter()
        .map(PathBuf::from)
        .map(|p| if p.is_relative() { base_dir.join(p) } else { p })
        .collect::<Vec<_>>();

    let mut skills = SkillLoader::new(skill_folders)
        .load_all()
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to load skills: {e}");
            Vec::new()
        });

    deduplicate_skills(&mut skills);

    if !skills.is_empty() {
        tracing::info!("Loaded {} skill(s)", skills.len());
        for skill in &skills {
            tracing::debug!("  - {} (from {})", skill.name, skill.source_path.display());
        }
    }

    let mut agent = config.agent.clone();
    agent.skills = skills;
    agent
}

fn reload_config(file_path: Option<&PathBuf>, base_dir: &Path) -> Config {
    let mut config = match file_path {
        Some(path) => Config::from_file(path).unwrap_or_else(|e| {
            tracing::error!(
                "Failed to load config from {}: {e}, falling back to default",
                path.display()
            );
            Config::default()
        }),
        None => match Config::discover_file() {
            Some(path) => Config::from_file(&path).unwrap_or_else(|e| {
                tracing::error!(
                    "Failed to load discovered config from {}: {e}, falling back to default",
                    path.display()
                );
                Config::default()
            }),
            None => Config::default(),
        },
    };
    config.apply_env_overrides();
    config.finalize(base_dir);
    config
}

/// Kernel daemon server. Bridges external connections to the local Coordinator.
#[derive(Clone)]
pub struct KernelServer {
    coordinator: Arc<Coordinator>,
    config_file_path: Option<PathBuf>,
    base_dir: PathBuf,
    reload_lock: Arc<tokio::sync::Mutex<()>>,
    connections: Arc<dashmap::DashMap<u64, tokio_util::sync::CancellationToken>>,
    next_conn_id: Arc<std::sync::atomic::AtomicU64>,
}

impl KernelServer {
    pub fn new(
        coordinator: Arc<Coordinator>,
        config_file_path: Option<PathBuf>,
        base_dir: PathBuf,
    ) -> Self {
        Self {
            coordinator,
            config_file_path,
            base_dir,
            reload_lock: Arc::new(tokio::sync::Mutex::new(())),
            connections: Arc::new(dashmap::DashMap::new()),
            next_conn_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
        }
    }

    /// Reload agent configuration from disk.
    /// Returns `true` if reload succeeded, `false` if it fell back to defaults.
    pub async fn reload(&self) -> bool {
        let _guard = self.reload_lock.lock().await;
        let file_path = self.config_file_path.clone();
        let base_dir = self.base_dir.clone();
        let (new_agent, hook_registry) = match tokio::task::spawn_blocking(move || {
            let config = reload_config(file_path.as_ref(), &base_dir);
            let agent = build_agent_config(&config, &base_dir);
            let hooks = config
                .features
                .hooks
                .then(|| crate::hooks::build_registry(&config.hooks));
            (agent, hooks)
        })
        .await
        {
            Ok(pair) => pair,
            Err(e) => {
                tracing::error!("Reload task panicked: {e}");
                return false;
            }
        };
        let model_id = new_agent.model.model_id.clone();
        let skill_count = new_agent.skills.len();
        self.coordinator
            .update_agent_config(new_agent, hook_registry)
            .await;
        tracing::info!("Reloaded agent config (model={model_id}, {skill_count} skill(s))");
        true
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
                    tokio::spawn(async move {
                        let cancel = tokio_util::sync::CancellationToken::new();
                        connections.insert(conn_id, cancel.clone());

                        if let Err(e) = server.handle_connection(stream, cancel.clone()).await {
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

    pub async fn shutdown(&self) {
        for entry in self.connections.iter() {
            entry.value().cancel();
        }
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

        let subscriptions: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>> =
            Arc::new(RwLock::new(HashMap::new()));

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
                            Arc::clone(&subscriptions),
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
                WireMsg::Event { .. } | WireMsg::Response { .. } => {
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

    async fn dispatch_request(
        &self,
        subscriptions: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
        send_tx: mpsc::Sender<WireMsg>,
        cancel: tokio_util::sync::CancellationToken,
        method: RequestMethod,
    ) -> ResponseBody {
        match method {
            // ── Project ──────────────────────────────────────────────────
            RequestMethod::ListProjects => rpc_body(
                "list_projects_failed",
                self.coordinator.list_projects().await,
            ),
            RequestMethod::CreateProject { dir, name } => rpc_body(
                "create_project_failed",
                self.coordinator.create_project(dir.into(), name).await,
            ),
            RequestMethod::GetProject { project_id } => rpc_body(
                "get_project_failed",
                self.coordinator.get_project(&ProjectId(project_id)).await,
            ),
            RequestMethod::RenameProject { project_id, name } => rpc_body(
                "rename_project_failed",
                self.coordinator
                    .rename_project(&ProjectId(project_id), name)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            RequestMethod::DeleteProject { project_id } => rpc_body(
                "delete_project_failed",
                self.coordinator
                    .delete_project(&ProjectId(project_id))
                    .await
                    .map(|()| serde_json::Value::Null),
            ),

            // ── Session ──────────────────────────────────────────────────
            RequestMethod::CreateSession {
                project_id,
                working_dir,
                auto_approve_level,
            } => {
                let input = CreateSessionInput {
                    project_id: project_id.map(ProjectId),
                    working_dir: working_dir.map(std::path::PathBuf::from),
                    auto_approve_level,
                };
                rpc_body(
                    "create_session_failed",
                    self.coordinator
                        .create_session(input)
                        .await
                        .map(|sid| sid.0),
                )
            }
            RequestMethod::RestoreSession {
                session_id,
                auto_approve_level,
            } => {
                let sid = SessionId(session_id);
                rpc_body(
                    "restore_session_failed",
                    self.coordinator
                        .restore_session(&sid, auto_approve_level)
                        .await
                        .map(|sid| sid.0),
                )
            }
            RequestMethod::ForkSession {
                parent_id,
                auto_approve_level,
            } => {
                let parent = SessionId(parent_id);
                rpc_body(
                    "fork_session_failed",
                    self.coordinator
                        .fork_session(&parent, auto_approve_level)
                        .await
                        .map(|sid| sid.0),
                )
            }
            RequestMethod::SendMessage { session_id, blocks } => rpc_body(
                "send_message_failed",
                self.coordinator
                    .send_message(&SessionId(session_id), blocks)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            RequestMethod::Command { session_id, cmd } => {
                let sid = SessionId(session_id);
                rpc_body(
                    "command_failed",
                    dispatch_command(&self.coordinator, &sid, cmd)
                        .await
                        .map(|()| serde_json::Value::Null),
                )
            }
            RequestMethod::Subscribe {
                session_id,
                auto_approve_level,
            } => {
                let sid = SessionId(session_id.clone());
                let level = auto_approve_level;

                // Try to subscribe directly first
                let mut rx = self.coordinator.subscribe_session_events(&sid);
                if rx.is_none() {
                    // Session not in memory - try to restore from storage
                    match self.coordinator.restore_session(&sid, level).await {
                        Ok(_) => {
                            rx = self.coordinator.subscribe_session_events(&sid);
                        }
                        Err(e) => {
                            return ResponseBody::Err {
                                error: RpcError {
                                    code: "restore_failed".to_string(),
                                    message: e.to_string(),
                                    detail: None,
                                },
                            };
                        }
                    }
                }

                let rx = match rx {
                    Some(rx) => rx,
                    None => {
                        let err = SessionError::NotFound {
                            session_id: sid.0.clone(),
                        };
                        return ResponseBody::Err {
                            error: RpcError {
                                code: "session_error".to_string(),
                                message: KernelError::from(err.clone()).to_string(),
                                detail: Some(
                                    serde_json::to_value(&err).expect("SessionError serializes"),
                                ),
                            },
                        };
                    }
                };

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
            RequestMethod::Unsubscribe { session_id } => {
                if let Some(handle) = subscriptions.write().await.remove(&session_id) {
                    handle.abort();
                }
                ResponseBody::Ok {
                    result: serde_json::Value::Null,
                }
            }
            RequestMethod::GetSessionMessages { session_id } => rpc_body(
                "get_messages_failed",
                self.coordinator
                    .get_session_messages(&SessionId(session_id))
                    .await,
            ),
            RequestMethod::DeleteSession { session_id } => rpc_body(
                "delete_failed",
                self.coordinator
                    .delete_session(&SessionId(session_id))
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            RequestMethod::ListSessions {
                project_id,
                before,
                limit,
            } => {
                let pid = project_id.as_ref().map(|p| ProjectId(p.clone()));
                let result = self
                    .coordinator
                    .list_sessions(pid.as_ref(), before, limit)
                    .await;
                rpc_body("list_sessions_failed", result.map(|(s, _)| s))
            }
            RequestMethod::GetCheckpoints { session_id } => rpc_body(
                "get_checkpoints_failed",
                self.coordinator
                    .get_checkpoints(&SessionId(session_id))
                    .await,
            ),
            RequestMethod::GetTodos { session_id } => rpc_body(
                "get_todos_failed",
                self.coordinator.get_todos(&SessionId(session_id)).await,
            ),
            RequestMethod::ShutdownSession { session_id } => rpc_body(
                "shutdown_failed",
                self.coordinator
                    .shutdown_session(&SessionId(session_id))
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            RequestMethod::ReloadAgentConfig => {
                let ok = self.reload().await;
                if ok {
                    ResponseBody::Ok {
                        result: serde_json::Value::Null,
                    }
                } else {
                    ResponseBody::Err {
                        error: RpcError {
                            code: "reload_failed".to_string(),
                            message: "Failed to reload agent configuration".to_string(),
                            detail: None,
                        },
                    }
                }
            }
            RequestMethod::Hello => ResponseBody::Ok {
                result: serde_json::json!({
                    "proto": crate::wire::WIRE_PROTOCOL_VERSION,
                }),
            },
        }
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
        ControlCommand::AskUserResponse { req_id, answers } => {
            let response = crate::tools::AskUserResponse {
                answers: answers.into_iter().collect(),
            };
            coordinator
                .send_ask_user_response(sid, &req_id, response)
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

fn rpc_body<T: serde::Serialize>(
    default_code: &str,
    result: crate::types::Result<T>,
) -> ResponseBody {
    match result {
        Ok(val) => match serde_json::to_value(val) {
            Ok(v) => ResponseBody::Ok { result: v },
            Err(e) => ResponseBody::Err {
                error: RpcError {
                    code: "serialize_error".to_string(),
                    message: e.to_string(),
                    detail: None,
                },
            },
        },
        Err(e) => {
            let (code, detail) = match &e {
                crate::types::KernelError::Session(ref se) => (
                    "session_error",
                    Some(serde_json::to_value(se).expect("SessionError serializes")),
                ),
                _ => (default_code, None),
            };
            ResponseBody::Err {
                error: RpcError {
                    code: code.to_string(),
                    message: e.to_string(),
                    detail,
                },
            }
        }
    }
}
