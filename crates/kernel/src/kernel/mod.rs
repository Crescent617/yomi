//! Application layer - kernel and conductor management

pub mod conductor;
pub use conductor::Conductor;

use crate::agent::{AgentConfig, AgentInput, AgentShared, AgentState};
use crate::comms::InputBus;
use crate::permission::Level;
use crate::provider::Provider;
use crate::storage::usage::{DailyUsage, UsageSummary};
use crate::storage::{MessageStore, ProjectStore, SessionStore, StorageSet, UsageStore};
use crate::tools::AskUserResponse;
use crate::types::{KernelError, Project, ProjectId, Result, SessionError, SessionId};
use chrono::{DateTime, Utc};
use std::sync::Arc;

/// Input for creating a new session
#[derive(Debug, Clone)]
pub struct CreateSessionInput {
    pub project_id: Option<ProjectId>,
    pub working_dir: Option<std::path::PathBuf>,
    pub auto_approve_level: Level,
    pub tool_blocklist: Vec<String>,
}

pub struct Kernel {
    agent_shared: Arc<AgentShared>,
    input_bus: Arc<InputBus>,
    conductor: Arc<Conductor>,
    /// Default agent configuration for new sessions.
    agent_config: AgentConfig,
    /// Project store for project operations
    project_store: Arc<dyn ProjectStore>,
    /// Pinned session store for sidebar pinning and emoji.
    pinned_session_store: Arc<dyn crate::storage::PinnedSessionStore>,
    /// Cron store for scheduled job operations.
    pub(crate) cron_store: Option<Arc<dyn crate::cron::CronStore>>,
    pub(crate) channel_manager: Option<Arc<crate::channels::hub::ChannelHub>>,
    /// Global notification bus for state changes and other broadcasts.
    notification_bus: Arc<crate::notification::NotificationBus>,
    /// Global shutdown token for graceful stop.
    shutdown: tokio_util::sync::CancellationToken,
}

impl Kernel {
    /// Get session store from `agent_shared`
    pub async fn session_store(&self) -> Arc<dyn SessionStore> {
        self.agent_shared
            .session_store
            .clone()
            .expect("session_store not configured")
    }

    /// List all channels and their status.
    pub fn list_channels(&self) -> Vec<crate::channels::ChannelInfo> {
        match &self.channel_manager {
            Some(mgr) => mgr.list_channels(),
            None => Vec::new(),
        }
    }

    /// Get the channel manager (if channels are configured).
    pub fn channel_manager(&self) -> Option<Arc<crate::channels::hub::ChannelHub>> {
        self.channel_manager.clone()
    }

    /// Get pinned session store
    pub fn pinned_session_store(&self) -> Arc<dyn crate::storage::PinnedSessionStore> {
        self.pinned_session_store.clone()
    }

    /// Get message store from `agent_shared`
    pub async fn message_store(&self) -> Arc<dyn MessageStore> {
        self.agent_shared
            .message_store
            .clone()
            .expect("message_store not configured")
    }

    /// Get checkpoint store from `agent_shared`
    pub async fn checkpoint_store(&self) -> Arc<dyn crate::checkpoint::CheckpointStore> {
        self.agent_shared
            .checkpoint_store
            .clone()
            .expect("checkpoint_store not configured")
    }

    /// Get cron store if configured.
    pub fn cron_store(&self) -> Option<Arc<dyn crate::cron::CronStore>> {
        self.cron_store.clone()
    }

    /// Get data directory from `agent_shared`
    pub async fn data_dir(&self) -> std::path::PathBuf {
        self.agent_shared.data_dir.clone()
    }

    /// Get usage store from `agent_shared`
    pub async fn usage_store(&self) -> Arc<dyn UsageStore> {
        self.agent_shared
            .usage_store
            .clone()
            .expect("usage_store not configured")
    }

    /// Get goal store from `agent_shared`
    pub async fn goal_store(&self) -> Arc<dyn crate::goal::GoalStore> {
        self.agent_shared
            .goal_store
            .clone()
            .expect("goal_store not configured")
    }

    /// Get todo store from `agent_shared`
    pub async fn todo_store(&self) -> Arc<dyn crate::storage::TodoStore> {
        self.agent_shared
            .todo_storage
            .clone()
            .expect("todo_storage not configured")
    }

