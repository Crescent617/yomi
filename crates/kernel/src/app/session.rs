use crate::goal::JsonGoalStore;
use crate::permissions::{Level, PermissionState};
use crate::storage::file_state::JsonlFileStateStore;
use crate::types::{AgentId, KernelError, Result, SessionError, SessionId};
use crate::{
    agent::{Agent, AgentConfig, AgentHandle, AgentShared, AgentSpawnArgs, AgentState},
    event::Event,
};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Session {
    id: SessionId,
    main_agent: Option<AgentHandle>,
    /// Shared permission state for runtime level updates
    permission_state: Option<PermissionState>,
    /// Goal store for persisting active goal state
    goal_store: Arc<dyn crate::goal::GoalStore>,
    /// Session store for title updates
    session_store: Option<Arc<dyn crate::storage::SessionStore>>,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub agent: AgentConfig,
    pub project: Option<crate::types::Project>,
    pub working_dir: Option<std::path::PathBuf>,
    pub auto_approve_level: Level,
    pub data_dir: std::path::PathBuf,
}

impl Session {
    /// Initialize a new session with the main agent spawned.
    /// This is the single entry point for session creation.
    /// Returns (Session, `mpsc::Receiver<Event>`) - the receiver must be consumed by caller.
    pub(crate) async fn init(
        id: SessionId,
        config: SessionConfig,
        agent_shared: Arc<tokio::sync::RwLock<AgentShared>>,
    ) -> Result<(Self, mpsc::Receiver<Event>)> {
        let file_state_store = Self::create_file_state_store(&id, &config).await?;
        let goal_store: Arc<dyn crate::goal::GoalStore> =
            Arc::new(JsonGoalStore::new(&config.data_dir));

        let permission_state = Self::create_permission_state(&config);

        let (main_agent, event_rx) = Self::spawn_main_agent(
            &id,
            &config,
            &agent_shared,
            &file_state_store,
            &goal_store,
            permission_state.clone(),
        )
        .await?;

        let base = agent_shared.read().await;
        let session_store = base.session_store.clone();
        
        let session = Self {
            id,
            main_agent: Some(main_agent),
            permission_state,
            goal_store,
            session_store,
        };
        Ok((session, event_rx))
    }

    /// Create and populate the file state store for this session
    async fn create_file_state_store(
        id: &SessionId,
        config: &SessionConfig,
    ) -> Result<Arc<crate::tools::helper::FileStateStore>> {
        let jsonl_store = JsonlFileStateStore::new(&id.0, &config.data_dir);
        jsonl_store.maybe_vacuum().await?;
        let persistent_store: Arc<dyn crate::storage::FileStateStore> = Arc::new(jsonl_store);

        let states = persistent_store.read_all().await?;

        let file_state_store = crate::tools::helper::FileStateStore::new()
            .with_persistent(persistent_store)
            .with_states(states.into_iter().map(|fs| (fs.path, fs.mtime)));

        Ok(Arc::new(file_state_store))
    }

    /// Create permission state if needed based on config
    fn create_permission_state(config: &SessionConfig) -> Option<PermissionState> {
        // Always create PermissionState so runtime level changes work
        Some(PermissionState::new(config.auto_approve_level).0)
    }

