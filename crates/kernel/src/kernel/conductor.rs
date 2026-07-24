use std::sync::{Arc, Mutex};

use dashmap::DashMap;
use tokio::task::JoinHandle;
use tracing::Instrument;

use crate::agent::AgentShared;
use crate::agent::{Agent, AgentConfig, AgentInput, AgentSpawnArgs, AgentState};
use crate::comms::{EventBus, InputBus, InputBusSubscriber, Mailbox};
use crate::event::{AgentEvent, AgentStatus, Event, InternalEvent};
use crate::notification::{AgentActivity, Notification, NotificationBus};
use crate::types::SessionId;

/// 唯一管理 Agent 生命周期的地方。
/// `InputBus` 的唯一消费者，负责 Mailbox 管理、Agent lazy spawn、Cancel 分发。
pub struct Conductor {
    agent_shared: Arc<AgentShared>,
    agent_config: AgentConfig,
    active: DashMap<SessionId, ActiveAgent>,
    mailboxes: DashMap<SessionId, Arc<Mailbox>>,
    rx: std::sync::Mutex<Option<InputBusSubscriber>>,
    event_bus: Arc<EventBus>,
    input_bus: Arc<InputBus>,
    base_prompt: String,
    data_dir: std::path::PathBuf,
    /// Per-session spawn lock to prevent duplicate agent creation races.
    spawn_locks: DashMap<SessionId, Arc<tokio::sync::Mutex<()>>>,
    notification_bus: Arc<NotificationBus>,
}

pub struct ActiveSessionSnapshot {
    pub session_id: SessionId,
    pub state: AgentState,
}

struct ActiveAgent {
    handle: JoinHandle<()>,
    cancel_token: crate::agent::CancelToken,
    state: Mutex<AgentState>,
    permission_state: Option<crate::permission::PermissionState>,
}

