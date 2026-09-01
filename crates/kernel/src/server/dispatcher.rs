use crate::cron::CronJobId;
use crate::kernel::CreateSessionInput;
use crate::kernel::Kernel;
use crate::server::KernelServer;
use crate::types::{EventId, ProjectId, Result, SessionId};
use crate::wire::{ReqMethod, RespBody, RpcError, WireMsg};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, RwLock};

/// Key of the all-sessions subscription in a connection's subscription map
/// (session IDs are ULIDs, so "*" can never collide with a real one).
const ALL_SUBSCRIPTION_KEY: &str = "*";

impl KernelServer {
    pub(crate) async fn dispatch_request(
        &self,
        subscriptions: Arc<RwLock<HashMap<String, tokio::task::JoinHandle<()>>>>,
        send_tx: mpsc::Sender<WireMsg>,
        cancel: tokio_util::sync::CancellationToken,
        conn_id: &str,
        method: ReqMethod,
    ) -> RespBody {
        match method {
            // ── Extension（wire 外部扩展）──────────────────────────
            ReqMethod::ExtRegister {
                kind,
                name,
                desc,
                schema,
                level,
            } => {
                if kind != "tool" {
                    return rpc_error(
                        "ext_bad_kind",
                        format!("unsupported extension kind '{kind}' (phase 1: only 'tool')"),
                    );
                }
                let registry = self.kernel.extension_registry();
                rpc_body(
                    "ext_register_failed",
                    registry
                        .register_tool(
                            conn_id,
                            crate::extension::ExtToolDef {
                                name,
                                desc,
                                schema,
                                // 缺省 caution（走审批）：ext 是任意外部代码，
                                // 不给"默认免审"的口子。
                                level: level.unwrap_or(crate::permission::Level::Caution),
                            },
                        )
                        .map(|registration| serde_json::json!({ "registration": registration }))
                        .map_err(crate::types::KernelError::Config),
                )
            }
            ReqMethod::ExtPull { registration } => {
                let registry = self.kernel.extension_registry();
                match registry.pull(conn_id, &registration).await {
                    Ok(item) => RespBody::Ok {
                        result: match item {
                            Some(w) => serde_json::json!({
                                "call_id": w.call_id, "name": w.name, "args": w.args,
                            }),
                            None => serde_json::Value::Null,
                        },
                    },
                    Err(e) => rpc_error("ext_pull_failed", e),
                }
            }
            ReqMethod::ExtResult {
                call_id,
                output,
                is_error,
            } => {
                let registry = self.kernel.extension_registry();
                rpc_body(
                    "ext_result_failed",
                    registry
                        .submit_result(conn_id, &call_id, output, is_error)
                        .map(|()| serde_json::Value::Null)
                        .map_err(crate::types::KernelError::Config),
                )
            }
            ReqMethod::ExtRoute { source, key } => {
                rpc_body("ext_route_failed", self.kernel.ext_route(&source, &key).await.map(
                    |(sid, created)| {
                        serde_json::json!({ "session_id": sid.0.to_string(), "created": created })
                    },
                ))
            }

            // ── Config ───────────────────────────────────────────────────
            ReqMethod::GetConfig => {
                let path = self
                    .config_path
                    .clone()
                    .unwrap_or_else(crate::config::Config::write_path);
                rpc_body(
                    "get_config_failed",
                    crate::config::Config::get_kernel_config_from(&path),
                )
            }
            ReqMethod::SetConfig { content } => {
                let path = self
                    .config_path
                    .clone()
                    .unwrap_or_else(crate::config::Config::write_path);
                rpc_body(
                    "set_config_failed",
                    crate::config::Config::set_kernel_config_at(&path, &content)
                        .map(|()| serde_json::Value::Null),
                )
            }
            ReqMethod::Restart => {
                let Some(restart_tx) = &self.restart_tx else {
                    return rpc_error(
                        "restart_unavailable",
                        "daemon restart is not supported by this server",
                    );
                };
                let restart_tx = restart_tx.clone();
                match restart_tx.try_reserve_owned() {
                    Ok(permit) => {
                        tokio::spawn(async move {
                            // Keep the server alive long enough for the queued RPC response
                            // to reach the caller, while reserving lifecycle capacity now.
                            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                            permit.send(());
                        });
                        RespBody::Ok {
                            result: serde_json::Value::Null,
                        }
                    }
                    Err(error) => rpc_error(
                        "restart_unavailable",
                        format!("failed to request daemon restart: {error}"),
                    ),
                }
            }
            ReqMethod::ReadFile {
                source,
                offset,
                limit,
            } => {
                let data_dir = self.kernel.data_dir().await;
                rpc_body(
                    "read_file_failed",
                    crate::utils::file_read::read_file(&source, &data_dir, offset, limit).await,
                )
            }

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
            ReqMethod::SubscribeAll => {
                let mut subs = subscriptions.write().await;
                if let Some(old) = subs.remove(ALL_SUBSCRIPTION_KEY) {
                    old.abort();
                }
                let handle = self.spawn_all_subscription(send_tx.clone(), cancel.clone());
                subs.insert(ALL_SUBSCRIPTION_KEY.to_string(), handle);
                RespBody::Ok {
                    result: serde_json::Value::Null,
                }
            }
            ReqMethod::UnsubscribeAll => {
                if let Some(handle) = subscriptions.write().await.remove(ALL_SUBSCRIPTION_KEY) {
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
            ReqMethod::ReadSessionJsonl {
                session_id,
                before_offset,
                after_offset,
            } => rpc_body(
                "read_session_jsonl_failed",
                self.kernel
                    .read_session_jsonl(&SessionId::from(session_id), before_offset, after_offset)
                    .await,
            ),
            ReqMethod::GetSession { session_id } => rpc_body(
                "get_session_failed",
                self.kernel.get_session(&SessionId::from(session_id)).await,
            ),
            ReqMethod::GetRules { session_id } => rpc_body(
                "get_rules_failed",
                self.kernel
                    .get_session_rules(&SessionId::from(session_id))
                    .await,
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
            ReqMethod::MailboxSnapshot { session_id } => {
                let sid = SessionId::from(session_id);
                ok_body(self.kernel.mailbox_snapshot(&sid).await)
            }
            ReqMethod::RemoveMailboxItem {
                session_id,
                item_id,
            } => {
                let sid = SessionId::from(session_id);
                let removed = self.kernel.remove_mailbox_item(&sid, &item_id).await;
                ok_body(serde_json::json!({ "removed": removed }))
            }
            ReqMethod::SteerMailboxItem {
                session_id,
                item_id,
            } => {
                let sid = SessionId::from(session_id);
                let moved = self.kernel.steer_mailbox_item(&sid, &item_id).await;
                ok_body(serde_json::json!({ "moved": moved }))
            }
            ReqMethod::ClearMailbox { session_id, scope } => {
                let sid = SessionId::from(session_id);
                let removed = self.kernel.clear_mailbox(&sid, scope).await;
                ok_body(serde_json::json!({ "removed": removed }))
            }
            ReqMethod::ListSessions {
                project_id,
                scope,
                before,
                limit,
            } => {
                let pid = project_id.as_ref().map(|p| ProjectId::from(p.clone()));
                let result = self
                    .kernel
                    .list_sessions(pid.as_ref(), scope, before, limit)
                    .await;
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
            ReqMethod::AddFavorite { input } => {
                rpc_body("add_favorite_failed", self.kernel.add_favorite(input).await)
            }
            ReqMethod::RemoveFavorite { favorite_id } => rpc_body(
                "remove_favorite_failed",
                self.kernel
                    .remove_favorite(&favorite_id)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            ReqMethod::RemoveFavoriteByMessage {
                session_id,
                message_id,
            } => rpc_body(
                "remove_favorite_by_message_failed",
                self.kernel
                    .remove_favorite_by_message(
                        &SessionId::from(session_id),
                        &crate::types::MessageId::from(message_id),
                    )
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            ReqMethod::ListFavorites {
                query,
                limit,
                offset,
            } => rpc_body(
                "list_favorites_failed",
                self.kernel.list_favorites(query, limit, offset).await,
            ),
            ReqMethod::UpdateFavoriteNote { favorite_id, note } => rpc_body(
                "update_favorite_note_failed",
                self.kernel
                    .update_favorite_note(&favorite_id, note)
                    .await
                    .map(|()| serde_json::Value::Null),
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
                precheck,
            } => {
                let input = crate::cron::CreateCronJobInput {
                    name,
                    schedule,
                    action,
                    max_runs,
                    expires_at,
                    precheck,
                };
                match self.kernel.create_cron_job(input).await {
                    Ok(job_id) => ok_body(JobIdResponse {
                        job_id: job_id.0.to_string(),
                    }),
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
                precheck,
            } => {
                let status = status.and_then(|s| s.parse().ok());
                let input = crate::cron::UpdateCronJobInput {
                    name,
                    schedule,
                    action,
                    status,
                    max_runs,
                    expires_at,
                    precheck,
                    ..Default::default()
                };
                match self
                    .kernel
                    .update_cron_job(&CronJobId::from(job_id), input)
                    .await
                {
                    Ok(updated) => RespBody::Ok {
                        result: serde_json::Value::Bool(updated),
                    },
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
                    Ok(deleted) => RespBody::Ok {
                        result: serde_json::Value::Bool(deleted),
                    },
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
            ReqMethod::GetUsageRecords { before_id, limit } => {
                match self
                    .kernel
                    .get_usage_records(before_id.as_deref(), limit)
                    .await
                {
                    Ok(records) => ok_body(records),
                    Err(e) => RespBody::Err {
                        error: RpcError {
                            code: "get_usage_records_failed".to_string(),
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
            ReqMethod::ChannelNewThread {
                channel,
                platform,
                chat_id,
                title,
                text,
            } => rpc_body(
                "channel_new_thread_failed",
                match self.kernel.channel_manager() {
                    Some(hub) => {
                        hub.create_thread_in_chat(
                            &self.kernel,
                            channel.as_deref(),
                            platform
                                .as_deref()
                                .unwrap_or(crate::channels::DEFAULT_PLATFORM),
                            &chat_id,
                            title.as_deref(),
                            &text,
                        )
                        .await
                    }
                    None => Err(crate::types::KernelError::Config(
                        "no channels are running".to_string(),
                    )),
                },
            ),

            ReqMethod::SetChannelWatch {
                channel,
                platform,
                chat_id,
                on,
            } => rpc_body(
                "set_channel_watch_failed",
                match self.kernel.channel_manager() {
                    Some(hub) => {
                        hub.rpc_set_channel_watch(
                            &self.kernel,
                            channel.as_deref(),
                            platform
                                .as_deref()
                                .unwrap_or(crate::channels::DEFAULT_PLATFORM),
                            &chat_id,
                            on,
                        )
                        .await
                    }
                    None => Err(crate::types::KernelError::Config(
                        "no channels are running".to_string(),
                    )),
                },
            ),

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

            // ── Agent Template ─────────────────────────────────────────────
            ReqMethod::ListAgentTemplates { session_id } => rpc_body(
                "list_agent_templates_failed",
                self.kernel
                    .list_agent_templates(session_id.map(SessionId::from).as_ref())
                    .await,
            ),
            ReqMethod::SaveAgentTemplate {
                session_id,
                scope,
                name,
                body,
            } => rpc_body(
                "save_agent_template_failed",
                self.kernel
                    .save_agent_template(
                        session_id.map(SessionId::from).as_ref(),
                        scope,
                        &name,
                        &body,
                    )
                    .await
                    .map(|()| serde_json::Value::Null),
            ),
            ReqMethod::DeleteAgentTemplate {
                session_id,
                scope,
                name,
            } => rpc_body(
                "delete_agent_template_failed",
                self.kernel
                    .delete_agent_template(session_id.map(SessionId::from).as_ref(), scope, &name)
                    .await
                    .map(|()| serde_json::Value::Null),
            ),

            ReqMethod::Hello => ok_body(ProtoResponse {
                proto: crate::wire::WIRE_PROTOCOL_VERSION,
                instance_id: &self.instance_id,
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

    /// Forward the cross-session live stream to this connection (real-time
    /// only — no replay, see `ReqMethod::SubscribeAll`).
    fn spawn_all_subscription(
        &self,
        send_tx: mpsc::Sender<WireMsg>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> tokio::task::JoinHandle<()> {
        use tokio::sync::broadcast::error::RecvError;

        let mut rx = self.all_subscribers.subscribe();
        tokio::spawn(async move {
            loop {
                let envelope = tokio::select! {
                    biased;
                    () = cancel.cancelled() => break,
                    result = rx.recv() => match result {
                        Ok(e) => e,
                        // The receiver auto-resumes from the oldest retained
                        // event; keep the subscription alive and only log the
                        // gap instead of silently going dark.
                        Err(RecvError::Lagged(n)) => {
                            tracing::warn!(dropped = n, "all-events subscriber lagged");
                            continue;
                        }
                        Err(RecvError::Closed) => break,
                    },
                };
                match send_tx.try_send(WireMsg::Event(envelope)) {
                    Ok(()) => {}
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        tracing::warn!("outbound channel full, dropping all-events envelope");
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                }
            }
        })
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
            kernel.send_permission_response(sid, &req_id, approved, remember)?;
            Ok(serde_json::Value::Null)
        }
        Command::AskUserResponse { req_id, answers } => {
            let response = crate::tools::AskUserResponse {
                answers: answers.into_iter().collect(),
            };
            kernel.send_ask_user_response(sid, &req_id, response)?;
            Ok(serde_json::Value::Null)
        }
        Command::SetLevel(level) => {
            kernel.set_permission_level(sid, level).await?;
            Ok(serde_json::Value::Null)
        }
        Command::Compact => {
            kernel.compact_session(sid)?;
            Ok(serde_json::Value::Null)
        }
        Command::Rewind { message_id, target } => {
            kernel.rewind_session(sid, message_id, target).await?;
            Ok(serde_json::Value::Null)
        }
        Command::Steer { content } => {
            kernel.send_steer(sid, content).await;
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
struct ProtoResponse<'a> {
    proto: u32,
    instance_id: &'a str,
}

fn rpc_error(code: &str, message: impl Into<String>) -> RespBody {
    RespBody::Err {
        error: RpcError {
            code: code.to_string(),
            message: message.into(),
            detail: None,
        },
    }
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

#[cfg(test)]
#[path = "dispatcher_test.rs"]
mod tests;
