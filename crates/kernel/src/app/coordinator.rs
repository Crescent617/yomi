use crate::agent::{AgentConfig, AgentShared, AgentState};
use crate::app::session::{Session, SessionConfig};
use crate::event::{Event, SystemEvent};
use crate::permissions::Level;
use crate::providers::ModelConfig;
use crate::providers::Provider;
use crate::storage::usage::{DailyUsage, UsageFilter, UsageSummary};
use crate::storage::{MessageStore, ProjectStore, SessionStore, StorageSet, UsageStore};
use crate::types::{KernelError, Project, ProjectId, Result, SessionError, SessionId};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, RwLock};

/// Input for creating a new session
#[derive(Debug, Clone)]
pub struct CreateSessionInput {
    pub project_id: Option<ProjectId>,
    pub working_dir: Option<std::path::PathBuf>,
    pub auto_approve_level: Level,
}

pub struct Coordinator {
    agent_shared: Arc<tokio::sync::RwLock<AgentShared>>,
    sessions: Arc<DashMap<SessionId, Arc<RwLock<Session>>>>,
    /// Broadcast channels for session events (for forwarding and cleanup)
    session_event_senders: Arc<DashMap<SessionId, broadcast::Sender<Event>>>,
    /// Epoch seconds of the last event received from any session.
    /// Updated by `forward_session_events` on every event.
    last_activity_at: Arc<AtomicU64>,
    /// Default agent configuration for new sessions.
    /// Wrapped in `RwLock` so it can be hot-reloaded in daemon mode.
    agent_config: Arc<RwLock<AgentConfig>>,
    /// Project store for project operations
    project_store: Arc<dyn ProjectStore>,
    /// Cron store for scheduled job operations.
    /// Kept here so that the Coordinator can expose a unified API for both
    /// in-process and remote clients (via `CoordinatorApi`).
    ///
    /// DESIGN PRINCIPLE: Never let the client layer hold a `CronStore` directly;
    /// that would only work in local mode and break remote IPC mode. All cron
    /// operations MUST go through the Coordinator.
    pub(crate) cron_store: Option<Arc<dyn crate::cron::CronStore>>,
}

impl Coordinator {
    /// Get session store from `agent_shared`
    pub async fn session_store(&self) -> Arc<dyn SessionStore> {
        self.agent_shared
            .read()
            .await
            .session_store
            .clone()
            .expect("session_store not configured")
    }

    /// Get message store from `agent_shared`
    pub async fn message_store(&self) -> Arc<dyn MessageStore> {
        self.agent_shared
            .read()
            .await
            .message_store
            .clone()
            .expect("message_store not configured")
    }

    /// Get checkpoint store from `agent_shared`
    pub async fn checkpoint_store(&self) -> Arc<dyn crate::checkpoint::CheckpointStore> {
        self.agent_shared
            .read()
            .await
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
        self.agent_shared.read().await.data_dir.clone()
    }

    /// Get usage store from `agent_shared`
    pub async fn usage_store(&self) -> Arc<dyn UsageStore> {
        self.agent_shared
            .read()
            .await
            .usage_store
            .clone()
            .expect("usage_store not configured")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: &StorageSet,
        provider: Arc<dyn Provider>,
        agent_config: AgentConfig,
        task_store: Option<Arc<crate::task::TaskStore>>,
        compactor: Option<crate::compactor::Compactor>,
        skill_folders: Vec<std::path::PathBuf>,
        hook_registry: Option<crate::hooks::HookRegistry>,
    ) -> Arc<Self> {
        let session_store = storage.session_store();
        let message_store = storage.message_store();
        let todo_storage = storage.todo_store();
        let todo_interceptor = Arc::new(crate::agent::TodoReminderInterceptor::new(
            todo_storage.clone(),
        ));
        let checkpoint_store = storage.checkpoint_store();
        let data_dir = storage.data_dir().to_path_buf();
        let project_store = storage.project_store();
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
        .with_message_interceptor(todo_interceptor);
        let agent_shared = agent_shared.with_tool_blocklist(agent_config.tool_blocklist.clone());
        let agent_shared = agent_shared.with_allow_command_hooks(agent_config.allow_command_hooks);
        let agent_shared = match hook_registry {
            Some(registry) => {
                agent_shared.with_hook_registry(Arc::new(tokio::sync::RwLock::new(registry)))
            }
            None => agent_shared,
        };

        let agent_shared = Arc::new(tokio::sync::RwLock::new(agent_shared));
        let sessions = Arc::new(DashMap::new());
        let session_event_senders = Arc::new(DashMap::new());
        let last_activity_at = Arc::new(AtomicU64::new(Self::now_epoch()));
        let agent_config = Arc::new(RwLock::new(agent_config));
        let cron_store = Some(storage.cron_store());

        Self::spawn_session_pruner(Arc::clone(&sessions), Arc::clone(&session_event_senders));

        Arc::new(Self {
            agent_shared,
            sessions,
            session_event_senders,
            last_activity_at,
            agent_config,
            project_store,
            cron_store,
        })
    }