impl Conductor {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        agent_shared: Arc<AgentShared>,
        agent_config: AgentConfig,
        rx: InputBusSubscriber,
        event_bus: Arc<EventBus>,
        input_bus: Arc<InputBus>,
        base_prompt: String,
        data_dir: std::path::PathBuf,
        notification_bus: Arc<NotificationBus>,
    ) -> Self {
        Self {
            agent_shared,
            agent_config,
            active: DashMap::new(),
            mailboxes: DashMap::new(),
            rx: std::sync::Mutex::new(Some(rx)),
            event_bus,
            input_bus,
            base_prompt,
            data_dir,
            spawn_locks: DashMap::new(),
            notification_bus,
        }
    }

    pub async fn run(self: Arc<Self>, shutdown: tokio_util::sync::CancellationToken) {
        let mut rx = self
            .rx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
            .expect("Conductor::run already called");
        let mut subscriber = self.event_bus.subscribe_all();
        let mut cleanup_interval = tokio::time::interval(std::time::Duration::from_mins(10));

        loop {
            tokio::select! {
                biased;
                () = shutdown.cancelled() => {
                    tracing::info!("Conductor shutting down, cancelling all active agents");
                    for agent in &self.active {
                        agent.cancel_token.cancel();
                    }
                    break;
                }
                Some((sid, input)) = rx.recv() => {
                    let this = Arc::clone(&self);
                    tokio::spawn(async move {
                        this.handle_input(sid, input).await;
                    });
                }
                Some((sid, envelope)) = subscriber.recv() => {
                    let event_id = envelope.event_id.to_string();
                    if let Event::Agent(event) = &envelope.event {
                        if let Some(activity) = pet_activity(event) {
                            let _ = self.notification_bus.send(Notification::AgentActivity {
                                session_id: sid.clone(),
                                event_id,
                                activity,
                            });
                        }
                    }
                    match envelope.event {
                        Event::Agent(AgentEvent::StateChanged { state }) => {
                            if let Some(agent) = self.active.get(&sid) {
                                *agent.state.lock().unwrap_or_else(|e| e.into_inner()) = state;
                            }
                            let _ = self.notification_bus.send(Notification::StateChanged {
                                session_id: sid.clone(),
                                status: state,
                            });
                        }
                        Event::Internal(InternalEvent::MessageAdded { message }) => {
                            if let Some(ref store) = self.agent_shared.message_store {
                                if message.role != crate::types::Role::System {
                                    if let Err(e) = store.append(&sid.0, &[(*message).clone()]).await {
                                        tracing::warn!("Failed to persist message for session={sid}: {e}");
                                    }
                                }
                            }
                        }
                        Event::Internal(InternalEvent::MessageReplaced { messages }) => {
                            if let Some(ref store) = self.agent_shared.message_store {
                                let to_persist: Vec<crate::types::Message> = messages
                                    .iter()
                                    .map(|m| (**m).clone())
                                    .filter(|m| m.role != crate::types::Role::System)
                                    .collect();
                                if let Err(e) = store.replace(&sid.0, &to_persist).await {
                                    tracing::warn!("Failed to replace messages for session={sid}: {e}");
                                }
                            }
                            let _ = self.event_bus.publish(
                                sid.clone(),
                                crate::event::Envelope::new(
                                    sid.clone(),
                                    crate::event::Event::Agent(crate::event::AgentEvent::MessageReplaced { session_id: sid.clone() }),
                                ),
                            );
                        }
                        Event::Tool(crate::event::ToolEvent::Metadata { message_id, tool_id, metadata }) => {
                            if let Some(ref store) = self.agent_shared.message_store {
                                let placeholder = Arc::new(crate::types::Message {
                                    id: message_id.clone(),
                                    role: crate::types::Role::Internal,
                                    content: vec![],
                                    tool_call_id: Some(tool_id),
                                    metadata: Some(metadata),
                                    ..Default::default()
                                });
                                // Persist tool metadata through the same MessageAdded path as
                                // other messages. Internal metadata is not a conversation
                                // boundary, so the server keeps its replay buffer intact.
                                let envelope = crate::event::Envelope::new(
                                    sid.clone(),
                                    crate::event::Event::Internal(
                                        crate::event::InternalEvent::MessageAdded {
                                            message: Arc::clone(&placeholder),
                                        },
                                    ),
                                );
                                if let Err(e) = self.event_bus.publish(sid.clone(), envelope) {
                                    tracing::warn!("Failed to publish metadata MessageAdded for session={sid}: {e}, falling back to direct persist");
                                    if let Err(e2) = store.append(&sid.0, &[(*placeholder).clone()]).await {
                                        tracing::warn!("Failed to persist tool metadata for session={sid}: {e2}");
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ = cleanup_interval.tick() => {
                    self.active.retain(|_sid, agent| !agent.handle.is_finished());
                    // 清理没有活跃 agent 且 mailbox 为空的 session，防止内存泄漏
                    self.mailboxes.retain(|sid, mb| {
                        self.active.contains_key(sid) || !mb.is_empty()
                    });
                    // 清理已完成 spawn 或不会再被唤醒的 session 的锁。
                    // 保留既没有活跃 agent 但 mailbox 仍在的 session（未来还会 spawn）。
                    self.spawn_locks.retain(|sid, _| {
                        !self.active.contains_key(sid) && self.mailboxes.contains_key(sid)
                    });
                }
                else => break,
            }
        }
    }

    pub fn get_state(&self, sid: &SessionId) -> Option<AgentState> {
        self.active
            .get(sid)
            .map(|a| *a.state.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// Whether the session's agent task is live and in an active (non-idle)
    /// run. Used by the channel observability watchdog: a long tool call
    /// keeps the agent in `ExecutingTool` (alive), so liveness — unlike an
    /// event-gap timeout — never false-positives on slow tools.
    pub fn is_running(&self, sid: &SessionId) -> bool {
        self.active.get(sid).is_some_and(|agent| {
            !agent.handle.is_finished()
                && *agent.state.lock().unwrap_or_else(|e| e.into_inner()) != AgentState::Idle
        })
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Snapshot sessions whose agent task is still live and not idle.
    pub fn running_sessions(&self) -> Vec<ActiveSessionSnapshot> {
        self.active
            .iter()
            .filter_map(|agent| {
                if agent.handle.is_finished() {
                    return None;
                }
                let state = *agent.state.lock().unwrap_or_else(|e| e.into_inner());
                (state != AgentState::Idle).then(|| ActiveSessionSnapshot {
                    session_id: agent.key().clone(),
                    state,
                })
            })
            .collect()
    }

    /// Update the permission level for a live session (real-time).
    /// Returns `true` if the session is currently active and the level was updated.
    pub fn set_permission_level(&self, sid: &SessionId, level: crate::permission::Level) -> bool {
        if let Some(agent) = self.active.get(sid) {
            if let Some(ref ps) = agent.permission_state {
                tokio::spawn({
                    let ps = ps.clone();
                    let sid = sid.clone();
                    async move {
                        ps.set_auto_approve_level(level).await;
                        tracing::info!("Permission level updated in-memory for {}", sid.0);
                    }
                });
                return true;
            }
        }
        false
    }

    async fn handle_input(&self, sid: SessionId, input: AgentInput) {
        match input {
            AgentInput::Cancel => {
                if let Some(agent) = self.active.get(&sid) {
                    agent.cancel_token.cancel();
                }
                if let Some(mb) = self.mailboxes.get(&sid) {
                    mb.clear().await;
                }
            }
            // PermissionResponse and AskUserResponse are consumed directly by
            // Checker / AskUserTool via input_bus subscription; do not queue them.
            AgentInput::PermissionResponse { .. } | AgentInput::AskUserResponse { .. } => {}
            input => {
                let mb = self
                    .mailboxes
                    .entry(sid.clone())
                    .or_insert_with(|| Arc::new(Mailbox::new()))
                    .clone();

                Self::push_to_mailbox(&mb, input).await;
                self.wake_agent(&sid, mb).await;
            }
        }
    }

    async fn push_to_mailbox(mailbox: &Mailbox, input: AgentInput) {
        match input {
            AgentInput::Steer(content) => mailbox.push_steer(content).await,
            other => mailbox.push(other).await,
        }
    }

    /// Check whether the session or any ancestor is routed from an external
    /// channel (i.e. has no interactive UI attached).
    ///
    /// Subagent sessions don't carry their own channel mapping, so the parent
    /// chain is walked to find a channel-routed ancestor.
    async fn is_channel_routed(
        &self,
        sid: &SessionId,
        session_info: Option<&crate::storage::SessionInfo>,
    ) -> bool {
        let Some(hub) = &self.agent_shared.channel_hub else {
            return false;
        };
        if hub.is_channel_session(sid).await {
            return true;
        }
        let Some(store) = &self.agent_shared.session_store else {
            return false;
        };
        let mut cursor = session_info.and_then(|i| i.parent_id.clone());
        while let Some(pid) = cursor {
            if hub.is_channel_session(&pid).await {
                return true;
            }
            cursor = match store.get(&pid).await {
                Ok(Some(info)) => info.parent_id,
                _ => None,
            };
        }
        false
    }

    async fn wake_agent(&self, sid: &SessionId, mailbox: Arc<Mailbox>) {
        let lock = self
            .spawn_locks
            .entry(sid.clone())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .downgrade()
            .clone();
        let _guard = match lock.try_lock() {
            Ok(g) => g,
            Err(_) => {
                // Another task is already spawning for this session.
                // Messages have already been pushed to the mailbox; once that
                // spawn completes and the agent starts, it will consume them.
                return;
            }
        };

        if self
            .active
            .get(sid)
            .is_some_and(|v| !v.handle.is_finished())
        {
            return;
        }

        let history = match &self.agent_shared.message_store {
            Some(store) => match store.get(&sid.0).await {
                Ok(msgs) => msgs.into_iter().map(Arc::new).collect(),
                Err(_) => Vec::new(),
            },
            None => Vec::new(),
        };

        let session_info = match self.agent_shared.session_store.as_ref() {
            Some(s) => s.get(sid).await.ok().flatten(),
            None => None,
        };

        let cancel_token = session_info
            .as_ref()
            .and_then(|i| {
                i.parent_id
                    .clone()
                    .and_then(|p| self.active.get(&p))
                    .map(|a| a.cancel_token.child_token())
            })
            .unwrap_or_else(crate::agent::CancelToken::new);

        // Resolve working directory from session info or fallback to data_dir/workspace
        let working_dir = session_info
            .as_ref()
            .and_then(|i| i.working_dir.clone())
            .map(std::path::PathBuf::from);

        let cwd = working_dir.or_else(|| Some(self.data_dir.join("workspace")));

        // Create file state store
        let file_state_store = match Self::create_file_state_store(&sid.0, &self.data_dir).await {
            Ok(store) => store,
            Err(e) => {
                tracing::error!("Failed to create file state store: {}", e);
                Arc::new(crate::tools::helper::FileStateStore::new())
            }
        };

        // Create permission state from session's auto_approve_level
        let auto_approve_level = session_info
            .as_ref()
            .and_then(|i| i.auto_approve_level.as_ref())
            .and_then(|s| s.parse::<crate::permission::Level>().ok())
            .unwrap_or_default();
        let permission_state = Some(crate::permission::PermissionState::new(auto_approve_level));

        // Resolve workspace skill directory
        let workspace_skill_dir = cwd
            .as_ref()
            .map(|d| d.join(".agents/skills"))
            .filter(|d| d.exists());

        // Merge global skills with workspace skills
        let mut skills = self.agent_config.skills.clone();
        if let Some(dir) = workspace_skill_dir.as_ref() {
            match crate::skill::SkillLoader::new(vec![dir.clone()]).load_all() {
                Ok(mut ws_skills) => {
                    let mut merged = std::collections::HashMap::new();
                    for skill in &skills {
                        merged.insert(skill.name.clone(), skill.clone());
                    }
                    for skill in ws_skills.drain(..) {
                        merged.insert(skill.name.clone(), skill);
                    }
                    skills = merged.into_values().collect();
                    tracing::info!(
                        "loaded {} skill(s) from workspace {}",
                        skills.len(),
                        dir.display()
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to load workspace skills from {}: {}",
                        dir.display(),
                        e
                    );
                }
            }
        }

        // Clone AgentShared so we can mutate skill_folders per-session
        let mut base_clone: AgentShared = (*self.agent_shared).clone();
        if let Some(dir) = workspace_skill_dir.as_ref() {
            if !base_clone.skill_folders.contains(dir) {
                base_clone.skill_folders.push(dir.clone());
            }
        }
        let checkpoint_store = base_clone.checkpoint_store.clone();
        let shared = Arc::new(base_clone.with_per_session(
            permission_state,
            Some(Arc::clone(&file_state_store)),
            checkpoint_store,
        ));

        let working_dir = cwd.unwrap_or_default();

        // Channel sessions (and their subagent descendants) have no
        // interactive UI to answer ask_user; block the tool so the model
        // doesn't hang waiting for an answer that can never arrive.
        // Temporary heuristic until per-session tool_blocklist is persisted
        // in session meta at creation time.
        let mut tool_blocklist = self.agent_config.tool_blocklist.clone();
        if !tool_blocklist
            .iter()
            .any(|p| p == crate::tools::ask_user::ASK_USER_TOOL_NAME)
            && self.is_channel_routed(sid, session_info.as_ref()).await
        {
            tool_blocklist.push(crate::tools::ask_user::ASK_USER_TOOL_NAME.to_string());
        }

        let args = AgentSpawnArgs::new(
            self.base_prompt.clone(),
            sid.0.clone(),
            mailbox,
            working_dir,
        )
        .with_skills(skills)
        .with_arc_history(history)
        .with_max_iterations(self.agent_config.max_iterations)
        .with_subagent(self.agent_config.enable_subagent)
        .with_cron_tool(self.agent_config.enable_cron_tool)
        .with_file_state_store(Arc::clone(&file_state_store))
        .with_tool_blocklist(tool_blocklist)
        .with_max_tool_output_length(self.agent_config.max_tool_output_length)
        .with_cancel_token(cancel_token.clone())
        .with_input_bus(self.input_bus.clone());

        let agent = Agent::new(&shared, args).await;

        let session_id = sid.0.clone();
        let loop_span = tracing::info_span!("agent_loop", session_id = %session_id);
        let (start_tx, start_rx) = tokio::sync::oneshot::channel();
        let handle = tokio::spawn(
            async move {
                if start_rx.await.is_err() {
                    return;
                }
                tracing::info!("agent loop started");
                let _ = agent.start_loop().await;
                tracing::info!("agent loop ended");
            }
            .instrument(loop_span),
        );

        self.active.insert(
            sid.to_owned(),
            ActiveAgent {
                handle,
                cancel_token,
                state: Mutex::new(AgentState::Idle),
                permission_state: shared.permission_state.clone(),
            },
        );
        let _ = start_tx.send(());
    }

    /// Create and populate the file state store for this session
    async fn create_file_state_store(
        session_id: &str,
        data_dir: &std::path::Path,
    ) -> crate::types::Result<Arc<crate::tools::helper::FileStateStore>> {
        let jsonl_store =
            crate::storage::file_state::JsonlFileStateStore::new(session_id, data_dir);
        jsonl_store.maybe_vacuum().await?;
        let persistent_store: Arc<dyn crate::storage::FileStateStore> = Arc::new(jsonl_store);

        let states = persistent_store.read_all().await?;

        let file_state_store = crate::tools::helper::FileStateStore::new()
            .with_persistent(persistent_store)
            .with_states(states.into_iter().map(|fs| (fs.path, fs.mtime)));

        Ok(Arc::new(file_state_store))
    }
}

fn pet_activity(event: &AgentEvent) -> Option<AgentActivity> {
    match event {
        AgentEvent::PermissionRequest {
            req_id, session_id, ..
        } => Some(AgentActivity::PermissionRequested {
            req_id: req_id.clone(),
            target_session_id: session_id.clone(),
        }),
        AgentEvent::AskUserQuestion {
            req_id, session_id, ..
        } => Some(AgentActivity::AskUserRequested {
            req_id: req_id.clone(),
            target_session_id: session_id.clone(),
        }),
        AgentEvent::PermissionAck { req_id } | AgentEvent::AskUserAck { req_id } => {
            Some(AgentActivity::RequestResolved {
                req_id: req_id.clone(),
            })
        }
        AgentEvent::Lifecycle {
            state: AgentStatus::Running,
        } => Some(AgentActivity::Started),
        AgentEvent::Lifecycle {
            state: AgentStatus::Stopped { reason },
        } => Some(AgentActivity::Stopped {
            reason: reason.clone(),
        }),
        _ => None,
    }
}

#[cfg(test)]
#[path = "conductor_test.rs"]
mod tests;
