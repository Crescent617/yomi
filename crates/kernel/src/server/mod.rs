use crate::agent::AgentConfig;
use crate::app::coordinator::CreateSessionInput;
use crate::app::Coordinator;
use crate::config::Config;
use crate::cron::CronJobId;
use crate::skill::{deduplicate_skills, SkillLoader};
use crate::transport::{recv_frame, send_frame};
use crate::types::{ProjectId, Result, SessionId};
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
    agent.allow_command_hooks = config.features.allow_command_hooks;
    agent
}

/// Kernel daemon server. Bridges external connections to the local Coordinator.
#[derive(Clone)]
pub struct KernelServer {
    coordinator: Arc<Coordinator>,
    connections: Arc<dashmap::DashMap<u64, tokio_util::sync::CancellationToken>>,
    next_conn_id: Arc<std::sync::atomic::AtomicU64>,
    /// Cron scheduler.  Held here because the `KernelServer` owns the lifecycle
    /// of the cron subsystem (start / reload / shutdown) independently of the
    /// `Coordinator`, which only provides the data layer (`CronStore`).
    cron_scheduler: Arc<std::sync::Mutex<Option<Arc<crate::cron::CronScheduler>>>>,
    shutdown: tokio_util::sync::CancellationToken,
}

impl KernelServer {
    pub fn new(coordinator: Arc<Coordinator>) -> Self {
        Self {
            coordinator,
            connections: Arc::new(dashmap::DashMap::new()),
            next_conn_id: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            cron_scheduler: Arc::new(std::sync::Mutex::new(None)),
            shutdown: tokio_util::sync::CancellationToken::new(),
        }
    }