    /// Spawn the main agent for this session
    async fn spawn_main_agent(
        id: &SessionId,
        config: &SessionConfig,
        agent_shared: &Arc<tokio::sync::RwLock<AgentShared>>,
        file_state_store: &Arc<crate::tools::helper::FileStateStore>,
        goal_store: &Arc<dyn crate::goal::GoalStore>,
        permission_state: Option<PermissionState>,
    ) -> Result<(AgentHandle, mpsc::Receiver<Event>)> {
        let base = agent_shared.read().await;
        let history = base
            .message_store
            .as_ref()
            .ok_or_else(|| KernelError::from(SessionError::StoreNotConfigured))?
            .get(&id.0)
            .await
            .unwrap_or_default();

        // Resume active goal if one exists
        let goal_state = goal_store
            .load(&id.0)
            .await
            .ok()
            .flatten()
            .filter(|g| matches!(g.status, crate::goal::GoalStatus::Active));

        let mut spawn_args = AgentSpawnArgs::new(config.agent.system_prompt.clone(), id.0.clone())
            .with_skills(config.agent.skills.clone())
            .with_history(history)
            .with_max_iterations(config.agent.max_iterations)
            .with_subagent(config.agent.enable_subagent)
            .with_file_state_store(Arc::clone(file_state_store));

        // Only set working_dir if resolved
        if let Some(cwd) = resolve_cwd(config) {
            spawn_args = spawn_args.with_working_dir(cwd);
        }

        let goal_ctx = goal_state.map(|state| {
            crate::goal::GoalContext::new(state, Some(Arc::clone(goal_store)), id.0.clone())
        });
        if let Some(ctx) = goal_ctx {
            spawn_args = spawn_args.with_goal_ctx(ctx);
        }

        let checkpoint_store = base.checkpoint_store.clone();
        let shared = Arc::new(base.with_per_session(
            permission_state,
            Some(Arc::clone(file_state_store)),
            checkpoint_store,
        ));

        let (handle, event_rx) = Agent::spawn(AgentId::new(), &shared, spawn_args).await;
        tracing::info!("Main agent {} spawned for session {}", handle.id, id.0);

        Ok((handle, event_rx))
    }

