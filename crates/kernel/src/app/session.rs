use crate::agent::{Agent, AgentConfig, AgentHandle, AgentShared, AgentSpawnArgs, AgentState};
use crate::event::{Event, SystemEvent};
use crate::permissions::{Level, PermissionState};
use crate::skill::SkillLoader;
use crate::storage::file_state::JsonlFileStateStore;
use crate::types::{AgentId, KernelError, Result, SessionError, SessionId};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc};

pub struct Session {
    id: SessionId,
    main_agent: Option<AgentHandle>,
    /// Shared permission state for runtime level updates
    permission_state: Option<PermissionState>,
    /// Goal store for persisting active goal state
    goal_store: Arc<dyn crate::goal::GoalStore>,
    /// Session store for title updates
    session_store: Option<Arc<dyn crate::storage::SessionStore>>,
    /// Event broadcast sender for emitting session-level events (e.g. title updates)
    event_tx: Option<broadcast::Sender<Event>>,
    /// Workspace skill directory (e.g. `<cwd>/.agents/skills`) loaded when the session starts.
    /// Kept so that `refresh_skills` can re-merge workspace skills after a global reload.
    workspace_skill_dir: Option<PathBuf>,
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
        let goal_store = agent_shared
            .read()
            .await
            .goal_store
            .clone()
            .expect("goal_store configured by coordinator");

        let permission_state = Some(Self::create_permission_state(&config));

        let workspace_skill_dir = resolve_cwd(&config)
            .map(|cwd| cwd.join(".agents/skills"))
            .filter(|d| d.exists());

