use crate::permissions::{Level, PermissionState};
use crate::storage::file_state::JsonlFileStateStore;
use crate::types::{AgentId, KernelError, Result, SessionId};
use crate::{
    agent::{Agent, AgentConfig, AgentHandle, AgentShared, AgentSpawnArgs, AgentState},
    event::Event,
};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Session {
    id: SessionId,
    #[allow(dead_code)]
    config: SessionConfig,
    /// Shared agent resources (contains `session_store`, `message_store`, etc.)
    #[allow(dead_code)]
    agent_shared: Arc<AgentShared>,
    main_agent: Option<AgentHandle>,
    /// Shared permission state for runtime level updates
    permission_state: Option<PermissionState>,
    /// File state store for tracking file modification times
    #[allow(dead_code)]
    file_state_store: Arc<crate::tools::helper::FileStateStore>,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub agent: AgentConfig,
    pub project_path: std::path::PathBuf,
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
        agent_shared: Arc<AgentShared>,
    ) -> Result<(Self, mpsc::Receiver<Event>)> {
        let file_state_store = Self::create_file_state_store(&id, &config).await?;

        let permission_state = Self::create_permission_state(&config);

        let (main_agent, event_rx) = Self::spawn_main_agent(
            &id,
            &config,
            &agent_shared,
            &file_state_store,
            permission_state.clone(),
        )
        .await?;

        let session = Self {
            id,
            config,
            agent_shared,
            main_agent: Some(main_agent),
            permission_state,
            file_state_store,
        };
        Ok((session, event_rx))
    }

    /// Create and populate the file state store for this session
    async fn create_file_state_store(
        id: &SessionId,
        config: &SessionConfig,
    ) -> Result<Arc<crate::tools::helper::FileStateStore>> {
        let persistent_store: Arc<dyn crate::storage::FileStateStore> =
            Arc::new(JsonlFileStateStore::new(&id.0, &config.data_dir).await?);

        let states = persistent_store.read_all().await?;

        let file_state_store = crate::tools::helper::FileStateStore::new()
            .with_persistent(persistent_store)
            .with_states(states.into_iter().map(|fs| (fs.path, fs.mtime)));

        Ok(Arc::new(file_state_store))
    }

    /// Create permission state if needed based on config
    fn create_permission_state(config: &SessionConfig) -> Option<PermissionState> {
        if config.auto_approve_level == Level::Dangerous {
            None
        } else {
            Some(PermissionState::new(config.auto_approve_level).0)
        }
    }

    /// Spawn the main agent for this session
    async fn spawn_main_agent(
        id: &SessionId,
        config: &SessionConfig,
        agent_shared: &Arc<AgentShared>,
        file_state_store: &Arc<crate::tools::helper::FileStateStore>,
        permission_state: Option<PermissionState>,
    ) -> Result<(AgentHandle, mpsc::Receiver<Event>)> {
        let history = agent_shared
            .message_store
            .as_ref()
            .ok_or_else(|| KernelError::session("message store not configured"))?
            .get(&id.0)
            .await
            .unwrap_or_default();

        let spawn_args = AgentSpawnArgs::new(config.agent.system_prompt.clone(), id.0.clone())
            .with_skills(config.agent.skills.clone())
            .with_history(history)
            .with_max_iterations(config.agent.max_iterations)
            .with_working_dir(config.project_path.clone())
            .with_subagent(config.agent.enable_subagent)
            .with_file_state_store(Arc::clone(file_state_store));

        let shared = Arc::new(
            agent_shared.with_per_session(permission_state, Some(Arc::clone(file_state_store))),
        );

        let (handle, event_rx) = Agent::spawn(AgentId::new(), &shared, spawn_args);
        tracing::info!("Main agent {} spawned for session {}", handle.id, id.0);

        Ok((handle, event_rx))
    }

    pub async fn send_message(&self, content: String) -> Result<()> {
        tracing::debug!(
            "Session {} sending message ({} bytes)",
            self.id.0,
            content.len()
        );
        match &self.main_agent {
            Some(handle) => {
                handle.send_text(content).await?;
                Ok(())
            }
            None => Err(KernelError::session("Session not initialized")),
        }
    }

    /// Send a multi-modal message with content blocks
    pub async fn send_blocks(&self, blocks: Vec<crate::types::ContentBlock>) -> Result<()> {
        tracing::debug!(
            "Session {} sending {} content blocks",
            self.id.0,
            blocks.len()
        );
        match &self.main_agent {
            Some(handle) => {
                handle.send_message(blocks).await?;
                Ok(())
            }
            None => Err(KernelError::session("Session not initialized")),
        }
    }

    pub fn cancel(&self) {
        if let Some(handle) = &self.main_agent {
            tracing::info!("Cancelling session {}", self.id.0);
            handle.cancel();
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
                .map_err(|e| {
                    KernelError::session(format!("Failed to send permission response: {e}"))
                }),
            None => Err(KernelError::session("Session not initialized")),
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
            None => Err(KernelError::session("Session not initialized")),
        }
    }
}
