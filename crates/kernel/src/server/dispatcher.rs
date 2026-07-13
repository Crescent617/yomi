use crate::cron::CronJobId;
use crate::kernel::CreateSessionInput;
use crate::kernel::Kernel;
use crate::server::KernelServer;
use crate::types::{EventId, ProjectId, Result, SessionId};
use crate::wire::{ReqMethod, RespBody, RpcError, WireMsg};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

impl KernelServer {
    pub(crate) async fn dispatch_request(
        &self,
        subscriptions: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
        send_tx: mpsc::Sender<WireMsg>,
        cancel: tokio_util::sync::CancellationToken,
        method: ReqMethod,
    ) -> RespBody {
        match method {
            // ── Project ──────────────────────────────────────────────────
            ReqMethod::ListProjects => {
                rpc_body("list_projects_failed", self.kernel.list_projects().await)
            }
            ReqMethod::CreateProject { dir, name } => rpc_body(
                "create_project_failed",
                self.kernel.create_project(dir.into(), name).await,
            ),
            ReqMethod::GetProject { project_id } => rpc_body(
                "get_project_failed",
                self.kernel.get_project(&ProjectId::from(project_id)).await,
            ),
            ReqMethod::RenameProject { project_id, name } => rpc_body(
                "rename_project_failed",
                self.kernel
                    .rename_project(&ProjectId::from(project_id), name)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            ReqMethod::DeleteProject { project_id } => {
                let pid = ProjectId::from(project_id);
                let result = self.kernel.delete_project(&pid).await;
                if let Ok(report) = &result {
                    // Drop buffered events of deleted sessions
                    for sid in &report.sessions {
                        self.cleanup_session(sid);
                    }
                }
                rpc_body("delete_project_failed", result)
            }

            // ── Session ──────────────────────────────────────────────────
            ReqMethod::CreateSession {
                project_id,
                working_dir,
                auto_approve_level,
                model_key,
            } => {
                let input = CreateSessionInput {
                    project_id: project_id.map(ProjectId::from),
                    working_dir: working_dir.map(std::path::PathBuf::from),
                    auto_approve_level,
                    tool_blocklist: Vec::new(),
                    model_key,
                };
                rpc_body(
                    "create_session_failed",
                    self.kernel.create_session(input).await.map(|sid| sid.0),
                )
            }
            ReqMethod::RestoreSession { session_id } => {
                let sid = SessionId::from(session_id);
                rpc_body(
                    "restore_session_failed",
                    self.kernel.restore_session(&sid).await.map(|sid| sid.0),
                )
            }
            ReqMethod::ForkSession {
                parent_id,
                auto_approve_level,
            } => {
                let parent = SessionId::from(parent_id);
                rpc_body(
                    "fork_session_failed",
                    self.kernel
                        .fork_session(&parent, auto_approve_level)
                        .await
                        .map(|sid| sid.0),
                )
            }
            ReqMethod::SendMessage { session_id, blocks } => rpc_body(
                "send_message_failed",
                self.kernel
                    .send_message(&SessionId::from(session_id), blocks)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            ReqMethod::ListSessionSkills { session_id } => rpc_body(
                "list_session_skills_failed",
                self.kernel
                    .list_session_skills(&SessionId::from(session_id))
                    .await,
            ),
            ReqMethod::Command { session_id, cmd } => {
                let sid = SessionId::from(session_id);
                rpc_body(
                    "command_failed",
                    dispatch_command(&self.kernel, &sid, cmd).await,
                )
            }
            ReqMethod::Subscribe {
                session_id,
                after_event_id,
            } => {
                let mut subs = subscriptions.write().await;
                if let Some(old) = subs.remove(&session_id) {
                    old.abort();
                }
                let handle = self.spawn_subscription(
                    session_id.clone(),
                    after_event_id,
                    send_tx.clone(),
                    cancel.clone(),
                );
                subs.insert(session_id, handle);
                RespBody::Ok {
                    result: serde_json::Value::Null,
                }
            }
            ReqMethod::Unsubscribe { session_id } => {
                if let Some(handle) = subscriptions.write().await.remove(&session_id) {
                    handle.abort();
                }
                RespBody::Ok {
                    result: serde_json::Value::Null,
                }
            }
            ReqMethod::ListMessages { session_id } => rpc_body(
                "list_messages_failed",
                self.kernel
                    .list_messages(&SessionId::from(session_id))
                    .await,
            ),
            ReqMethod::GetSession { session_id } => rpc_body(
                "get_session_failed",
                self.kernel.get_session(&SessionId::from(session_id)).await,
            ),
            ReqMethod::DeleteSession { session_id } => {
                let sid = SessionId::from(session_id);
                let result = self
                    .kernel
                    .delete_session(&sid)
                    .await
                    .map(|()| serde_json::Value::Null);
                if result.is_ok() {
                    self.cleanup_session(&sid);
                }
                rpc_body("delete_failed", result)
            }
            ReqMethod::ClearSession { session_id } => {
                let sid = SessionId::from(session_id);
                rpc_body(
                    "clear_session_failed",
                    self.kernel
                        .clear_session(&sid)
                        .map(|()| serde_json::Value::Null),
                )
            }
            ReqMethod::ListSessions {
                project_id,
                before,
                limit,
            } => {
                let pid = project_id.as_ref().map(|p| ProjectId::from(p.clone()));
                let result = self.kernel.list_sessions(pid.as_ref(), before, limit).await;
                rpc_body("list_sessions_failed", result)
            }
            ReqMethod::ListRunningSessions => rpc_body(
                "list_running_sessions_failed",
                self.kernel.list_running_sessions().await,
            ),
            ReqMethod::ListSubagents { parent_session_id } => rpc_body(
                "list_subagents_failed",
                self.kernel
                    .list_subagents(&SessionId::from(parent_session_id))
                    .await,
            ),
            ReqMethod::GetCheckpoints { session_id } => rpc_body(
                "get_checkpoints_failed",
                self.kernel
                    .get_checkpoints(&SessionId::from(session_id))
                    .await,
            ),
            ReqMethod::GetTodos { session_id } => rpc_body(
                "get_todos_failed",
                self.kernel.get_todos(&SessionId::from(session_id)).await,
            ),
            ReqMethod::RenameSession { session_id, title } => rpc_body(
                "rename_session_failed",
                self.kernel
                    .rename_session(&SessionId::from(session_id), title)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            ReqMethod::PinSession {
                session_id,
                icon_emoji,
            } => rpc_body(
                "pin_session_failed",
                self.kernel
                    .pin_session(&SessionId::from(session_id), icon_emoji)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            ReqMethod::UnpinSession { session_id } => rpc_body(
                "unpin_session_failed",
                self.kernel
                    .unpin_session(&SessionId::from(session_id))
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            ReqMethod::SetPinnedSessionEmoji {
                session_id,
                icon_emoji,
            } => rpc_body(
                "set_pinned_session_emoji_failed",
                self.kernel
                    .set_pinned_session_emoji(&SessionId::from(session_id), icon_emoji)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            ReqMethod::ListPinnedSessions => rpc_body(
                "list_pinned_sessions_failed",
                self.kernel.list_pinned_sessions().await,
            ),
            ReqMethod::ShutdownSession { session_id: _ } => RespBody::Ok {
                result: serde_json::Value::Null,
            },

            // ── Cron Job ──────────────────────────────────────────────────
            ReqMethod::CreateCronJob {
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
                match self.kernel.create_cron_job(input).await {
                    Ok(job_id) => {
                        if let Some(ref scheduler) = *self.cron_scheduler.lock().unwrap() {
                            scheduler.reload();
                        }
                        ok_body(JobIdResponse {
                            job_id: job_id.0.to_string(),
                        })
                    }
                    Err(e) => RespBody::Err {
                        error: RpcError {
                            code: "create_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            ReqMethod::ListCronJobs { status, limit } => {
                let status = status.and_then(|s| s.parse().ok());
                match self.kernel.list_cron_jobs(status, limit).await {
                    Ok(jobs) => RespBody::Ok {
                        result: match serde_json::to_value(jobs) {
                            Ok(v) => v,
                            Err(e) => {
                                return RespBody::Err {
                                    error: RpcError {
                                        code: "serialize_error".to_string(),
                                        message: e.to_string(),
                                        detail: None,
                                    },
                                };
                            }
                        },
                    },
                    Err(e) => RespBody::Err {
                        error: RpcError {
                            code: "list_cron_jobs_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            ReqMethod::GetCronJob { job_id } => {
                match self.kernel.get_cron_job(&CronJobId::from(job_id)).await {
                    Ok(Some(job)) => RespBody::Ok {
                        result: match serde_json::to_value(job) {
                            Ok(v) => v,
                            Err(e) => {
                                return RespBody::Err {
                                    error: RpcError {
                                        code: "serialize_error".to_string(),
                                        message: e.to_string(),
                                        detail: None,
                                    },
                                };
                            }
                        },
                    },
                    Ok(None) => RespBody::Ok {
                        result: serde_json::Value::Null,
                    },
                    Err(e) => RespBody::Err {
                        error: RpcError {
                            code: "get_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            ReqMethod::UpdateCronJob {
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
                    .kernel
                    .update_cron_job(&CronJobId::from(job_id), input)
                    .await
                {
                    Ok(updated) => {
                        if updated {
                            if let Some(ref scheduler) = *self.cron_scheduler.lock().unwrap() {
                                scheduler.reload();
                            }
                        }
                        RespBody::Ok {
                            result: serde_json::Value::Bool(updated),
                        }
                    }
                    Err(e) => RespBody::Err {
                        error: RpcError {
                            code: "update_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            ReqMethod::DeleteCronJob { job_id } => {
                match self.kernel.delete_cron_job(&CronJobId::from(job_id)).await {
                    Ok(deleted) => {
                        if deleted {
                            if let Some(ref scheduler) = *self.cron_scheduler.lock().unwrap() {
                                scheduler.reload();
                            }
                        }
                        RespBody::Ok {
                            result: serde_json::Value::Bool(deleted),
                        }
                    }
                    Err(e) => RespBody::Err {
                        error: RpcError {
                            code: "delete_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }

            ReqMethod::TriggerCronJob { job_id } => {
                match self.kernel.trigger_cron_job(&CronJobId::from(job_id)).await {
                    Ok(()) => RespBody::Ok {
                        result: serde_json::Value::Null,
                    },
                    Err(e) => RespBody::Err {
                        error: RpcError {
                            code: "trigger_cron_job_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }

            // ── Usage ───────────────────────────────────────────────────────
            ReqMethod::GetUsageSummary { days } => {
                let days = days.unwrap_or(365);
                match self.kernel.get_usage_summary(days).await {
                    Ok(summary) => ok_body(summary),
                    Err(e) => RespBody::Err {
                        error: RpcError {
                            code: "get_usage_summary_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }
            ReqMethod::GetDailyUsage { days } => match self.kernel.get_daily_usage(days).await {
                Ok(daily) => ok_body(daily),
                Err(e) => RespBody::Err {
                    error: RpcError {
                        code: "get_daily_usage_failed".to_string(),
                        message: e.to_string(),
                        detail: None,
                    },
                },
            },
            ReqMethod::GetModelUsage { days } => match self.kernel.get_model_usage(days).await {
                Ok(usage) => ok_body(usage),
                Err(e) => RespBody::Err {
                    error: RpcError {
                        code: "get_model_usage_failed".to_string(),
                        message: e.to_string(),
                        detail: None,
                    },
                },
            },
            ReqMethod::GetModelUsageSince { start } => {
                match self.kernel.get_model_usage_since(start).await {
                    Ok(usage) => ok_body(usage),
                    Err(e) => RespBody::Err {
                        error: RpcError {
                            code: "get_model_usage_since_failed".to_string(),
                            message: e.to_string(),
                            detail: None,
                        },
                    },
                }
            }

            // ── Channel ────────────────────────────────────────────────────
            ReqMethod::ListChannels => {
                let channels = self.kernel.list_channels();
                ok_body(channels)
            }

            // ── Model ──────────────────────────────────────────────────────
            ReqMethod::ListModels => {
                rpc_body("list_models_failed", self.kernel.list_models().await)
            }
            ReqMethod::GetSessionModel { session_id } => {
                let sid = SessionId::from(session_id);
                let key = self.kernel.get_session_model(&sid).await;
                ok_body(key)
            }
            ReqMethod::SetSessionModel { session_id, key } => {
                let sid = SessionId::from(session_id);
                rpc_body(
                    "set_session_model_failed",
                    self.kernel
                        .set_session_model(&sid, &key)
                        .await
                        .map(|()| serde_json::Value::Null),
                )
            }

            ReqMethod::Hello => ok_body(ProtoResponse {
                proto: crate::wire::WIRE_PROTOCOL_VERSION,
            }),
        }
    }

    /// Spawn the per-connection event-forwarding task for one session.
    ///
    /// Replays buffered history first, then switches to real-time push.
    /// Events that arrive while the replay is running are deduplicated
    /// against the already-sent history.
    fn spawn_subscription(
        &self,
        session_id: String,
        after_event_id: Option<EventId>,
        send_tx: mpsc::Sender<WireMsg>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        use tokio::sync::broadcast::error::RecvError;

        let sid = SessionId::from(session_id.clone());
        // Register the real-time receiver *before* reading the buffer so
        // events arriving during replay are queued rather than lost.
        let mut rt_rx = self.session_subscribers.subscribe(&sid);
        let event_buffer = Arc::clone(&self.event_buffer);

        tokio::spawn(async move {
            // Forward one envelope; returns false when the connection is gone.
            let forward = |envelope| match send_tx.try_send(WireMsg::Event(envelope)) {
                Ok(()) => true,
                Err(mpsc::error::TrySendError::Full(_)) => {
                    tracing::warn!(%session_id, "outbound channel full, dropping event");
                    true
                }
                Err(mpsc::error::TrySendError::Closed(_)) => false,
            };

            // 1. Replay buffered history.
            let mut seen = std::collections::HashSet::<EventId>::new();
            for envelope in event_buffer.get_after(&sid, after_event_id.as_ref()) {
                seen.insert(envelope.event_id.clone());
                if !forward(envelope) {
                    return;
                }
            }

            // 2. Drain events that arrived during the replay, deduplicated
            //    against the already-sent history.
            while let Ok(envelope) = rt_rx.try_recv() {
                if seen.insert(envelope.event_id.clone()) && !forward(envelope) {
                    return;
                }
            }
            drop(seen);

            // 3. Real-time loop. No deduplication needed here because the
            //    global forwarder pushes each event exactly once.
            loop {
                let envelope = tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    result = rt_rx.recv() => match result {
                        Ok(e) => e,
                        // The receiver auto-resumes from the oldest retained
                        // event; keep the subscription alive and only log the
                        // gap instead of silently going dark.
                        Err(RecvError::Lagged(n)) => {
                            tracing::warn!(%session_id, dropped = n, "event subscriber lagged");
                            continue;
                        }
                        Err(RecvError::Closed) => break,
                    },
                };
                if !forward(envelope) {
                    break;
                }
            }
        })
    }

    /// Drop all server-side per-session state (replay buffer + fan-out channel).
    pub(crate) fn cleanup_session(&self, sid: &SessionId) {
        self.event_buffer.remove(sid);
        self.session_subscribers.remove_session(sid);
    }
}

async fn dispatch_command(
    kernel: &Kernel,
    sid: &SessionId,
    cmd: crate::event::Command,
) -> Result<serde_json::Value> {
    use crate::event::Command;
    match cmd {
        Command::Cancel => {
            kernel.cancel(sid);
            Ok(serde_json::Value::Null)
        }
        Command::Response {
            req_id,
            approved,
            remember,
        } => {
            kernel.send_permission_response(sid, &req_id, approved, remember);
            Ok(serde_json::Value::Null)
        }
        Command::AskUserResponse { req_id, answers } => {
            let response = crate::tools::AskUserResponse {
                answers: answers.into_iter().collect(),
            };
            kernel.send_ask_user_response(sid, &req_id, response);
            Ok(serde_json::Value::Null)
        }
        Command::SetLevel(level) => {
            kernel.set_permission_level(sid, level).await?;
            Ok(serde_json::Value::Null)
        }
        Command::Compact => {
            kernel.compact_session(sid);
            Ok(serde_json::Value::Null)
        }
        Command::StartGoal(state) => {
            kernel.start_goal(sid, state).await?;
            Ok(serde_json::Value::Null)
        }
        Command::StopGoal => {
            kernel.stop_goal(sid).await?;
            Ok(serde_json::Value::Null)
        }
        Command::PauseGoal => {
            kernel.pause_goal(sid).await?;
            Ok(serde_json::Value::Null)
        }
        Command::ResumeGoal => {
            kernel.resume_goal(sid).await?;
            Ok(serde_json::Value::Null)
        }
        Command::EditGoal { description } => {
            kernel.update_goal(sid, description).await?;
            Ok(serde_json::Value::Null)
        }
        Command::GetGoal => {
            let goal = kernel.get_goal(sid).await?;
            Ok(serde_json::to_value(goal)?)
        }
        Command::Rewind { message_id, target } => {
            kernel.rewind_session(sid, message_id, target).await?;
            Ok(serde_json::Value::Null)
        }
        Command::Steer { content } => {
            kernel.send_steer(sid, content);
            Ok(serde_json::Value::Null)
        }
        Command::Continue => {
            kernel.send_continue(sid);
            Ok(serde_json::Value::Null)
        }
    }
}

/// Serialize a value into `ResponseBody::Ok`, handling serialization errors.
fn ok_body<T: serde::Serialize>(val: T) -> RespBody {
    match serde_json::to_value(val) {
        Ok(v) => RespBody::Ok { result: v },
        Err(e) => RespBody::Err {
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

fn rpc_body<T: serde::Serialize>(default_code: &str, result: crate::types::Result<T>) -> RespBody {
    match result {
        Ok(val) => match serde_json::to_value(val) {
            Ok(v) => RespBody::Ok { result: v },
            Err(e) => RespBody::Err {
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
            RespBody::Err {
                error: RpcError {
                    code: code.to_string(),
                    message: e.to_string(),
                    detail,
                },
            }
        }
    }
}