    pub async fn start(&self, configs: Vec<crate::channels::ChannelConfig>) {
        self.coordinator.start(self.shutdown.clone());

        if let Some(store) = self.coordinator.cron_store.as_ref() {
            let (task_tx, task_rx) = mpsc::channel(64);
            let scheduler = Arc::new(crate::cron::CronScheduler::new(Arc::clone(store), task_tx));

            let sched_clone = Arc::clone(&scheduler);
            let cron_token = self.shutdown.child_token();
            tokio::spawn(async move { sched_clone.run(cron_token).await });

            let worker = crate::cron::CronWorker::new(
                Arc::clone(&self.coordinator) as Arc<dyn crate::cron::CronExecutor>,
                task_rx,
                Arc::clone(store),
                Some(Arc::clone(&scheduler)),
            );
            let worker_token = self.shutdown.child_token();
            tokio::spawn(async move { worker.run(worker_token).await });

            *self.cron_scheduler.lock().unwrap() = Some(scheduler);
        }

        if let Some(ref mgr) = self.coordinator.channel_manager {
            let weak = Arc::downgrade(&self.coordinator);
            if let Err(e) = mgr.start_all(self.shutdown.clone(), configs, weak).await {
                tracing::warn!(error = %e, "some channels failed to start");
            }
        }
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
                    tool_blocklist: Vec::new(),
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
                tool_blocklist,
            } => {
                let sid = SessionId(session_id);
                rpc_body(
                    "restore_session_failed",
                    self.coordinator
                        .restore_session(&sid, tool_blocklist)
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
            RequestMethod::ListSessionSkills { session_id } => rpc_body(
                "list_session_skills_failed",
                self.coordinator
                    .list_session_skills(&SessionId(session_id))
                    .await,
            ),
            RequestMethod::Command { session_id, cmd } => {
                let sid = SessionId(session_id);
                rpc_body(
                    "command_failed",
                    dispatch_command(&self.coordinator, &sid, cmd).await,
                )
            }
            RequestMethod::Subscribe { session_id } => {
                let sid = SessionId(session_id.clone());

                let rx = self.coordinator.subscribe_session_events(&sid);

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
                            opt = rx.recv() => match opt {
                                Some((_sid, ev)) => ev,
                                None => break,
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
            RequestMethod::GetSessionStatus { session_id } => rpc_body(
                "get_session_status_failed",
                self.coordinator
                    .get_session_status(&SessionId(session_id))
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
                rpc_body("list_sessions_failed", result)
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
            RequestMethod::RenameSession { session_id, title } => rpc_body(
                "rename_session_failed",
                self.coordinator
                    .rename_session(&SessionId(session_id), title)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            RequestMethod::PinSession {
                session_id,
                icon_emoji,
            } => rpc_body(
                "pin_session_failed",
                self.coordinator
                    .pin_session(&SessionId(session_id), icon_emoji)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            RequestMethod::UnpinSession { session_id } => rpc_body(
                "unpin_session_failed",
                self.coordinator
                    .unpin_session(&SessionId(session_id))
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            RequestMethod::SetPinnedSessionEmoji {
                session_id,
                icon_emoji,
            } => rpc_body(
                "set_pinned_session_emoji_failed",
                self.coordinator
                    .set_pinned_session_emoji(&SessionId(session_id), icon_emoji)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            RequestMethod::ListPinnedSessions => rpc_body(
                "list_pinned_sessions_failed",
                self.coordinator.list_pinned_sessions().await,
            ),
            RequestMethod::ShutdownSession { session_id } => rpc_body(
                "shutdown_failed",
                self.coordinator
                    .shutdown_session(&SessionId(session_id))
                    .await
                    .map(|()| serde_json::Value::Null),
            ),

            // ── Cron Job ──────────────────────────────────────────────────
            RequestMethod::CreateCronJob {
                name,
                schedule,
                action,
                max_runs,
                expires_at,
            } => {
                let input = crate::cron::CreateCronJobInput {
                    name,
                    schedule,
                    action,
                    max_runs,
                    expires_at,
                };
                match self.coordinator.create_cron_job(input).await {
                    Ok(job_id) => {
                        if let Some(ref scheduler) = *self.cron_scheduler.lock().unwrap() {
                            scheduler.reload();
                        }
                        ok_body(JobIdResponse { job_id: job_id.0 })
                    }
                    Err(e) => ResponseBody::Err {
                        error: RpcError {
                            code: "create_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            RequestMethod::ListCronJobs { status, limit } => {
                let status = status.and_then(|s| s.parse().ok());
                match self.coordinator.list_cron_jobs(status, limit).await {
                    Ok(jobs) => ResponseBody::Ok {
                        result: match serde_json::to_value(jobs) {
                            Ok(v) => v,
                            Err(e) => {
                                return ResponseBody::Err {
                                    error: RpcError {
                                        code: "serialize_error".to_string(),
                                        message: e.to_string(),
                                        detail: None,
                                    },
                                };
                            }
                        },
                    },
                    Err(e) => ResponseBody::Err {
                        error: RpcError {
                            code: "list_cron_jobs_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            RequestMethod::GetCronJob { job_id } => {
                match self.coordinator.get_cron_job(&CronJobId(job_id)).await {
                    Ok(Some(job)) => ResponseBody::Ok {
                        result: match serde_json::to_value(job) {
                            Ok(v) => v,
                            Err(e) => {
                                return ResponseBody::Err {
                                    error: RpcError {
                                        code: "serialize_error".to_string(),
                                        message: e.to_string(),
                                        detail: None,
                                    },
                                };
                            }
                        },
                    },
                    // Return null so the client can distinguish "not found" from a real error.
                    Ok(None) => ResponseBody::Ok {
                        result: serde_json::Value::Null,
                    },
                    Err(e) => ResponseBody::Err {
                        error: RpcError {
                            code: "get_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            RequestMethod::UpdateCronJob {
                job_id,
                name,
                schedule,
                action,
                status,
                max_runs,
                expires_at,
            } => {
                let status = status.and_then(|s| s.parse().ok());
                let input = crate::cron::UpdateCronJobInput {
                    name,
                    schedule,
                    action,
                    status,
                    max_runs,
                    expires_at,
                    ..Default::default()
                };
                match self
                    .coordinator
                    .update_cron_job(&CronJobId(job_id), input)
                    .await
                {
                    // Return true/false so the client can distinguish "updated" from "not found".
                    Ok(updated) => {
                        if updated {
                            if let Some(ref scheduler) = *self.cron_scheduler.lock().unwrap() {
                                scheduler.reload();
                            }
                        }
                        ResponseBody::Ok {
                            result: serde_json::Value::Bool(updated),
                        }
                    }
                    Err(e) => ResponseBody::Err {
                        error: RpcError {
                            code: "update_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            RequestMethod::DeleteCronJob { job_id } => {
                match self.coordinator.delete_cron_job(&CronJobId(job_id)).await {
                    // Return true/false so the client can distinguish "deleted" from "not found".
                    Ok(deleted) => {
                        if deleted {
                            if let Some(ref scheduler) = *self.cron_scheduler.lock().unwrap() {
                                scheduler.reload();
                            }
                        }
                        ResponseBody::Ok {
                            result: serde_json::Value::Bool(deleted),
                        }
                    }
                    Err(e) => ResponseBody::Err {
                        error: RpcError {
                            code: "delete_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }

            RequestMethod::TriggerCronJob { job_id } => {
                match self.coordinator.trigger_cron_job(&CronJobId(job_id)).await {
                    Ok(()) => ResponseBody::Ok {
                        result: serde_json::Value::Null,
                    },
                    Err(e) => ResponseBody::Err {
                        error: RpcError {
                            code: "trigger_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }

            // ── Usage ───────────────────────────────────────────────────────
            RequestMethod::GetUsageSummary { days } => {
                let days = days.unwrap_or(365);
                match self.coordinator.get_usage_summary(days).await {
                    Ok(summary) => ok_body(summary),
                    Err(e) => ResponseBody::Err {
                        error: RpcError {
                            code: "get_usage_summary_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            RequestMethod::GetDailyUsage { days } => {
                match self.coordinator.get_daily_usage(days).await {
                    Ok(daily) => ok_body(daily),
                    Err(e) => ResponseBody::Err {
                        error: RpcError {
                            code: "get_daily_usage_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }

            // ── Channel ────────────────────────────────────────────────────
            RequestMethod::ListChannels => {
                let channels = self.coordinator.list_channels();
                ok_body(channels)
            }

            RequestMethod::Hello => ok_body(ProtoResponse {
                proto: crate::wire::WIRE_PROTOCOL_VERSION,
            }),
        }
    }
}

async fn dispatch_command(
    coordinator: &Coordinator,
    sid: &SessionId,
    cmd: crate::event::ControlCommand,
) -> Result<serde_json::Value> {
    use crate::event::ControlCommand;
    match cmd {
        ControlCommand::Cancel => {
            coordinator.cancel(sid).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::Response {
            req_id,
            approved,
            remember,
        } => {
            coordinator
                .send_permission_response(sid, &req_id, approved, remember)
                .await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::AskUserResponse { req_id, answers } => {
            let response = crate::tools::AskUserResponse {
                answers: answers.into_iter().collect(),
            };
            coordinator
                .send_ask_user_response(sid, &req_id, response)
                .await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::SetLevel(level) => {
            coordinator.set_permission_level(sid, level).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::Compact => {
            coordinator.compact_session(sid).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::StartGoal(state) => {
            coordinator.start_goal(sid, state).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::StopGoal => {
            coordinator.stop_goal(sid).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::PauseGoal => {
            coordinator.pause_goal(sid).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::ResumeGoal => {
            coordinator.resume_goal(sid).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::EditGoal { description } => {
            coordinator.update_goal(sid, description).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::GetGoal => {
            let goal = coordinator.get_goal(sid).await?;
            Ok(serde_json::to_value(goal)?)
        }
        ControlCommand::Rewind { message_id, target } => {
            coordinator.rewind_session(sid, message_id, target).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::Steer { content } => {
            coordinator.send_steer(sid, content).await?;
            Ok(serde_json::Value::Null)
        }
        ControlCommand::Continue => {
            coordinator.send_continue(sid).await?;
            Ok(serde_json::Value::Null)
        }
    }
}

/// Serialize a value into `ResponseBody::Ok`, handling serialization errors.
fn ok_body<T: serde::Serialize>(val: T) -> ResponseBody {
    match serde_json::to_value(val) {
        Ok(v) => ResponseBody::Ok { result: v },
        Err(e) => ResponseBody::Err {
            error: RpcError {
                code: "serialize_error".to_string(),
                message: e.to_string(),
                detail: None,
            },
        },
    }
}

#[derive(serde::Serialize)]
struct JobIdResponse {
    job_id: String,
}

#[derive(serde::Serialize)]
struct ProtoResponse {
    proto: u32,
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
