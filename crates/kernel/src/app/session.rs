use crate::permissions::{Level, PermissionState};
use crate::storage::file_state::JsonlFileStateStore;
use crate::storage::{MessageStore, SessionStore};
use crate::types::{AgentId, KernelError, Result, SessionId};
use crate::{
    agent::{Agent, AgentConfig, AgentHandle, AgentShared, AgentSpawnArgs, AgentState},
    event::Event,
};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Session {
    id: SessionId,
    config: SessionConfig,
    #[allow(dead_code)]
    session_store: Arc<dyn SessionStore>,
    message_store: Arc<dyn MessageStore>,
    agent_shared: Arc<AgentShared>,
    main_agent: Option<AgentHandle>,
    event_rx: Option<mpsc::Receiver<Event>>,
    /// Shared permission state for runtime level updates
    permission_state: Option<PermissionState>,
    /// File state store for tracking file modification times
    file_state_store: Arc<crate::tools::file_state::FileStateStore>,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub agent: AgentConfig,
    pub project_path: std::path::PathBuf,
    pub auto_approve_level: Level,
    pub data_dir: std::path::PathBuf,
}

impl Session {
    pub async fn new(
        id: SessionId,
        config: SessionConfig,
        session_store: Arc<dyn SessionStore>,
        message_store: Arc<dyn MessageStore>,
        agent_shared: Arc<AgentShared>,
    ) -> Result<Self> {
        // Create persistent file state store
        let persistent_store: Arc<dyn crate::storage::FileStateStore> =
            Arc::new(JsonlFileStateStore::new(&id.0, &config.data_dir).await?);

        // Load previous file states from disk
        let states = persistent_store.get_all().await?;

        // Create file state store with persistent backend and preload states
        let file_state_store = Arc::new(crate::tools::file_state::FileStateStore::with_persistent(
            persistent_store,
        ));
        for (path, mtime) in states {
            file_state_store.record(path, mtime);
        }

        Ok(Self {
            id,
            config,
            session_store,
            message_store,
            agent_shared,
            main_agent: None,
            event_rx: None,
            permission_state: None,
            file_state_store,
        })
    }

    pub async fn init(&mut self) -> Result<()> {
        self.spawn_main_agent().await?;
        Ok(())
    }

    async fn spawn_main_agent(&mut self) -> Result<()> {
        // Load history from storage
        let history = self.message_store.get(&self.id.0).await.unwrap_or_default();

        // Create shared permission state for all agents in this session
        if self.permission_state.is_none() && self.config.auto_approve_level != Level::Dangerous {
            let ps = PermissionState::new(self.config.auto_approve_level).0;
            self.permission_state = Some(ps);
        }
        let permission_state = self.permission_state.clone();

        let config =
            AgentSpawnArgs::new(self.config.agent.system_prompt.clone(), self.id.0.clone())
                .with_skills(self.config.agent.skills.clone())
                .with_history(history)
                .with_max_iterations(self.config.agent.max_iterations)
                .with_working_dir(self.config.project_path.clone())
                .with_subagent(self.config.agent.enable_subagent)
                .with_file_state_store(Arc::clone(&self.file_state_store));

        // Create AgentShared with permission state and file state store
        let shared = Arc::new(AgentShared::new(
            self.agent_shared.provider.clone(),
            self.agent_shared.model_config.clone(),
            self.agent_shared.task_store.clone(),
            self.agent_shared.todo_storage.clone(),
            self.agent_shared.project_memory.clone(),
            self.agent_shared.compactor.clone(),
            self.agent_shared.session_store.clone(),
            self.agent_shared.message_store.clone(),
            self.agent_shared.usage_store.clone(),
            permission_state,
            self.agent_shared.skill_folders.clone(),
            Some(Arc::clone(&self.file_state_store)),
        ));

        let (handle, event_rx) = Agent::spawn(AgentId::new(), &shared, config);
        let agent_id = handle.id.clone();
        tracing::info!("Main agent {} spawned for session {}", agent_id, self.id.0);
        self.main_agent = Some(handle);
        self.event_rx = Some(event_rx);
        Ok(())
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

    pub const fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<Event>> {
        self.event_rx.take()
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
