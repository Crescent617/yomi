use crate::agent::AgentShared;
use crate::app::session::{Session, SessionConfig};
use crate::event::Event;
use crate::permissions::Level;
use crate::providers::{ModelConfig, Provider};
use crate::storage::{MessageStore, SessionStore};
use crate::types::{KernelError, Result, SessionId};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct Coordinator {
    session_store: Arc<dyn SessionStore>,
    message_store: Arc<dyn MessageStore>,
    agent_shared: Arc<AgentShared>,
    sessions: RwLock<HashMap<SessionId, Arc<RwLock<Session>>>>,
}

impl Coordinator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        session_store: Arc<dyn SessionStore>,
        message_store: Arc<dyn MessageStore>,
        provider: Arc<dyn Provider>,
        model_config: ModelConfig,
        task_store: Option<Arc<crate::task::TaskStore>>,
        todo_storage: Option<Arc<dyn crate::storage::TodoStore>>,
        project_memory: crate::project_memory::MemoryFiles,
        compactor: Option<crate::compactor::Compactor>,
        skill_folders: Vec<std::path::PathBuf>,
    ) -> Self {
        let agent_shared = Arc::new(AgentShared::new(
            provider,
            Arc::new(model_config),
            task_store,
            todo_storage,
            Arc::new(project_memory),
            compactor,
            Some(session_store.clone()),
            Some(message_store.clone()),
            None, // usage_store - TODO: inject from caller
            None,
            skill_folders,
            None,
        ));
        Self {
            session_store,
            message_store,
            agent_shared,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Create a new session with the given configuration
    pub async fn create_session(&self, config: SessionConfig) -> Result<SessionId> {
        let working_dir = config.project_path.to_string_lossy().to_string();
        let id = self.session_store.create(Some(&working_dir)).await?;
        self.init_session(id.clone(), config).await?;
        tracing::info!("Session {} created", id.0);
        Ok(id)
    }

    /// Initialize a session in memory
    async fn init_session(&self, session_id: SessionId, config: SessionConfig) -> Result<()> {
        let mut session = Session::new(
            session_id.clone(),
            config,
            self.session_store.clone(),
            self.message_store.clone(),
            Arc::clone(&self.agent_shared),
        )
        .await?;
        session.init().await?;

        self.sessions
            .write()
            .await
            .insert(session_id, Arc::new(RwLock::new(session)));
        Ok(())
    }

    /// Restore a session from storage by its ID
    pub async fn restore_session(
        &self,
        session_id: &SessionId,
        config: SessionConfig,
    ) -> Result<SessionId> {
        // Verify session exists in storage
        let session_info = self.session_store.get(session_id).await?.ok_or_else(|| {
            KernelError::session(format!("Session not found in storage: {}", session_id.0))
        })?;

        tracing::info!("Restoring session {} from storage", session_id.0);
        self.init_session(session_info.id.clone(), config).await?;
        tracing::info!("Session {} restored", session_info.id.0);
        Ok(session_info.id)
    }

    /// Fork a session: create new session with copied history from parent
    pub async fn fork_session(
        &self,
        parent_id: &SessionId,
        config: SessionConfig,
    ) -> Result<SessionId> {
        // Create new session with copied history in storage
        let new_id = self.session_store.fork(parent_id).await?;
        tracing::info!("Forked session {} from {}", new_id.0, parent_id.0);

        self.init_session(new_id.clone(), config).await?;
        tracing::info!("Forked session {} initialized", new_id.0);
        Ok(new_id)
    }

    pub async fn get_session(&self, id: &SessionId) -> Option<Arc<RwLock<Session>>> {
        self.sessions.read().await.get(id).cloned()
    }

    pub async fn list_sessions(&self) -> Vec<SessionId> {
        self.sessions.read().await.keys().cloned().collect()
    }

    pub async fn send_message(&self, session_id: &SessionId, content: String) -> Result<()> {
        tracing::debug!(
            "Sending message to session {} ({} bytes)",
            session_id.0,
            content.len()
        );
        let session = self
            .get_session(session_id)
            .await
            .ok_or_else(|| KernelError::session(format!("Session not found: {}", session_id.0)))?;
        let result = session.read().await.send_message(content).await;
        if let Err(ref e) = result {
            tracing::error!("Failed to send message to session {}: {}", session_id.0, e);
        }
        result
    }

    /// Send a multi-modal message with content blocks
    pub async fn send_blocks(
        &self,
        session_id: &SessionId,
        blocks: Vec<crate::types::ContentBlock>,
    ) -> Result<()> {
        tracing::debug!(
            "Sending {} content blocks to session {}",
            blocks.len(),
            session_id.0
        );
        let session = self
            .get_session(session_id)
            .await
            .ok_or_else(|| KernelError::session(format!("Session not found: {}", session_id.0)))?;
        let result = session.read().await.send_blocks(blocks).await;
        if let Err(ref e) = result {
            tracing::error!("Failed to send blocks to session {}: {}", session_id.0, e);
        }
        result
    }

    pub async fn take_session_event_receiver(
        &self,
        session_id: &SessionId,
    ) -> Option<tokio::sync::mpsc::Receiver<Event>> {
        let session = self.get_session(session_id).await?;
        let rx = session.write().await.take_event_receiver();
        rx
    }

    pub async fn cancel(&self, session_id: &SessionId) -> Result<()> {
        let session = self
            .get_session(session_id)
            .await
            .ok_or_else(|| KernelError::session(format!("Session not found: {}", session_id.0)))?;
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
        let session = self
            .get_session(session_id)
            .await
            .ok_or_else(|| KernelError::session(format!("Session not found: {}", session_id.0)))?;
        let result = session
            .read()
            .await
            .send_permission_response(req_id, approved, remember)
            .await;
        result
    }

    pub async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()> {
        let session = self
            .get_session(session_id)
            .await
            .ok_or_else(|| KernelError::session(format!("Session not found: {}", session_id.0)))?;
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
        let session = self
            .get_session(session_id)
            .await
            .ok_or_else(|| KernelError::session(format!("Session not found: {}", session_id.0)))?;
        let result = session.read().await.compact().await;
        if let Err(ref e) = result {
            tracing::error!("Failed to compact session {}: {}", session_id.0, e);
        } else {
            tracing::info!("Compaction requested for session {}", session_id.0);
        }
        result
    }

    /// Delete a session from storage
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.session_store.delete(session_id).await
    }

    /// Get messages for a session from storage
    pub async fn get_session_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::Message>> {
        self.message_store.get(&session_id.0).await
    }

    /// Get session store reference
    pub fn session_store(&self) -> &Arc<dyn SessionStore> {
        &self.session_store
    }

    /// Get message store reference
    pub fn message_store(&self) -> &Arc<dyn MessageStore> {
        &self.message_store
    }
}
