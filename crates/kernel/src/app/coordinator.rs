use crate::agent::{AgentConfig, AgentShared};
use crate::app::session::{Session, SessionConfig};
use crate::event::{Event, SystemEvent};
use crate::permissions::Level;
use crate::providers::Provider;
use crate::storage::{MessageStore, SessionStore, StorageSet};
use crate::types::{KernelError, Result, SessionId};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc, RwLock};

pub struct Coordinator {
    agent_shared: Arc<AgentShared>,
    sessions: Arc<RwLock<HashMap<SessionId, Arc<RwLock<Session>>>>>,
    /// Broadcast channels for session events (for forwarding and cleanup)
    session_event_senders: Arc<RwLock<HashMap<SessionId, broadcast::Sender<Event>>>>,
    /// Epoch seconds of the last event received from any session.
    /// Updated by `forward_session_events` on every event.
    last_activity_at: Arc<AtomicU64>,
    /// Default agent configuration for new sessions.
    /// Wrapped in `RwLock` so it can be hot-reloaded in daemon mode.
    agent_config: Arc<RwLock<AgentConfig>>,
}

impl Coordinator {
    /// Get session store from `agent_shared`
    pub fn session_store(&self) -> &Arc<dyn SessionStore> {
        self.agent_shared
            .session_store
            .as_ref()
            .expect("session_store not configured")
    }

    /// Get message store from `agent_shared`
    pub fn message_store(&self) -> &Arc<dyn MessageStore> {
        self.agent_shared
            .message_store
            .as_ref()
            .expect("message_store not configured")
    }

    /// Get checkpoint store from `agent_shared`
    pub fn checkpoint_store(&self) -> Arc<dyn crate::checkpoint::CheckpointStore> {
        self.agent_shared
            .checkpoint_store
            .clone()
            .expect("checkpoint_store not configured")
    }

    /// Get data directory from `agent_shared`
    pub fn data_dir(&self) -> &std::path::PathBuf {
        &self.agent_shared.data_dir
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
    ) -> Self {
        let session_store = storage.session_store();
        let message_store = storage.message_store();
        let todo_storage = storage.todo_store();
        let todo_interceptor = Arc::new(crate::agent::TodoReminderInterceptor::new(
            todo_storage.clone(),
        ));
        let checkpoint_store = storage.checkpoint_store();
        let data_dir = storage.data_dir().to_path_buf();
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
        let agent_shared = match hook_registry {
            Some(registry) => agent_shared.with_hook_registry(registry),
            None => agent_shared,
        };

        let agent_shared = Arc::new(agent_shared);
        Self {
            agent_shared,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            session_event_senders: Arc::new(RwLock::new(HashMap::new())),
            last_activity_at: Arc::new(AtomicU64::new(Self::now_epoch())),
            agent_config: Arc::new(RwLock::new(agent_config)),
        }
    }