    fn now_epoch() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Spawn a background task that shuts down idle sessions which no longer
    /// have any subscribers. This prevents unbounded memory growth when TUI
    /// clients disconnect while the agent is waiting for input.
    fn spawn_session_pruner(
        sessions: Arc<DashMap<SessionId, Arc<RwLock<Session>>>>,
        senders: Arc<DashMap<SessionId, broadcast::Sender<Event>>>,
    ) {
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let mut to_shutdown = Vec::new();
                for entry in senders.iter() {
                    let sid = entry.key().clone();
                    let tx = entry.value();
                    if tx.receiver_count() == 0 {
                        if let Some(s_entry) = sessions.get(&sid) {
                            if let Some(AgentState::Idle) =
                                s_entry.value().read().await.agent_state()
                            {
                                to_shutdown.push(sid);
                            }
                        }
                    }
                }
                for sid in to_shutdown {
                    tracing::info!("Session {} idle with no subscribers — shutting down", sid.0);
                    // Close the agent so it exits cleanly (Shutdown transitions
                    // to Closed, which breaks the agent loop). Then remove
                    // directly from the maps so memory is freed regardless
                    // of whether forward_session_events finishes.
                    if let Some(s_entry) = sessions.get(&sid) {
                        s_entry.value().read().await.close().await;
                    }
                    sessions.remove(&sid);
                    senders.remove(&sid);
                }
            }
        });
    }

    /// Shut down a running session (close agent + remove from memory).
    pub async fn shutdown_session(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id)?;
        session.read().await.close().await;
        self.sessions.remove(session_id);
        self.session_event_senders.remove(session_id);
        tracing::info!("Session {} shut down", session_id.0);
        Ok(())
    }

    /// Seconds since the last activity across all sessions.
    pub fn idle_seconds(&self) -> u64 {
        let last = self.last_activity_at.load(Ordering::Relaxed);
        Self::now_epoch().saturating_sub(last)
    }

    // ── Project API ──────────────────────────────────────────────────────

    /// Create a new project.
    /// If a project already exists for the given directory, returns the existing one.
    pub async fn create_project(
        &self,
        dir: std::path::PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        let abs = std::fs::canonicalize(&dir).unwrap_or(dir);
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

    /// Rename a session (update title in storage)
    pub async fn rename_session(&self, id: &SessionId, title: String) -> Result<()> {
        self.session_store().await.update_title(id, &title).await
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
    pub async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        let project = match &input.project_id {
            Some(pid) => Some(
                self.project_store
                    .get(pid)
                    .await?
                    .ok_or_else(|| SessionError::Other(format!("Project {} not found", pid.0)))?,
            ),
            None => None,
        };

        let working_dir = input.working_dir.map(|p| {
            std::fs::canonicalize(&p)
                .unwrap_or(p)
                .to_string_lossy()
                .to_string()
        });

        let id = SessionId::new();
        self.session_store()
            .await
            .create(
                &id,
                input.project_id.as_ref(),
                working_dir.as_deref(),
                Some(input.auto_approve_level.as_str()),
            )
            .await?;

        let config = SessionConfig {
            agent: self.agent_config.read().await.clone(),
            project,
            working_dir: working_dir.map(std::path::PathBuf::from),
            auto_approve_level: input.auto_approve_level,
            data_dir: self.data_dir().await.clone(),
        };

        if let Err(e) = self.init_session(id.clone(), config).await {
            let _ = self.session_store().await.delete(&id).await;
            return Err(e);
        }

        if let Some(ref pid) = input.project_id {
            let _ = self.project_store.touch(pid).await;
        }
        tracing::info!("Session {} created", id.0);
        Ok(id)
    }

    /// Initialize a session in memory.
    async fn init_session(&self, session_id: SessionId, config: SessionConfig) -> Result<()> {
        if self.sessions.contains_key(&session_id) {
            return Err(SessionError::AlreadyExists {
                session_id: session_id.0,
            }
            .into());
        }

        let (mut session, event_rx) =
            Session::init(session_id.clone(), config, Arc::clone(&self.agent_shared)).await?;

        let (broadcast_tx, _) = broadcast::channel::<Event>(256);
        session.set_event_sender(broadcast_tx.clone());

        let session_arc = Arc::new(RwLock::new(session));

        if self
            .sessions
            .insert(session_id.clone(), Arc::clone(&session_arc))
            .is_some()
        {
            // Raced with another init — close the orphaned agent so it exits.
            session_arc.read().await.close().await;
            return Err(SessionError::AlreadyExists {
                session_id: session_id.0,
            }
            .into());
        }
        self.session_event_senders
            .insert(session_id.clone(), broadcast_tx.clone());

        // Spawn event forwarding task
        let sessions_clone = Arc::clone(&self.sessions);
        let senders_clone = Arc::clone(&self.session_event_senders);
        let activity_clone = Arc::clone(&self.last_activity_at);
        let sid_clone = session_id.clone();
        tokio::spawn(async move {
            Self::forward_session_events(
                sid_clone,
                event_rx,
                broadcast_tx,
                sessions_clone,
                senders_clone,
                activity_clone,
            )
            .await;
        });

        Ok(())
    }

    /// Forward events from agent to broadcast channel and handle cleanup
    async fn forward_session_events(
        session_id: SessionId,
        mut agent_rx: mpsc::Receiver<Event>,
        broadcast_tx: broadcast::Sender<Event>,
        sessions: Arc<DashMap<SessionId, Arc<RwLock<Session>>>>,
        senders: Arc<DashMap<SessionId, broadcast::Sender<Event>>>,
        last_activity_at: Arc<AtomicU64>,
    ) {
        let sid_str = session_id.0.clone();
        tracing::info!("Event forwarding started for session {}", sid_str);

        // Forward events until the channel closes (agent ended)
        while let Some(event) = agent_rx.recv().await {
            last_activity_at.store(Self::now_epoch(), Ordering::Relaxed);
            if broadcast_tx.send(event).is_err() {
                // No active subscribers (this is ok, receivers can come and go)
                tracing::trace!("No active subscribers for session {} events", sid_str);
            }
        }

        // Agent channel closed - session is shutting down
        tracing::info!("Main agent for session {} closed", sid_str);

        // Broadcast shutdown event
        let shutdown_event = Event::System(SystemEvent::Shutdown {
            session_id: session_id.clone(),
            error: None, // TODO: capture error from agent if needed
        });
        let _ = broadcast_tx.send(shutdown_event);

        // Remove session from coordinator
        sessions.remove(&session_id);
        senders.remove(&session_id);
        tracing::info!("Session {} removed from coordinator", sid_str);
    }

    /// Restore a session from storage by its ID.
    pub async fn restore_session(&self, session_id: &SessionId) -> Result<SessionId> {
        let live = self.get_session(session_id).is_some();
        tracing::info!("restore_session: {} live={}", session_id.0, live);

        if live {
            tracing::info!("Session {} already live, re-attaching", session_id.0);
            return Ok(session_id.clone());
        }

        let info = self
            .session_store()
            .await
            .get(session_id)
            .await?
            .ok_or_else(|| {
                KernelError::from(SessionError::NotFound {
                    session_id: session_id.0.clone(),
                })
            })?;

        let auto_approve_level = info
            .auto_approve_level
            .as_deref()
            .and_then(|s| s.parse().ok())
            .unwrap_or(Level::Safe);

        let project = match &info.project_id {
            Some(pid) => self.project_store.get(pid).await?,
            None => None,
        };
        let working_dir = info.working_dir.map(std::path::PathBuf::from);

        let config = SessionConfig {
            agent: self.agent_config.read().await.clone(),
            project,
            working_dir,
            auto_approve_level,
            data_dir: self.data_dir().await.clone(),
        };
        tracing::info!("Restoring session {} from storage", session_id.0);
        if let Err(e) = self.init_session(info.id.clone(), config).await {
            if e.is_session_already_exists() {
                tracing::debug!(
                    "Session {} already initialized — treating as restored",
                    session_id.0
                );
                return Ok(info.id);
            }
            return Err(e);
        }
        tracing::info!("Session {} restored", info.id.0);
        Ok(info.id)
    }

    /// Fork a session: create new session with copied history from parent.
    /// `auto_approve_level` overrides the parent's level for the new session.
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
                    session_id: parent_id.0.clone(),
                })
            })?;

        let new_id = self.session_store().await.fork(parent_id).await?;
        // Override the copied level with the requested one
        self.session_store()
            .await
            .update_auto_approve_level(&new_id, auto_approve_level.as_str())
            .await?;
        tracing::info!("Forked session {} from {}", new_id.0, parent_id.0);

        let project = match &parent_info.project_id {
            Some(pid) => self.project_store.get(pid).await?,
            None => None,
        };

        let config = SessionConfig {
            agent: self.agent_config.read().await.clone(),
            project,
            working_dir: parent_info.working_dir.map(std::path::PathBuf::from),
            auto_approve_level,
            data_dir: self.data_dir().await.clone(),
        };

        if let Err(e) = self.init_session(new_id.clone(), config).await {
            let _ = self.session_store().await.delete(&new_id).await;
            return Err(e);
        }
        tracing::info!("Forked session {} initialized", new_id.0);
        Ok(new_id)
    }

    /// List sessions with cursor-based pagination.
    pub async fn list_sessions(
        &self,
        project_id: Option<&ProjectId>,
        before: Option<DateTime<Utc>>,
        limit: usize,
    ) -> Result<(Vec<crate::storage::session::SessionInfo>, bool)> {
        self.session_store()
            .await
            .list(project_id, before, limit)
            .await
    }

    pub fn get_session(&self, id: &SessionId) -> Option<Arc<RwLock<Session>>> {
        self.sessions.get(id).map(|e| Arc::clone(e.value()))
    }

    /// Get runtime status for a session (streaming, compacting, etc.)
    pub async fn get_session_status(&self, id: &SessionId) -> Result<crate::types::SessionStatus> {
        let session = self.require_session(id)?;
        let session = session.read().await;
        Ok(crate::types::SessionStatus {
            streaming: session.is_streaming(),
            compacting: session.is_compacting(),
        })
    }

    fn require_session(&self, session_id: &SessionId) -> Result<Arc<RwLock<Session>>> {
        self.get_session(session_id).ok_or_else(|| {
            SessionError::NotFound {
                session_id: session_id.0.clone(),
            }
            .into()
        })
    }

    /// Return the number of sessions currently live in memory.
    pub fn live_session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Send a multi-modal message with content blocks
    pub async fn send_message(
        &self,
        session_id: &SessionId,
        blocks: Vec<crate::types::ContentBlock>,
    ) -> Result<()> {
        tracing::debug!(
            "Sending {} content blocks to session {}",
            blocks.len(),
            session_id.0
        );
        let session = self.require_session(session_id)?;
        let result = session.read().await.send_blocks(blocks).await;
        if let Err(ref e) = result {
            tracing::error!("Failed to send blocks to session {}: {}", session_id.0, e);
        }
        result
    }

    /// Subscribe to events for a session (to be called by TUI)
    pub fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Option<broadcast::Receiver<Event>> {
        Some(
            self.session_event_senders
                .get(session_id)?
                .value()
                .subscribe(),
        )
    }

    /// Send a steer message to a session (injected before next streaming turn)
    pub async fn send_steer(
        &self,
        session_id: &SessionId,
        content: Vec<crate::types::ContentBlock>,
    ) -> Result<()> {
        tracing::debug!("Sending steer to session {}", session_id.0);
        let session = self.require_session(session_id)?;
        let result = session.read().await.send_steer(content).await;
        if let Err(ref e) = result {
            tracing::error!("Failed to send steer to session {}: {}", session_id.0, e);
        }
        result
    }

    pub async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id)?;
        session.read().await.cancel();
        Ok(())
    }

    pub async fn send_permission_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()> {
        let session = self.require_session(session_id)?;
        session
            .read()
            .await
            .send_permission_response(req_id, approved, remember)
            .await?;
        Ok(())
    }

    pub async fn send_ask_user_response(
        &self,
        session_id: &SessionId,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()> {
        let session = self.require_session(session_id)?;
        session
            .read()
            .await
            .send_ask_user_response(req_id, response)
            .await?;
        Ok(())
    }

    pub async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        // Update in-memory state if session is currently live
        if let Some(session) = self.get_session(session_id) {
            session.read().await.set_permission_level(level).await;
        }
        // Always persist to database regardless of whether session is in memory
        let rows = self
            .session_store()
            .await
            .update_auto_approve_level(session_id, level.as_str())
            .await?;
        if rows == 0 {
            tracing::warn!(
                "set_permission_level: no rows updated for session {} — session may not exist in DB",
                session_id.0
            );
        } else {
            tracing::info!(
                "Permission level persisted to DB as {:?} for session {} ({} row(s) affected)",
                level,
                session_id.0,
                rows
            );
        }
        Ok(())
    }

    /// Request compaction for a session's message buffer
    pub async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id)?;
        let result = session.read().await.compact().await;
        if let Err(ref e) = result {
            tracing::error!("Failed to compact session {}: {}", session_id.0, e);
        } else {
            tracing::info!("Compaction requested for session {}", session_id.0);
        }
        result
    }

    /// Rewind a session to a specific checkpoint
    pub async fn rewind_session(
        &self,
        session_id: &SessionId,
        message_id: crate::types::MessageId,
        target: crate::checkpoint::RewindTarget,
    ) -> Result<()> {
        let session = self.require_session(session_id)?;
        let result = session.read().await.rewind(message_id, target).await;
        if let Err(ref e) = result {
            tracing::error!("Failed to rewind session {}: {}", session_id.0, e);
        } else {
            tracing::info!("Session {} rewound successfully", session_id.0);
        }
        result
    }

    /// Start autonomous goal-mode for a session
    pub async fn start_goal(
        &self,
        session_id: &SessionId,
        state: crate::goal::GoalState,
    ) -> Result<()> {
        let session = self.require_session(session_id)?;
        let mut session_guard = session.write().await;
        session_guard.start_goal(state).await
    }

    /// Stop autonomous goal-mode for a session
    pub async fn stop_goal(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id)?;
        let mut session_guard = session.write().await;
        session_guard.stop_goal().await
    }

    /// Delete a session from storage
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.session_store().await.delete(session_id).await
    }

    /// Get messages for a session from storage
    pub async fn get_session_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::Message>> {
        self.message_store().await.get(&session_id.0).await
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
        match &self.agent_shared.read().await.todo_storage {
            Some(store) => store.load(&session_id.0).await,
            None => Ok(None),
        }
    }

    /// Update the agent configuration for new sessions.
    pub async fn update_agent_config(
        &self,
        agent_config: AgentConfig,
        hook_registry: Option<crate::hooks::HookRegistry>,
        provider: Option<Arc<dyn Provider>>,
        model_config: Option<Arc<ModelConfig>>,
    ) {
        let model_id = agent_config.model.model_id.clone();
        let skills = agent_config.skills.clone();
        let skill_count = skills.len();
        *self.agent_config.write().await = agent_config;

        // Hot-reload shared provider and model config if provided
        if provider.is_some() || model_config.is_some() {
            let mut guard = self.agent_shared.write().await;
            let mut updated = guard.clone();
            if let Some(p) = provider {
                updated = updated.with_provider(p);
            }
            if let Some(m) = model_config {
                updated = updated.with_model_config(m);
            }
            *guard = updated;
        }

        // Hot-reload shared hook registry if it was originally enabled
        if let Some(registry) = hook_registry {
            if let Some(ref existing) = self.agent_shared.read().await.hook_registry {
                let mut guard = existing.write().await;
                *guard = registry;
                tracing::info!("Hot-reloaded shared hook registry");
            } else {
                tracing::warn!("Cannot hot-reload hooks: hooks were disabled at daemon startup");
            }
        }

        // Propagate skill refresh to all live sessions.
        let handles: Vec<_> = self
            .sessions
            .iter()
            .map(|e| Arc::clone(e.value()))
            .collect();
        for session in handles {
            let session = session.read().await;
            if let Err(e) = session.refresh_skills(skills.clone()).await {
                let sid = session.id().clone();
                tracing::warn!("Failed to refresh skills for session {}: {}", sid.0, e);
            }
        }

        tracing::info!("Updated agent config (model={model_id}, {skill_count} skill(s))");
    }

    /// Reload agent configuration from disk and environment.
    pub async fn reload(
        &self,
        config_file: Option<&std::path::PathBuf>,
        base_dir: &std::path::Path,
    ) -> Result<()> {
        let mut config = match config_file {
            Some(path) => crate::config::Config::from_file(path).map_err(|e| {
                crate::types::KernelError::from(crate::types::SessionError::Other(format!(
                    "Failed to load config from {}: {e}",
                    path.display()
                )))
            })?,
            None => match crate::config::Config::discover_file() {
                Some(path) => crate::config::Config::from_file(&path).map_err(|e| {
                    crate::types::KernelError::from(crate::types::SessionError::Other(format!(
                        "Failed to load discovered config from {}: {e}",
                        path.display()
                    )))
                })?,
                None => crate::config::Config::default(),
            },
        };
        config.apply_env_overrides();
        config.finalize(base_dir);

        let provider: Arc<dyn crate::providers::Provider> = if config.has_api_key() {
            match config.agent.model.provider {
                crate::config::ModelProvider::OpenAI => {
                    Arc::new(crate::providers::OpenAIProvider::new().map_err(|e| {
                        crate::types::KernelError::from(crate::types::SessionError::Other(format!(
                            "Failed to create OpenAI provider: {e}"
                        )))
                    })?)
                }
                crate::config::ModelProvider::Anthropic => {
                    Arc::new(crate::providers::AnthropicProvider::new().map_err(|e| {
                        crate::types::KernelError::from(crate::types::SessionError::Other(format!(
                            "Failed to create Anthropic provider: {e}"
                        )))
                    })?)
                }
            }
        } else {
            tracing::warn!("No API key configured — using NoKeyProvider");
            Arc::new(crate::providers::NoKeyProvider)
        };

        let _skill_folders: Vec<std::path::PathBuf> = config
            .skill_folders()
            .iter()
            .map(std::path::PathBuf::from)
            .map(|p| if p.is_relative() { base_dir.join(p) } else { p })
            .collect();

        let agent_config = crate::server::build_agent_config(&config, base_dir);
        let hook_registry = config.features.hooks.then(|| {
            crate::hooks::build_registry(&config.hooks, config.features.allow_command_hooks)
        });

        self.update_agent_config(
            agent_config,
            hook_registry,
            Some(provider),
            Some(Arc::new(config.agent.model)),
        )
        .await;
        tracing::info!("Reloaded agent configuration from disk");
        Ok(())
    }

    /// Get aggregated usage summary for today
    pub async fn get_usage_summary(&self) -> Result<UsageSummary> {
        let now = Utc::now();
        let start = now - chrono::Duration::days(1);
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

    /// Get usage for a specific session
    pub async fn get_session_usage(&self, _session_id: &SessionId) -> Result<UsageSummary> {
        let now = Utc::now();
        let start = now - chrono::Duration::days(365); // all time
        let filter = UsageFilter {
            model: None,
            provider: None,
            usage_type: None,
        };
        // TODO: UsageStore doesn't support session filter yet, so we get all and filter client-side
        // or extend the filter. For now, just return the total summary.
        self.usage_store()
            .await
            .summarize(start, now, Some(&filter))
            .await
    }

    // ── Cron Job API ──────────────────────────────────────────────────────
    //
    // All cron operations go through the Coordinator so that clients (GUI, TUI,
    // CLI) can use the same `CoordinatorApi` regardless of whether they are
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
    ///
    /// Caller is responsible for notifying the scheduler to reload if needed.
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
    ///
    /// Caller is responsible for notifying the scheduler to reload if needed.
    pub async fn delete_cron_job(&self, id: &crate::cron::CronJobId) -> Result<bool> {
        let store = self
            .cron_store
            .as_ref()
            .ok_or_else(|| crate::types::KernelError::storage("Cron store not configured"))?;
        store.delete(id).await.map_err(|e| {
            crate::types::KernelError::storage(format!("Failed to delete cron job: {e}"))
        })
    }
}

#[async_trait::async_trait]
impl crate::cron::CronExecutor for Coordinator {
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
                let sid = SessionId(session_id.clone());
                if self.get_session(&sid).is_none() {
                    self.restore_session(&sid)
                        .await
                        .map_err(CronError::Session)?;
                }
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