    /// Get the global notification bus.
    pub fn notification_bus(&self) -> Arc<crate::notification::NotificationBus> {
        self.notification_bus.clone()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: &StorageSet,
        provider: Arc<dyn Provider>,
        agent_config: AgentConfig,
        task_store: Option<Arc<crate::tools::task::TaskStore>>,
        compactor: Option<crate::compactor::Compactor>,
        skill_folders: Vec<std::path::PathBuf>,
        hook_registry: Option<crate::hooks::HookRegistry>,
        enable_cron: bool,
        channel_store: Option<Arc<dyn crate::channels::ChannelStore>>,
    ) -> Arc<Self> {
        let session_store = storage.session_store();
        let message_store = storage.message_store();
        let todo_storage = storage.todo_store();
        let todo_interceptor = Arc::new(crate::agent::TodoReminderInterceptor::new(
            todo_storage.clone(),
        ));
        let checkpoint_store = storage.checkpoint_store();
        let data_dir = storage.data_dir().to_path_buf();
        let data_dir_for_conductor = data_dir.clone();
        let project_store = storage.project_store();
        let pinned_session_store = storage.pinned_session_store();
        let goal_store = storage.goal_store();
        let agent_shared = AgentShared::with_data_dir(
            provider,
            Arc::new(agent_config.model.clone()),
            task_store,
            Some(todo_storage),
            compactor,
            Some(session_store),
            Some(message_store),
            Some(storage.usage_store()),
            None,
            skill_folders,
            None,
            Some(checkpoint_store),
            data_dir,
        )
        .with_goal_store(goal_store)
        .with_message_interceptor(todo_interceptor);
        let agent_shared = match hook_registry {
            Some(registry) => agent_shared.with_hook_registry(Arc::new(registry)),
            None => agent_shared,
        };

        let channel_manager =
            channel_store.map(|store| Arc::new(crate::channels::hub::ChannelHub::new(store)));

        let agent_shared = agent_shared.with_channel_manager(channel_manager.clone());
        let event_bus = crate::comms::EventBus::new();
        let agent_shared = agent_shared.with_event_bus(event_bus.clone());
        let agent_shared = Arc::new(agent_shared);

        let input_bus = InputBus::new();
        let rx = input_bus.subscribe_all();
        let base_prompt = agent_config.system_prompt.clone();
        let notification_bus = Arc::new(crate::notification::NotificationBus::new());
        let conductor = Arc::new(Conductor::new(
            agent_shared.clone(),
            agent_config.clone(),
            rx,
            event_bus,
            input_bus.clone(),
            base_prompt,
            data_dir_for_conductor,
            notification_bus.clone(),
        ));
        let agent_config = agent_config;
        let cron_store = if enable_cron {
            Some(storage.cron_store())
        } else {
            None
        };

        Arc::new(Self {
            agent_shared,
            input_bus,
            conductor,
            agent_config,
            project_store,
            pinned_session_store,
            cron_store,
            channel_manager,
            notification_bus,
            shutdown: tokio_util::sync::CancellationToken::new(),
        })
    }

    pub fn start(&self) {
        let conductor = self.conductor.clone();
        let token = self.shutdown.clone();
        tokio::spawn(async move { conductor.run(token).await });
    }

    /// Gracefully stop the kernel and all background tasks.
    pub fn stop(&self) {
        self.shutdown.cancel();
        self.input_bus.shutdown();
        if let Some(ref bus) = self.agent_shared.event_bus {
            bus.shutdown();
        }
    }

    // ── Project API ──────────────────────────────────────────────────────

    /// Create a new project.
    /// If a project already exists for the given directory, returns the existing one.
    pub async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        let abs = tokio::fs::canonicalize(&dir).await.unwrap_or(dir);
        let dir_str = abs
            .to_str()
            .ok_or_else(|| SessionError::Other("Invalid project directory path".to_string()))?;

        // Check for existing project by directory
        if let Some(existing) = self.project_store.get_by_dir(dir_str).await? {
            return Ok(existing);
        }