    fn now_epoch() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    /// Gracefully shut down a running session (cancel agent + remove from memory).
    pub async fn shutdown_session(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id).await?;
        session.read().await.cancel();
        // Note: forward_session_events will detect the channel close
        // and remove the session from sessions / session_event_senders.
        tracing::info!("Session {} shutdown requested", session_id.0);
        Ok(())
    }

    /// Seconds since the last activity across all sessions.
    pub fn idle_seconds(&self) -> u64 {
        let last = self.last_activity_at.load(Ordering::Relaxed);
        Self::now_epoch().saturating_sub(last)
    }

    /// Create a new session with the given project path and auto-approve level.
    pub async fn create_session(
        &self,
        project_path: std::path::PathBuf,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        let working_dir = project_path.to_string_lossy().to_string();
        let id = SessionId::new();
        self.session_store().create(&id, Some(&working_dir)).await?;
        let config = SessionConfig {
            agent: self.agent_config.read().await.clone(),
            project_path,
            auto_approve_level,
            data_dir: self.data_dir().clone(),
        };
        if let Err(e) = self.init_session(id.clone(), config).await {
            // Rollback: remove the orphaned storage record
            let _ = self.session_store().delete(&id).await;
            return Err(e);
        }
        tracing::info!("Session {} created", id.0);
        Ok(id)
    }

    /// Initialize a session in memory.
    /// Uses a single write-lock to avoid the race window of double-checked locking.
    async fn init_session(&self, session_id: SessionId, config: SessionConfig) -> Result<()> {
        // Hold write lock for the entire critical section.
        let mut sessions = self.sessions.write().await;
        if sessions.contains_key(&session_id) {
            return Err(KernelError::session(format!(
                "Session {} already initialized",
                session_id.0
            )));
        }

        // Create session (this may await, but we hold the lock).
        let (session, event_rx) =
            Session::init(session_id.clone(), config, Arc::clone(&self.agent_shared)).await?;

        let main_agent_id = session.main_agent_id().cloned();
        let session_arc = Arc::new(RwLock::new(session));
        let (broadcast_tx, _) = broadcast::channel::<Event>(256);

        sessions.insert(session_id.clone(), Arc::clone(&session_arc));
        drop(sessions);

        self.session_event_senders
            .write()
            .await
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
                main_agent_id,
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
        _main_agent_id: Option<crate::types::AgentId>,
        sessions: Arc<RwLock<HashMap<SessionId, Arc<RwLock<Session>>>>>,
        senders: Arc<RwLock<HashMap<SessionId, broadcast::Sender<Event>>>>,
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
        sessions.write().await.remove(&session_id);
        senders.write().await.remove(&session_id);
        tracing::info!("Session {} removed from coordinator", sid_str);
    }

    /// Restore a session from storage by its ID.
    /// If the session is already in memory (e.g., a previous client left it
    /// running in the daemon), return its ID without re-initialising.
    pub async fn restore_session(
        &self,
        session_id: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        let live = self.get_session(session_id).await.is_some();
        tracing::info!("restore_session: {} live={}", session_id.0, live);

        // Already live in the daemon – just re-attach.
        if live {
            tracing::info!("Session {} already live, re-attaching", session_id.0);
            return Ok(session_id.clone());
        }

        // Verify session exists in storage
        let session_info = self.session_store().get(session_id).await?.ok_or_else(|| {
            KernelError::session(format!("Session not found in storage: {}", session_id.0))
        })?;

        let project_path = session_info.working_dir.map_or_else(
            || {
                tracing::warn!(
                    "Session {} has no working_dir, falling back to current_dir",
                    session_id.0
                );
                std::env::current_dir().unwrap_or_default()
            },
            std::path::PathBuf::from,
        );
        let config = SessionConfig {
            agent: self.agent_config.read().await.clone(),
            project_path,
            auto_approve_level,
            data_dir: self.data_dir().clone(),
        };
        tracing::info!("Restoring session {} from storage", session_id.0);
        if let Err(e) = self.init_session(session_info.id.clone(), config).await {
            // If the session was raced into memory by another client
            // (e.g. TUI reconnect_task + CLI input_handle), treat it as
            // success instead of failing the caller's retry loop.
            if e.to_string().contains("already initialized") {
                tracing::debug!(
                    "Session {} already initialized — treating as restored",
                    session_id.0
                );
                return Ok(session_info.id);
            }
            return Err(e);
        }
        tracing::info!("Session {} restored", session_info.id.0);
        Ok(session_info.id)
    }

    /// Fork a session: create new session with copied history from parent
    pub async fn fork_session(
        &self,
        parent_id: &SessionId,
        auto_approve_level: Level,
    ) -> Result<SessionId> {
        // Create new session with copied history in storage
        let new_id = self.session_store().fork(parent_id).await?;
        tracing::info!("Forked session {} from {}", new_id.0, parent_id.0);

        // Read working_dir from parent session info
        let parent_info = self.session_store().get(parent_id).await?.ok_or_else(|| {
            KernelError::session(format!(
                "Parent session not found in storage: {}",
                parent_id.0
            ))
        })?;
        let project_path = parent_info.working_dir.map_or_else(
            || {
                tracing::warn!(
                    "Parent session {} has no working_dir, falling back to current_dir",
                    parent_id.0
                );
                std::env::current_dir().unwrap_or_default()
            },
            std::path::PathBuf::from,
        );

        let config = SessionConfig {
            agent: self.agent_config.read().await.clone(),
            project_path,
            auto_approve_level,
            data_dir: self.data_dir().clone(),
        };
        if let Err(e) = self.init_session(new_id.clone(), config).await {
            // Rollback: remove the orphaned forked storage record
            let _ = self.session_store().delete(&new_id).await;
            return Err(e);
        }
        tracing::info!("Forked session {} initialized", new_id.0);
        Ok(new_id)
    }

    pub async fn get_session(&self, id: &SessionId) -> Option<Arc<RwLock<Session>>> {
        self.sessions.read().await.get(id).cloned()
    }

    /// Get session or return not found error
    async fn require_session(&self, session_id: &SessionId) -> Result<Arc<RwLock<Session>>> {
        self.get_session(session_id)
            .await
            .ok_or_else(|| KernelError::session(format!("session_not_found: {}", session_id.0)))
    }

    pub async fn list_sessions(&self) -> Vec<SessionId> {
        self.sessions.read().await.keys().cloned().collect()
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
        let session = self.require_session(session_id).await?;
        let result = session.read().await.send_blocks(blocks).await;
        if let Err(ref e) = result {
            tracing::error!("Failed to send blocks to session {}: {}", session_id.0, e);
        }
        result
    }

    /// Subscribe to events for a session (to be called by TUI)
    /// Returns None if session not found
    /// Each call returns a new receiver that will receive all future events
    pub async fn subscribe_session_events(
        &self,
        session_id: &SessionId,
    ) -> Option<broadcast::Receiver<Event>> {
        Some(
            self.session_event_senders
                .read()
                .await
                .get(session_id)?
                .subscribe(),
        )
    }

    pub async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id).await?;
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
        let session = self.require_session(session_id).await?;
        session
            .read()
            .await
            .send_permission_response(req_id, approved, remember)
            .await?;
        Ok(())
    }

    pub async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        let session = self.require_session(session_id).await?;
        session.read().await.set_permission_level(level).await;
        tracing::info!(
            "Permission level set to {:?} for session {}",
            level,
            session_id.0
        );
        Ok(())
    }

    /// Request compaction for a session's message buffer
    pub async fn compact_session(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id).await?;
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
        let session = self.require_session(session_id).await?;
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
        let session = self.require_session(session_id).await?;
        let mut session_guard = session.write().await;
        session_guard.start_goal(state).await
    }

    /// Stop autonomous goal-mode for a session
    pub async fn stop_goal(&self, session_id: &SessionId) -> Result<()> {
        let session = self.require_session(session_id).await?;
        let mut session_guard = session.write().await;
        session_guard.stop_goal().await
    }

    /// Delete a session from storage
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.session_store().delete(session_id).await
    }

    /// Get messages for a session from storage
    pub async fn get_session_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::Message>> {
        self.message_store().get(&session_id.0).await
    }

    /// List sessions from storage with filters.
    pub async fn list_sessions_filtered(
        &self,
        args: crate::storage::session::ListArgs,
    ) -> Result<Vec<crate::storage::session::SessionInfo>> {
        self.session_store().list(args).await
    }

    /// Get checkpoints for a session.
    pub async fn get_checkpoints(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::checkpoint::Checkpoint>> {
        self.checkpoint_store()
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

    /// Update the agent configuration for new sessions.
    ///
    /// NOTE: This only updates `AgentConfig` (skills, `system_prompt`, `max_iterations`,
    /// etc.). `AgentShared` fields like `model_config` and `skill_folders` are
    /// fixed at startup and cannot be hot-reloaded without restarting the daemon.
    pub async fn update_agent_config(&self, agent_config: AgentConfig) {
        let model_id = agent_config.model.model_id.clone();
        let skill_count = agent_config.skills.len();
        *self.agent_config.write().await = agent_config;
        tracing::info!("Updated agent config (model={model_id}, {skill_count} skill(s))");
    }
}
