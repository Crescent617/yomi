use crate::agent::{Agent, AgentConfig, AgentHandle, AgentShared, AgentSpawnArgs, AgentState};
use crate::event::{Event, SystemEvent};
use crate::permissions::{Level, PermissionState};
use crate::skill::SkillLoader;
use crate::storage::file_state::JsonlFileStateStore;
use crate::types::{AgentId, KernelError, Result, SessionError, SessionId};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub struct Session {
    id: SessionId,
    main_agent: Option<AgentHandle>,
    /// Shared permission state for runtime level updates
    permission_state: Option<PermissionState>,
    /// Session store for title updates
    session_store: Option<Arc<dyn crate::storage::SessionStore>>,
    /// Event bus handle for emitting session-level events (e.g. title updates)
    event_bus: Option<crate::event_bus::EventBusHandle>,
    /// Workspace skill directory (e.g. `<cwd>/.agents/skills`) loaded when the session starts.
    /// Kept so that `refresh_skills` can re-merge workspace skills after a global reload.
    workspace_skill_dir: Option<PathBuf>,
    /// Epoch seconds of the last user activity (`send_blocks`, etc.).
    last_activity_at: AtomicU64,
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
    pub(crate) async fn init(
        id: SessionId,
        config: SessionConfig,
        agent_shared: Arc<AgentShared>,
    ) -> Result<Self> {
        let file_state_store = Self::create_file_state_store(&id, &config).await?;

        let permission_state = Some(Self::create_permission_state(&config));

        let workspace_skill_dir = resolve_cwd(&config)
            .map(|cwd| cwd.join(".agents/skills"))
            .filter(|d| d.exists());

        let main_agent = Self::spawn_main_agent(
            &id,
            &config,
            &agent_shared,
            &file_state_store,
            permission_state.clone(),
            workspace_skill_dir.as_ref(),
        )
        .await?;

        let session_store = agent_shared.session_store.clone();

        let event_bus = agent_shared
            .event_bus
            .as_ref()
            .map(|bus| bus.handle(id.clone()));

        let session = Self {
            id,
            main_agent: Some(main_agent),
            permission_state,
            session_store,
            event_bus,
            workspace_skill_dir,
            last_activity_at: AtomicU64::new(now_epoch()),
        };
        Ok(session)
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
        agent_shared: &Arc<AgentShared>,
        file_state_store: &Arc<crate::tools::helper::FileStateStore>,
        permission_state: Option<PermissionState>,
        workspace_skill_dir: Option<&PathBuf>,
    ) -> Result<AgentHandle> {
        let history = agent_shared
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
            .with_file_state_store(Arc::clone(file_state_store))
            .with_tool_blocklist(config.agent.tool_blocklist.clone())
            .with_allow_command_hooks(config.agent.allow_command_hooks);

        // Only set working_dir if resolved
        if let Some(cwd) = resolve_cwd(config) {
            spawn_args = spawn_args.with_working_dir(cwd);
        }

        // Clone AgentShared so we can mutate skill_folders per-session
        let mut base_clone: AgentShared = (**agent_shared).clone();
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

        let handle = Agent::spawn(AgentId::new(), &shared, spawn_args).await;
        tracing::info!("main agent {} spawned", handle.id);

        Ok(handle)
    }

    /// Send a multi-modal message with content blocks
    pub async fn send_blocks(&self, blocks: Vec<crate::types::ContentBlock>) -> Result<()> {
        tracing::debug!("sending {} content blocks", blocks.len());
        self.touch();

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

    pub fn workspace_skill_dir(&self) -> Option<&PathBuf> {
        self.workspace_skill_dir.as_ref()
    }

    /// Update session title from user message content (trim, collapse whitespace, first 20 chars).
    async fn update_title(&self, blocks: &[crate::types::ContentBlock]) {
        if let Some(session_store) = &self.session_store {
            let text: String = blocks.iter().filter_map(|b| b.as_text()).take(1).collect();
            let title = normalize_session_title(&text);
            if !title.is_empty() {
                match session_store.update_title(&self.id, &title).await {
                    Ok(()) => {
                        if let Some(ref bus) = self.event_bus {
                            let _ = bus.try_send(Event::System(SystemEvent::TitleUpdated {
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

    /// Record the current time as the last activity timestamp.
    pub fn touch(&self) {
        self.last_activity_at.store(now_epoch(), Ordering::Relaxed);
    }

    /// Seconds since the last recorded activity.
    pub fn idle_seconds(&self) -> u64 {
        now_epoch().saturating_sub(self.last_activity_at.load(Ordering::Relaxed))
    }

    /// Whether the main agent is currently streaming
    pub fn is_streaming(&self) -> bool {
        self.main_agent
            .as_ref()
            .is_some_and(|h| h.state() == AgentState::Streaming)
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
            Some(handle) => handle
                .rewind(message_id, target)
                .await
                .map_err(|e| KernelError::from(SessionError::RewindFailed(e.to_string()))),
            None => Err(SessionError::NotInitialized.into()),
        }
    }

    /// Emit `TitleUpdated` event if event sender is configured.
    pub(crate) fn emit_title_updated(&self, title: &str) {
        self.emit_system(SystemEvent::TitleUpdated {
            session_id: self.id.clone(),
            title: title.to_string(),
        });
    }

    fn emit_system(&self, event: SystemEvent) {
        if let Some(ref bus) = self.event_bus {
            let _ = bus.try_send(Event::System(event));
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
        .or_else(|| Some(config.data_dir.join("workspace")))
}