        let name = name.unwrap_or_else(|| {
            abs.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unnamed")
                .to_string()
        });
        let id = ProjectId::new();
        self.project_store.create(&id, &name, dir_str).await?;
        Ok(Project {
            id,
            name,
            dir: abs,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        })
    }

    /// List all projects
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        self.project_store.list().await
    }

    /// Get project by ID
    pub async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        self.project_store.get(id).await
    }

    /// Rename a project
    pub async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()> {
        self.project_store.update_name(id, &name).await
    }

    /// Rename a session (update title in storage, normalized: collapse whitespace, truncated to 20 chars).
    pub async fn rename_session(&self, id: &SessionId, title: String) -> Result<()> {
        let title = normalize_session_title(&title);
        self.session_store().await.update_title(id, &title).await?;
        let noti = crate::notification::Notification::TitleUpdated {
            session_id: id.clone(),
            title: title.clone(),
        };
        let _ = self.notification_bus.send(noti);
        Ok(())
    }

    /// Pin a session to the top of the sidebar, optionally with an emoji.
    pub async fn pin_session(&self, id: &SessionId, emoji: Option<String>) -> Result<()> {
        self.pinned_session_store().pin(id, emoji.as_deref()).await
    }

    /// Unpin a session from the sidebar.
    pub async fn unpin_session(&self, id: &SessionId) -> Result<()> {
        self.pinned_session_store().unpin(id).await
    }

    /// Update the emoji for a pinned session.
    pub async fn set_pinned_session_emoji(
        &self,
        id: &SessionId,
        emoji: Option<String>,
    ) -> Result<()> {
        self.pinned_session_store()
            .update_emoji(id, emoji.as_deref())
            .await
    }

    /// List pinned sessions with their session metadata.
    pub async fn list_pinned_sessions(&self) -> Result<Vec<crate::storage::PinnedSessionDetail>> {
        self.pinned_session_store().list_with_details().await
    }

    /// Delete a project (only if it has no sessions)
    pub async fn delete_project(&self, id: &ProjectId) -> Result<()> {
        let (sessions, _) = self.session_store().await.list(Some(id), None, 1).await?;
        if !sessions.is_empty() {
            return Err(SessionError::Other(format!(
                "Project {} has sessions, remove or reassign them first",
                id.0
            ))
            .into());
        }
        self.project_store.delete(id).await
    }

    // ── Session API ──────────────────────────────────────────────────────

    /// Create a new session with the given input.
    #[tracing::instrument(skip(self, input))]
    pub async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        let _project = match &input.project_id {
            Some(pid) => Some(
                self.project_store
                    .get(pid)
                    .await?
                    .ok_or_else(|| SessionError::Other(format!("Project {} not found", pid.0)))?,
            ),
            None => None,
        };

        let working_dir = if let Some(p) = input.working_dir {
            let canonical = tokio::fs::canonicalize(&p).await;
            Some(canonical.unwrap_or(p).to_string_lossy().to_string())
        } else {
            None
        };

        let id = SessionId::new();
        self.session_store()
            .await
            .create(
                &id,
                input.project_id.as_ref(),
                working_dir.as_deref(),
                Some(input.auto_approve_level.as_str()),
                None,
            )
            .await?;

        if let Some(ref pid) = input.project_id {
            let _ = self.project_store.touch(pid).await;
        }
        tracing::info!("created");
        Ok(id)
    }

    /// Restore a session from storage by its ID.
    pub async fn restore_session(&self, session_id: &SessionId) -> Result<SessionId> {
        let info = self
            .session_store()
            .await
            .get(session_id)
            .await?
            .ok_or_else(|| {
                KernelError::from(SessionError::NotFound {
                    session_id: session_id.0.to_string(),
                })
            })?;

        tracing::info!(session_id = %session_id.0, "Restoring from storage");
        Ok(info.id)
    }

    /// Fork a session: create new session with copied history from parent.
    /// Also copies message history, goal state, todo list, file states, and checkpoints.
    #[tracing::instrument(skip(self, auto_approve_level), fields(parent_id = %parent_id.0))]
    pub async fn fork_session(
        &self,
        parent_id: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        let parent_info = self
            .session_store()
            .await
            .get(parent_id)
            .await?
            .ok_or_else(|| {
                KernelError::from(SessionError::NotFound {
                    session_id: parent_id.0.to_string(),
                })
            })?;

        let new_id = self.session_store().await.fork(parent_id).await?;
        // Override the copied level with the requested one
        self.session_store()
            .await
            .update_auto_approve_level(&new_id, auto_approve_level.as_str())
            .await?;
        // Set title: "Forked: <parent_title>"
        let parent_title = parent_info.title.as_deref().unwrap_or("Untitled");
        let new_title = format!("Forked: {parent_title}");
        self.rename_session(&new_id, new_title).await?;
        tracing::info!("forked session {}", new_id.0);

        // Copy message history from parent to child
        let message_store = self.message_store().await;
        if let Ok(msgs) = message_store.get(&parent_id.0).await {
            if !msgs.is_empty() {
                if let Err(e) = message_store.replace(&new_id.0, &msgs).await {
                    tracing::warn!("failed to copy message history: {}", e);
                } else {
                    tracing::info!("copied {} messages", msgs.len());
                }
            }
        }

        // Copy goal state from parent to child
        let goal_store = self.goal_store().await;
        if let Ok(Some(goal)) = goal_store.load(&parent_id.0).await {
            if let Err(e) = goal_store.save(&new_id.0, &goal).await {
                tracing::warn!("failed to copy goal state: {}", e);
            } else {
                tracing::info!("copied goal state");
            }
        }

        // Copy todo list from parent to child
        let todo_store = self.todo_store().await;
        if let Ok(Some(todos)) = todo_store.load(&parent_id.0).await {
            if let Err(e) = todo_store.save(&new_id.0, &todos).await {
                tracing::warn!("failed to copy todo list: {}", e);
            } else {
                tracing::info!("copied todo list");
            }
        }

        // Copy file states from parent to child
        let data_dir = self.data_dir().await;
        let file_states_dir = data_dir.join("sessions").join("file_states");
        let parent_file_state =
            file_states_dir.join(format!("{}.jsonl", parent_id.0.replace(['/', '\\'], "_")));
        let child_file_state =
            file_states_dir.join(format!("{}.jsonl", new_id.0.replace(['/', '\\'], "_")));
        if parent_file_state.exists() {
            if let Err(e) = tokio::fs::copy(&parent_file_state, &child_file_state).await {
                tracing::warn!("failed to copy file state: {}", e);
            } else {
                tracing::info!("copied file state");
            }
        }

        // Copy checkpoints from parent to child
        let checkpoint_store = self.checkpoint_store().await;
        match checkpoint_store
            .copy_session_checkpoints(&parent_id.0, &new_id.0)
            .await
        {
            Ok(0) => tracing::debug!("no checkpoints to copy"),
            Ok(n) => tracing::info!("copied {} checkpoints", n),
            Err(e) => tracing::warn!("failed to copy checkpoints: {}", e),
        }

        tracing::info!("forked session {} initialized", new_id.0);
        Ok(new_id)
    }

    /// List sessions with cursor-based pagination.
    pub async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<crate::client::PaginatedSessions> {
        let (sessions, next_cursor) = self
            .session_store()
            .await
            .list(project_id, before, limit)
            .await?;
        Ok(crate::client::PaginatedSessions {
            sessions,
            next_cursor,
        })
    }

    /// Return the number of sessions currently live in memory.
    pub fn live_session_count(&self) -> usize {
        self.conductor.active_count()
    }

    /// List skills available to a session, merging global skills with workspace skills.
    /// Workspace skills take precedence on name collision.
    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn list_session_skills(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<Arc<crate::skill::Skill>>> {
        let info = self
            .session_store()
            .await
            .get(session_id)
            .await?
            .ok_or_else(|| SessionError::NotFound {
                session_id: session_id.0.to_string(),
            })?;

        let working_dir = info.working_dir.map(std::path::PathBuf::from);
        let project_dir = match &info.project_id {
            Some(pid) => self.project_store.get(pid).await?.map(|p| p.dir),
            None => None,
        };
        let data_dir = self.data_dir().await;
        let cwd = working_dir
            .or(project_dir)
            .or_else(|| Some(data_dir.join("workspace")));

        let workspace_skill_dir = cwd.map(|d| d.join(".agents/skills")).filter(|d| d.exists());

        let global_skills = self.agent_config.skills.clone();

        let mut skills = match workspace_skill_dir {
            Some(dir) => {
                match crate::skill::SkillLoader::new(vec![dir]).load_all() {
                    Ok(mut ws_skills) => {
                        // Merge global and workspace skills, workspace wins on collision.
                        let mut merged = std::collections::HashMap::new();
                        for skill in global_skills {
                            merged.insert(skill.name.clone(), skill);
                        }
                        for skill in ws_skills.drain(..) {
                            merged.insert(skill.name.clone(), skill);
                        }
                        merged.into_values().collect()
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to load workspace skills for session {}: {}, using global skills only",
                            session_id.0,
                            e
                        );
                        global_skills
                    }
                }
            }
            None => global_skills,
        };

        crate::skill::deduplicate_skills(&mut skills);
        Ok(skills)
    }

    /// Get full session info merged with runtime phase
    pub async fn get_session(&self, sid: &SessionId) -> Result<crate::types::SessionResponse> {
        let phase = match self.conductor.get_state(sid) {
            Some(AgentState::Streaming) => "streaming",
            Some(AgentState::ExecutingTool) => "executing_tool",
            Some(AgentState::Compacting) => "compacting",
            Some(AgentState::Idle) | None => "idle",
        };
        let info = self.session_store().await.get(sid).await?;
        let info = info.ok_or_else(|| crate::types::SessionError::NotFound {
            session_id: sid.0.to_string(),
        })?;
        Ok(crate::types::SessionResponse {
            id: info.id,
            phase: phase.to_string(),
            title: info.title,
            parent_id: info.parent_id,
            project_id: info.project_id,
            working_dir: info.working_dir,
            message_count: info.message_count,
            created_at: info.created_at,
            updated_at: info.updated_at,
            auto_approve_level: info.auto_approve_level,
        })
    }

    /// Send a multi-modal message with content blocks
    #[tracing::instrument(skip(self, blocks), fields(session_id = %session_id.0))]
    pub async fn send_message(
        &self,
        session_id: &SessionId,
        blocks: Vec<crate::types::ContentBlock>,
    ) -> Result<()> {
        let title = normalize_session_title(&text_from_blocks(&blocks));
        if !title.is_empty() {
            let _ = self
                .session_store()
                .await
                .update_title(session_id, &title)
                .await;
            let noti = crate::notification::Notification::TitleUpdated {
                session_id: session_id.clone(),
                title: title.clone(),
            };
            let _ = self.notification_bus.send(noti);
        }
        self.input_bus
            .publish(session_id.clone(), AgentInput::User { content: blocks })
            .map_err(|e| KernelError::io(format!("InputBus full: {e}")))?;
        Ok(())
    }

    /// Subscribe to events for a session (to be called by TUI / GUI / remote client).
    /// Filters out `InternalEvent` — those are for kernel-internal consumers only.
    pub fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> crate::comms::EventBusSubscriber {
        self.agent_shared
            .event_bus
            .as_ref()
            .expect("event_bus must be configured")
            .subscribe_filtered(session_id.clone(), |envelope| {
                !matches!(envelope.event, crate::event::Event::Internal(_))
            })
    }

    /// Get the global event bus.
    pub fn event_bus(&self) -> Option<Arc<crate::comms::EventBus>> {
        self.agent_shared.event_bus.clone()
    }

    /// Send a continue command to trigger the agent from Idle to Streaming
    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub fn send_continue(&self, session_id: &SessionId) {
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Continue)
        {
            tracing::warn!("Failed to publish continue input: {}", e);
        }
    }

    #[tracing::instrument(skip(self, content), fields(session_id = %session_id.0))]
    pub fn send_steer(&self, session_id: &SessionId, content: Vec<crate::types::ContentBlock>) {
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Steer(content))
        {
            tracing::warn!("Failed to publish steer input: {}", e);
        }
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub fn cancel(&self, session_id: &SessionId) {
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Cancel)
        {
            tracing::warn!("Failed to publish cancel input: {}", e);
        }
    }

    /// Clear the session's agent context (messages, file state, todos, persisted history).
    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub fn clear_session(&self, session_id: &SessionId) {
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Clear)
        {
            tracing::warn!("Failed to publish clear input: {}", e);
        }
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) {
        if let Err(e) = self.input_bus.publish(
            session_id.clone(),
            AgentInput::PermissionResponse {
                req_id: req_id.to_string(),
                approved,
                remember,
            },
        ) {
            tracing::warn!("Failed to publish permission response: {}", e);
        }
    }

    #[tracing::instrument(skip(self, response), fields(session_id = %session_id.0))]
    pub fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: AskUserResponse,
    ) {
        if let Err(e) = self.input_bus.publish(
            session_id.clone(),
            AgentInput::AskUserResponse {
                req_id: req_id.to_string(),
                response,
            },
        ) {
            tracing::warn!("Failed to publish ask_user response: {}", e);
        }
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        let rows = self
            .session_store()
            .await
            .update_auto_approve_level(session_id, level.as_str())
            .await?;
        if rows == 0 {
            tracing::warn!("set_permission_level: no rows updated — session may not exist in DB");
        } else {
            tracing::info!(
                "permission level persisted to DB as {:?} ({} row(s) affected)",
                level,
                rows,
            );
        }
        // Update in-memory permission state for the live agent (real-time)
        if self.conductor.set_permission_level(session_id, level) {
            tracing::info!(
                "permission level updated in-memory for live agent {}",
                session_id.0
            );
        }
        Ok(())
    }

    /// Request compaction for a session's message buffer
    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub fn compact_session(&self, session_id: &SessionId) {
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Compact)
        {
            tracing::warn!("Failed to publish compact input: {}", e);
        }
    }

    /// Rewind a session to a specific checkpoint
    #[tracing::instrument(skip(self, target), fields(session_id = %session_id.0))]
    pub async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: crate::types::MessageId,
        target: crate::checkpoint::RewindTarget,
    ) -> Result<()> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        self.input_bus
            .publish(
                session_id.clone(),
                AgentInput::Rewind {
                    message_id,
                    target,
                    result_tx: tx,
                },
            )
            .map_err(|e| KernelError::io(format!("InputBus full: {e}")))?;
        match rx.recv().await {
            Some(Ok(())) => Ok(()),
            Some(Err(e)) => Err(KernelError::Checkpoint(e.to_string())),
            None => Err(KernelError::Checkpoint("Rewind channel closed".to_string())),
        }
    }

    #[tracing::instrument(skip(self, state), fields(session_id = %session_id.0))]
    pub async fn start_goal(
        &self,
        session_id: &SessionId,
        state: crate::goal::GoalState,
    ) -> Result<()> {
        let store = self.goal_store().await;
        store.save(&session_id.0, &state).await?;

        if let Err(e) = self.input_bus.publish(
            session_id.clone(),
            AgentInput::Steer(vec![crate::types::ContentBlock::Text {
                text: state.build_continue_prompt(),
            }]),
        ) {
            tracing::warn!("Failed to publish goal steer: {}", e);
        }

        if self.conductor.get_state(session_id) == Some(AgentState::Idle) {
            if let Err(e) = self
                .input_bus
                .publish(session_id.clone(), AgentInput::Continue)
            {
                tracing::warn!("Failed to publish goal continue: {}", e);
            }
        }

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalUpdated {
                        description: state.description.clone(),
                        status: state.status.as_str().to_string(),
                    }),
                ));
        }
        tracing::info!("goal mode started");
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn pause_goal(&self, session_id: &SessionId) -> Result<()> {
        let store = self.goal_store().await;
        let mut state = store.load(&session_id.0).await?.ok_or_else(|| {
            crate::types::SessionError::Other("no active goal to pause".to_string())
        })?;
        state.status = crate::goal::GoalStatus::Paused;
        store.save(&session_id.0, &state).await?;

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalUpdated {
                        description: state.description.clone(),
                        status: state.status.as_str().to_string(),
                    }),
                ));
        }
        tracing::info!("goal paused");
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn resume_goal(&self, session_id: &SessionId) -> Result<()> {
        let store = self.goal_store().await;
        let mut state = store
            .load(&session_id.0)
            .await?
            .ok_or_else(|| crate::types::SessionError::Other("no goal to resume".to_string()))?;
        state.status = crate::goal::GoalStatus::Active;
        store.save(&session_id.0, &state).await?;

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalUpdated {
                        description: state.description.clone(),
                        status: state.status.as_str().to_string(),
                    }),
                ));
        }
        tracing::info!("goal resumed");
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn get_goal(&self, session_id: &SessionId) -> Result<Option<crate::goal::GoalState>> {
        self.goal_store().await.load(&session_id.0).await
    }

    #[tracing::instrument(skip(self, description), fields(session_id = %session_id.0))]
    pub async fn update_goal(
        &self,
        session_id: &SessionId,
        description: impl Into<String>,
    ) -> Result<()> {
        let description = description.into();
        let store = self.goal_store().await;
        let mut state = store
            .load(&session_id.0)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| crate::goal::GoalState::new(&description));

        state.description = description;
        state.status = crate::goal::GoalStatus::Active;
        store.save(&session_id.0, &state).await?;

        let prompt = state.objective_updated_prompt();
        let blocks = vec![crate::types::ContentBlock::Text { text: prompt }];
        if let Err(e) = self
            .input_bus
            .publish(session_id.clone(), AgentInput::Steer(blocks))
        {
            tracing::warn!("Failed to publish goal update steer: {}", e);
        }

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalUpdated {
                        description: state.description.clone(),
                        status: state.status.as_str().to_string(),
                    }),
                ));
        }
        tracing::info!("goal updated: {}", state.description);
        Ok(())
    }

    #[tracing::instrument(skip(self), fields(session_id = %session_id.0))]
    pub async fn stop_goal(&self, session_id: &SessionId) -> Result<()> {
        self.goal_store().await.delete(&session_id.0).await?;

        if let Some(ref bus) = self.event_bus() {
            let _ = bus
                .handle(session_id.clone())
                .try_send(crate::event::Envelope::new(
                    session_id.clone(),
                    crate::event::Event::Agent(crate::event::AgentEvent::GoalStopped),
                ));
        }
        tracing::info!("goal mode stopped");
        Ok(())
    }

    /// Delete a session from storage
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.session_store().await.delete(session_id).await
    }

    /// List messages for a session with a clean typed API (User / Assistant / Tool)
    pub async fn list_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::SessionMessage>> {
        let raw = self.message_store().await.get(&session_id.0).await?;
        Ok(crate::types::SessionMessage::from_storage(raw))
    }

    /// Get checkpoints for a session.
    pub async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        self.checkpoint_store()
            .await
            .get_session_checkpoints(&session_id.0)
            .await
    }

    /// Get todo JSON for a session.
    pub async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>> {
        match &self.agent_shared.todo_storage {
            Some(store) => store.load(&session_id.0).await,
            None => Ok(None),
        }
    }

    /// Get aggregated usage summary for the last N days
    pub async fn get_usage_summary(&self, days: i64) -> Result<UsageSummary> {
        let now = Utc::now();
        let start = now - chrono::Duration::days(days);
        self.usage_store().await.summarize(start, now, None).await
    }

    /// Get daily usage for the last N days
    pub async fn get_daily_usage(&self, days: i64) -> Result<Vec<DailyUsage>> {
        let now = Utc::now();
        let start = now - chrono::Duration::days(days);
        self.usage_store()
            .await
            .daily_summary(start, now, None)
            .await
    }

    // ── Cron Job API ──────────────────────────────────────────────────────
    //
    // All cron operations go through the Kernel so that clients (GUI, TUI,
    // CLI) can use the same `KernelApi` regardless of whether they are
    // talking to an in-process kernel or a remote daemon.  Never let the client
    // layer hold a `CronStore` directly — that would break remote mode.
    //
    // DESIGN PRINCIPLE: Every mutating cron operation (create / update / delete)
    // automatically notifies the scheduler to reload, so callers never need to
    // remember to do it manually.  This keeps both local (GUI in-process) and
    // remote (KernelServer) paths consistent.

    /// Create a new cron job.  Validates the schedule expression, computes the
    /// first `next_run_at`, persists, and notifies the scheduler.
    pub async fn create_cron_job(
        &self,
        input: crate::cron::CreateCronJobInput,
    ) -> Result<crate::cron::CronJobId> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;

        let schedule = crate::cron::CronSchedule::parse(&input.schedule)
            .map_err(|e| crate::types::KernelError::storage(e.to_string()))?;

        let next_run = schedule.next_after(Utc::now());
        let job = crate::cron::CronJob {
            id: crate::cron::CronJobId::new(),
            name: input.name,
            schedule: input.schedule,
            action: input.action,
            status: crate::cron::CronJobStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            next_run_at: next_run,
            last_run_at: None,
            run_count: 0,
            max_runs: input.max_runs,
            expires_at: input.expires_at,
            last_error: None,
        };

        let id = job.id.clone();
        store.create(&job).await.map_err(|e| {
            crate::types::KernelError::storage(format!("Failed to create cron job: {e}"))
        })?;
        Ok(id)
    }

    /// List cron jobs with optional status filter.
    pub async fn list_cron_jobs(
        &self,
        status: Option<crate::cron::CronJobStatus>,
        limit: usize,
    ) -> Result<Vec<crate::cron::CronJob>> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;
        store.list(status, limit).await.map_err(|e| {
            crate::types::KernelError::storage(format!("Failed to list cron jobs: {e}"))
        })
    }

    /// Get a single cron job by ID.
    pub async fn get_cron_job(
        &self,
        id: &crate::cron::CronJobId,
    ) -> Result<Option<crate::cron::CronJob>> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;
        store
            .get(id)
            .await
            .map_err(|e| crate::types::KernelError::storage(format!("Failed to get cron job: {e}")))
    }

    /// Update a cron job.  Validates the schedule if changed, recalculates
    /// `next_run_at`, persists.  Returns `true` if the job existed.
    pub async fn update_cron_job(
        &self,
        id: &crate::cron::CronJobId,
        mut input: crate::cron::UpdateCronJobInput,
    ) -> Result<bool> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;

        if let Some(ref schedule_str) = input.schedule {
            let schedule = crate::cron::CronSchedule::parse(schedule_str)
                .map_err(|e| crate::types::KernelError::storage(e.to_string()))?;
            input.next_run_at = schedule.next_after(Utc::now());
        }

        store.update(id, &input).await.map_err(|e| {
            crate::types::KernelError::storage(format!("Failed to update cron job: {e}"))
        })
    }

    /// Delete a cron job.  Returns `true` if the job existed.
    pub async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;
        store.delete(id).await.map_err(|e| {
            crate::types::KernelError::storage(format!("Failed to delete cron job: {e}"))
        })
    }

    /// Trigger a cron job manually (execute immediately, record result).
    pub async fn trigger_cron_job(&self, id: &crate::cron::CronJobId) -> Result<()> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;

        let job = store
            .get(id)
            .await
            .map_err(|e| {
                crate::types::KernelError::storage(format!("Failed to get cron job: {e}"))
            })?
            .ok_or_else(|| crate::types::KernelError::storage("Cron job not found"))?;

        let result = crate::cron::CronExecutor::execute_cron_action(self, &job.action).await;

        let error = match &result {
            Ok(()) => None,
            Err(e) => Some(e.to_string()),
        };

        store.record_execution(id, error).await.map_err(|e| {
            crate::types::KernelError::storage(format!("Failed to record execution: {e}"))
        })?;

        result.map_err(|e| crate::types::KernelError::storage(e.to_string()))
    }
}