        let (main_agent, event_rx) = Self::spawn_main_agent(
            &id,
            &config,
            &agent_shared,
            &file_state_store,
            permission_state.clone(),
            workspace_skill_dir.as_ref(),
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
            event_tx: None,
            workspace_skill_dir,
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
    fn create_permission_state(config: &SessionConfig) -> PermissionState {
        // Always create PermissionState so runtime level changes work
        PermissionState::new(config.auto_approve_level).0
    }

    /// Spawn the main agent for this session.
    /// If `workspace_skill_dir` is provided, workspace skills are loaded and merged
    /// with the global skills (workspace skills take precedence on name collision).
    /// The workspace directory is also appended to `AgentShared.skill_folders` so
    /// that the `skill_load` tool can resolve workspace skills at runtime.
    async fn spawn_main_agent(
        id: &SessionId,
        config: &SessionConfig,
        agent_shared: &Arc<tokio::sync::RwLock<AgentShared>>,
        file_state_store: &Arc<crate::tools::helper::FileStateStore>,
        permission_state: Option<PermissionState>,
        workspace_skill_dir: Option<&PathBuf>,
    ) -> Result<(AgentHandle, mpsc::Receiver<Event>)> {
        let base = agent_shared.read().await;
        let history = base
            .message_store
            .as_ref()
            .ok_or_else(|| KernelError::from(SessionError::StoreNotConfigured))?
            .get(&id.0)
            .await
            .unwrap_or_default();

        // Merge global skills with workspace skills (workspace wins on duplicate names)
        let mut skills = config.agent.skills.clone();
        if let Some(dir) = workspace_skill_dir {
            match SkillLoader::new(vec![dir.clone()]).load_all() {
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

        let mut spawn_args = AgentSpawnArgs::new(config.agent.system_prompt.clone(), id.0.clone())
            .with_skills(skills)
            .with_history(history)
            .with_max_iterations(config.agent.max_iterations)
            .with_subagent(config.agent.enable_subagent)
            .with_file_state_store(Arc::clone(file_state_store));

        // Only set working_dir if resolved
        if let Some(cwd) = resolve_cwd(config) {
            spawn_args = spawn_args.with_working_dir(cwd);
        }

        // Clone AgentShared so we can mutate skill_folders per-session
        let mut base_clone = base.clone();
        drop(base); // release read lock before we move base_clone
        if let Some(dir) = workspace_skill_dir {
            if !base_clone.skill_folders.contains(dir) {
                base_clone.skill_folders.push(dir.clone());
            }
        }
        let checkpoint_store = base_clone.checkpoint_store.clone();
        let shared = Arc::new(base_clone.with_per_session(
            permission_state,
            Some(Arc::clone(file_state_store)),
            checkpoint_store,
        ));

        let (handle, event_rx) = Agent::spawn(AgentId::new(), &shared, spawn_args).await;
        tracing::info!("main agent {} spawned", handle.id);

        Ok((handle, event_rx))
    }

    /// Send a multi-modal message with content blocks
    #[tracing::instrument(skip(self))]
    pub async fn send_blocks(&self, blocks: Vec<crate::types::ContentBlock>) -> Result<()> {
        tracing::debug!("sending {} content blocks", blocks.len());

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

    /// Reload skills for the main agent of this session.
    /// If the session was started with a workspace skill directory, the workspace
    /// skills are re-loaded and merged with the provided (global) skills — workspace
    /// skills take precedence on name collision.
    #[tracing::instrument(skip(self))]
    pub async fn reload_skills(&self, skills: Vec<Arc<crate::skill::Skill>>) -> Result<()> {
        let merged = if let Some(ref dir) = self.workspace_skill_dir {
            match SkillLoader::new(vec![dir.clone()]).load_all() {
                Ok(mut ws_skills) => {
                    let ws_count = ws_skills.len();
                    let mut merged = std::collections::HashMap::new();
                    for skill in &skills {
                        merged.insert(skill.name.clone(), skill.clone());
                    }
                    for skill in ws_skills.drain(..) {
                        merged.insert(skill.name.clone(), skill);
                    }
                    let result: Vec<_> = merged.into_values().collect();
                    tracing::info!(
                        "reloaded with {} skill(s) ({} global + {} workspace, merged)",
                        result.len(),
                        skills.len(),
                        ws_count
                    );
                    result
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to reload workspace skills from {}: {}, using global skills only",
                        dir.display(),
                        e
                    );
                    skills
                }
            }
        } else {
            skills
        };

        match &self.main_agent {
            Some(handle) => {
                handle.reload_skills(merged).await?;
                Ok(())
            }
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    pub fn workspace_skill_dir(&self) -> Option<&PathBuf> {
        self.workspace_skill_dir.as_ref()
    }

    /// Reload full agent configuration for the main agent of this session.
    /// The caller (coordinator) provides the new `shared`, which already includes
    /// the latest provider, `model_config`, and (if applicable) workspace skill folders.
    #[tracing::instrument(skip(self, shared))]
    pub async fn reload_config(
        &self,
        mut config: AgentConfig,
        shared: Arc<AgentShared>,
    ) -> Result<()> {
        let merged = if let Some(ref dir) = self.workspace_skill_dir {
            match SkillLoader::new(vec![dir.clone()]).load_all() {
                Ok(mut ws_skills) => {
                    let ws_count = ws_skills.len();
                    let mut merged = std::collections::HashMap::new();
                    for skill in &config.skills {
                        merged.insert(skill.name.clone(), skill.clone());
                    }
                    for skill in ws_skills.drain(..) {
                        merged.insert(skill.name.clone(), skill);
                    }
                    let result: Vec<_> = merged.into_values().collect();
                    tracing::info!(
                        "reloaded with {} skill(s) ({} global + {} workspace, merged)",
                        result.len(),
                        config.skills.len(),
                        ws_count
                    );
                    result
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to reload workspace skills from {}: {}, using global skills only",
                        dir.display(),
                        e
                    );
                    config.skills
                }
            }
        } else {
            config.skills
        };

        config.skills = merged;

        match &self.main_agent {
            Some(handle) => {
                handle.reload_config(config, shared).await?;
                Ok(())
            }
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    /// Update session title from user message content (trim, collapse whitespace, first 20 chars).
    async fn update_title(&self, blocks: &[crate::types::ContentBlock]) {
        if let Some(session_store) = &self.session_store {
            let text: String = blocks.iter().filter_map(|b| b.as_text()).take(1).collect();
            let title = normalize_session_title(&text);
            if !title.is_empty() {
                match session_store.update_title(&self.id, &title).await {
                    Ok(()) => {
                        if let Some(ref tx) = self.event_tx {
                            let _ = tx.send(Event::System(SystemEvent::TitleUpdated {
                                session_id: self.id.clone(),
                                title: title.clone(),
                            }));
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to update title: {}", e);
                    }
                }
            }
        }
    }

    /// Set the event broadcast sender so the session can emit events.
    pub fn set_event_sender(&mut self, tx: broadcast::Sender<Event>) {
        self.event_tx = Some(tx);
    }

    #[tracing::instrument(skip(self))]
    pub fn cancel(&self) {
        if let Some(handle) = &self.main_agent {
            tracing::info!("cancelling session");
            handle.cancel();
        }
    }

    /// Gracefully shut down the main agent (sends Shutdown signal).
    #[tracing::instrument(skip(self))]
    pub async fn close(&self) {
        if let Some(handle) = &self.main_agent {
            tracing::info!("closing session");
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

    /// Whether the main agent is currently streaming
    pub fn is_streaming(&self) -> bool {
        self.main_agent
            .as_ref()
            .is_some_and(|h| h.state() == AgentState::Streaming)
    }

    /// Whether the main agent is currently compacting messages
    pub fn is_compacting(&self) -> bool {
        self.main_agent.as_ref().is_some_and(|h| h.is_compacting())
    }

    pub const fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn main_agent_id(&self) -> Option<&AgentId> {
        self.main_agent.as_ref().map(|h| &h.id)
    }

    /// Update permission level at runtime
    #[tracing::instrument(skip(self))]
    pub async fn set_permission_level(&self, level: Level) {
        if let Some(ref ps) = self.permission_state {
            ps.set_auto_approve_level(level).await;
            tracing::info!("permission level updated to {:?}", level);
        } else {
            tracing::warn!("has no permission state to update");
        }
    }

    /// Send a steer message to the main agent (injected before next streaming turn)
    pub fn send_steer(&self, content: Vec<crate::types::ContentBlock>) -> Result<()> {
        match &self.main_agent {
            Some(handle) => handle
                .send_steer(content)
                .map_err(|e| SessionError::SendFailed(format!("steer: {e}")).into()),
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    /// Send a continue command to trigger the agent from Idle to Streaming
    pub async fn send_continue(&self) -> Result<()> {
        match &self.main_agent {
            Some(handle) => handle
                .send_continue()
                .map_err(|e| SessionError::SendFailed(format!("continue: {e}")).into()),
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    #[tracing::instrument(skip(self))]
    pub async fn compact(&self) -> Result<()> {
        tracing::debug!("requesting compaction");
        match &self.main_agent {
            Some(handle) => {
                handle.force_compact().await?;
                Ok(())
            }
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    /// Rewind to a specific checkpoint
    #[tracing::instrument(skip(self))]
    pub async fn rewind(
        &self,
        message_id: crate::types::MessageId,
        target: crate::checkpoint::RewindTarget,
    ) -> Result<()> {
        tracing::info!(
            "rewinding to message {} (target: {:?})",
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
    /// 1. Persist the goal state.
    /// 2. Inject the goal continuation prompt as a steer message.
    /// 3. Send Continue to trigger the agent from Idle to Streaming.
    #[tracing::instrument(skip(self))]
    pub async fn start_goal(&mut self, state: crate::goal::GoalState) -> Result<()> {
        self.goal_store.save(&self.id.0, &state).await?;
        if let Some(ref handle) = self.main_agent {
            handle
                .send_steer(vec![crate::types::ContentBlock::Text {
                    text: state.build_continue_prompt(),
                }])
                .map_err(|e| SessionError::SendFailed(format!("goal start steer: {e}")))?;
            if handle.state() == AgentState::Idle {
                if let Err(e) = handle.send_continue() {
                    tracing::warn!("goal start continue failed: {}", e);
                }
            }
        }
        self.emit_goal_updated(&state);
        tracing::info!("goal mode started");
        Ok(())
    }

    /// Pause goal auto-continue. The agent will stop after the current turn.
    #[tracing::instrument(skip(self))]
    pub async fn pause_goal(&mut self) -> Result<()> {
        let mut state = self
            .goal_store
            .load(&self.id.0)
            .await?
            .ok_or_else(|| SessionError::Other("no active goal to pause".to_string()))?;
        state.status = crate::goal::GoalStatus::Paused;
        self.goal_store.save(&self.id.0, &state).await?;
        self.emit_goal_updated(&state);
        tracing::info!("goal paused");
        Ok(())
    }

    /// Resume goal auto-continue. Does not trigger agent — next turn will PreStop-continue.
    #[tracing::instrument(skip(self))]
    pub async fn resume_goal(&mut self) -> Result<()> {
        let mut state = self
            .goal_store
            .load(&self.id.0)
            .await?
            .ok_or_else(|| SessionError::Other("no goal to resume".to_string()))?;
        state.status = crate::goal::GoalStatus::Active;
        self.goal_store.save(&self.id.0, &state).await?;
        self.emit_goal_updated(&state);
        tracing::info!("goal resumed");
        Ok(())
    }

    /// Get current goal state, if any.
    pub async fn get_goal(&self) -> Result<Option<crate::goal::GoalState>> {
        self.goal_store.load(&self.id.0).await
    }

    /// Update an active goal's description and inject an objective-updated prompt.
    /// If no goal exists, creates a new active goal.
    #[tracing::instrument(skip(self, description))]
    pub async fn update_goal(&mut self, description: impl Into<String>) -> Result<()> {
        let description = description.into();

        let mut state = self
            .goal_store
            .load(&self.id.0)
            .await
            .ok()
            .flatten()
            .unwrap_or_else(|| crate::goal::GoalState::new(&description));

        state.description = description;
        state.status = crate::goal::GoalStatus::Active;

        // Persist the updated goal
        self.goal_store.save(&self.id.0, &state).await?;

        // Inject objective-updated prompt as a steer message
        let prompt = state.objective_updated_prompt();
        let blocks = vec![crate::types::ContentBlock::Text { text: prompt }];
        self.send_steer(blocks)
            .map_err(|e| SessionError::SendFailed(format!("update goal steer: {e}")))?;

        self.emit_goal_updated(&state);
        tracing::info!("goal updated: {}", state.description);
        Ok(())
    }

    /// Stop autonomous goal-mode execution
    #[tracing::instrument(skip(self))]
    pub async fn stop_goal(&mut self) -> Result<()> {
        // Always clear storage first so that resume never restores a stale goal,
        // even if the agent handle is already closed.
        self.goal_store.delete(&self.id.0).await?;
        self.emit_goal_stopped();
        tracing::info!("goal mode stopped");
        Ok(())
    }

    /// Emit `TitleUpdated` event if event sender is configured.
    pub(crate) fn emit_title_updated(&self, title: &str) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(Event::System(SystemEvent::TitleUpdated {
                session_id: self.id.clone(),
                title: title.to_string(),
            }));
        }
    }

    /// Emit `GoalUpdated` event if event sender is configured.
    fn emit_goal_updated(&self, state: &crate::goal::GoalState) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(Event::System(SystemEvent::GoalUpdated {
                session_id: self.id.clone(),
                description: state.description.clone(),
                status: state.status.as_str().to_string(),
            }));
        }
    }

    /// Emit `GoalStopped` event if event sender is configured.
    fn emit_goal_stopped(&self) {
        if let Some(ref tx) = self.event_tx {
            let _ = tx.send(Event::System(SystemEvent::GoalStopped {
                session_id: self.id.clone(),
            }));
        }
    }
}

/// Normalize session title: collapse whitespace, trim, truncate to 20 chars.
pub fn normalize_session_title(title: &str) -> String {
    let title = title.split_whitespace().collect::<Vec<_>>().join(" ");
    title.chars().take(20).collect::<String>()
}

/// Resolve working directory from `SessionConfig`
fn resolve_cwd(config: &SessionConfig) -> Option<std::path::PathBuf> {
    config
        .working_dir
        .clone()
        .or_else(|| config.project.as_ref().map(|p| p.dir.clone()))
}