    /// Send a multi-modal message with content blocks
    pub async fn send_blocks(&self, blocks: Vec<crate::types::ContentBlock>) -> Result<()> {
        tracing::debug!(
            "Session {} sending {} content blocks",
            self.id.0,
            blocks.len()
        );

        // Update session title from first text block of user message
        self.update_title(&blocks).await;

        match &self.main_agent {
            Some(handle) => {
                handle.send_message(blocks).await?;
                Ok(())
            }
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    /// Refresh skills for the main agent of this session
    pub async fn refresh_skills(&self, skills: Vec<Arc<crate::skill::Skill>>) -> Result<()> {
        tracing::debug!("Session {} refreshing {} skills", self.id.0, skills.len());
        match &self.main_agent {
            Some(handle) => {
                handle.refresh_skills(skills).await?;
                Ok(())
            }
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    /// Update session title from user message content (first 20 chars of first line).
    /// Only updates if the session does not yet have a title.
    async fn update_title(&self, blocks: &[crate::types::ContentBlock]) {
        if let Some(session_store) = &self.session_store {
            let text: String = blocks.iter().filter_map(|b| b.as_text()).take(1).collect();
            let title = text.chars().take(20).collect::<String>();
            if !title.is_empty() {
                // Check if session already has a title; if so, skip
                let has_existing = session_store
                    .get(&self.id)
                    .await
                    .ok()
                    .and_then(|info| info)
                    .and_then(|info| info.title)
                    .is_some_and(|t| !t.trim().is_empty());
                if !has_existing {
                    let _ = session_store.update_title(&self.id, &title).await;
                }
            }
        }
    }

    pub fn cancel(&self) {
        if let Some(handle) = &self.main_agent {
            tracing::info!("Cancelling session {}", self.id.0);
            handle.cancel();
        }
    }

    /// Gracefully shut down the main agent (sends Shutdown signal).
    pub async fn close(&self) {
        if let Some(handle) = &self.main_agent {
            tracing::info!("Closing session {}", self.id.0);
            let _ = handle.close().await;
        }
    }

    /// Send permission response to the main agent
    pub async fn send_permission_response(
        &self,
        req_id: &str,
        approved: bool,
        remember: bool,
    ) -> Result<()> {
        match &self.main_agent {
            Some(handle) => handle
                .send_permission_response(req_id, approved, remember)
                .await
                .map_err(|e| SessionError::SendFailed(format!("permission response: {e}")).into()),
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    /// Send `ask_user` response to the main agent
    pub async fn send_ask_user_response(
        &self,
        req_id: &str,
        response: crate::tools::AskUserResponse,
    ) -> Result<()> {
        match &self.main_agent {
            Some(handle) => handle
                .send_ask_user_response(req_id, response)
                .await
                .map_err(|e| SessionError::SendFailed(format!("ask_user response: {e}")).into()),
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    pub fn agent_state(&self) -> Option<AgentState> {
        self.main_agent.as_ref().map(|h| h.state())
    }

    pub const fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn main_agent_id(&self) -> Option<&AgentId> {
        self.main_agent.as_ref().map(|h| &h.id)
    }

    /// Update permission level at runtime
    pub async fn set_permission_level(&self, level: Level) {
        if let Some(ref ps) = self.permission_state {
            ps.set_auto_approve_level(level).await;
            tracing::info!(
                "Session {} permission level updated to {:?}",
                self.id.0,
                level
            );
        } else {
            tracing::warn!("Session {} has no permission state to update", self.id.0);
        }
    }

    /// Request compaction of the session's message buffer
    pub async fn compact(&self) -> Result<()> {
        tracing::debug!("Session {} requesting compaction", self.id.0);
        match &self.main_agent {
            Some(handle) => {
                handle.force_compact().await?;
                Ok(())
            }
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    /// Rewind to a specific checkpoint
    pub async fn rewind(
        &self,
        message_id: crate::types::MessageId,
        target: crate::checkpoint::RewindTarget,
    ) -> Result<()> {
        tracing::info!(
            "Session {} rewinding to message {} (target: {:?})",
            self.id.0,
            message_id.as_str(),
            target
        );
        match &self.main_agent {
            Some(handle) => {
                let result = handle.rewind(message_id, target).await.map_err(|e| {
                    KernelError::from(SessionError::SendFailed(format!("rewind request: {e}")))
                })?;
                result.map_err(|e| SessionError::RewindFailed(e.clone()).into())
            }
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    /// Start autonomous goal-mode execution.
    ///
    /// Order matters: we first activate goal mode via `set_goal`, then send the
    /// goal description as a user message. This ensures the Agent sees the
    /// description in its conversation history and begins checking for
    /// `<goal_complete>` on the very next turn.
    pub async fn start_goal(&mut self, state: crate::goal::GoalState) -> Result<()> {
        if let Some(ref handle) = self.main_agent {
            let user_message = state.to_user_message();
            let ctx = crate::goal::GoalContext::new(
                state.clone(),
                Some(Arc::clone(&self.goal_store)),
                self.id.0.clone(),
            );
            // 1. Activate goal mode so the next Idle turn enters goal idle
            handle.set_goal(Some(ctx)).await?;
            // 2. Push the goal description as a user message into the conversation
            handle.send_text(user_message).await?;
        }
        // Persist only after agent activation succeeds so that resume never
        // restores a goal that was never actually started.
        self.goal_store.save(&self.id.0, &state).await?;
        tracing::info!("Session {} goal mode started", self.id.0);
        Ok(())
    }

    /// Stop autonomous goal-mode execution
    pub async fn stop_goal(&mut self) -> Result<()> {
        // Always clear storage first so that resume never restores a stale goal,
        // even if the agent handle is already closed.
        self.goal_store.delete(&self.id.0).await?;
        if let Some(ref handle) = self.main_agent {
            let _ = handle.set_goal(None).await;
        }
        tracing::info!("Session {} goal mode stopped", self.id.0);
        Ok(())
    }
}

/// Resolve working directory from `SessionConfig`
fn resolve_cwd(config: &SessionConfig) -> Option<std::path::PathBuf> {
    config
        .working_dir
        .clone()
        .or_else(|| config.project.as_ref().map(|p| p.dir.clone()))
}