#[async_trait::async_trait]
impl crate::cron::CronExecutor for Kernel {
    async fn execute_cron_action(
        &self,
        action: &crate::cron::CronAction,
    ) -> std::result::Result<(), crate::cron::CronError> {
        use crate::cron::types::{render_template, CronAction, CronError};
        use crate::types::{ContentBlock, SessionId};

        match action {
            CronAction::SendMessage {
                session_id,
                content,
            } => {
                let sid = SessionId::from(session_id.clone());
                let text = render_template(content);
                let blocks = vec![ContentBlock::Text { text }];
                self.send_message(&sid, blocks)
                    .await
                    .map_err(CronError::Session)?;
            }
            CronAction::Shell {
                command,
                working_dir,
            } => {
                let output = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .current_dir(working_dir.as_deref().unwrap_or("."))
                    .kill_on_drop(true)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .output()
                    .await
                    .map_err(CronError::Io)?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(CronError::ShellFailed(stderr.to_string()));
                }
            }
            CronAction::Internal { .. } => {
                return Err(CronError::UnsupportedAction("Internal".to_string()));
            }
        }
        Ok(())
    }
}

/// Normalize session title: collapse whitespace, trim, truncate to 20 chars.
fn normalize_session_title(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    title.chars().take(20).collect::<String>()
}

fn text_from_blocks(blocks: &[crate::types::ContentBlock]) -> String {
    blocks.iter().filter_map(|b| b.as_text()).collect()
}
