use crate::agent::AgentShared;
use crate::app::session::{Session, SessionConfig};
use crate::event::Event;
use crate::permissions::Level;
use crate::providers::{ModelConfig, Provider};
use crate::storage::Storage;
use crate::types::SessionId;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct Coordinator {
    storage: Arc<dyn Storage>,
    agent_shared: Arc<AgentShared>,
    sessions: RwLock<HashMap<SessionId, Arc<RwLock<Session>>>>,
}

impl Coordinator {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: Arc<dyn Storage>,
        provider: Arc<dyn Provider>,
        model_config: ModelConfig,
        task_store: Option<Arc<crate::task::TaskStore>>,
        todo_storage: Option<Arc<crate::storage::TodoStorage>>,
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
            Some(storage.clone()),
            None, // permission_state is created per-session
            skill_folders,
        ));
        Self {
            storage,
            agent_shared,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create_session(
        &self,
        config: SessionConfig,
        file_state_store: Arc<crate::tools::file_state::FileStateStore>,
    ) -> Result<SessionId> {
        let working_dir = config.project_path.to_string_lossy().to_string();
        let id = self.storage.create_session(Some(&working_dir)).await?;
        let mut session = Session::new(
            id.clone(),
            config,
            self.storage.clone(),
            Arc::clone(&self.agent_shared),
            file_state_store,
        );
        session.init().await?;
        let session_id = session.id().clone();
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::new(RwLock::new(session)));
        tracing::info!("Session {} created", session_id.0);
        Ok(session_id)
    }

    /// Restore a session from storage by its ID
    pub async fn restore_session(
        &self,
        session_id: &SessionId,
        config: SessionConfig,
        file_state_store: Arc<crate::tools::file_state::FileStateStore>,
    ) -> Result<SessionId> {
        // Verify session exists in storage
        let session_record = self
            .storage
            .get_session(session_id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Session not found in storage: {}", session_id.0))?;

        tracing::info!("Restoring session {} from storage", session_id.0);

        let mut session = Session::new(
            session_record.id.clone(),
            config,
            self.storage.clone(),
            Arc::clone(&self.agent_shared),
            file_state_store,
        );
        session.init().await?;

        self.sessions
            .write()
            .await
            .insert(session_record.id.clone(), Arc::new(RwLock::new(session)));
        tracing::info!("Session {} restored", session_record.id.0);
        Ok(session_record.id)
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
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id.0))?;
        let result = session.read().await.send_message(content).await;
        if let Err(ref e) = result {
            tracing::error!("Failed to send message to session {}: {}", session_id.0, e);
        }
        result
    }

    /// Send a multi-modal message with content blocks (supports images, text, etc.)
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
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id.0))?;
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
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id.0))?;
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
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id.0))?;
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
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id.0))?;
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
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id.0))?;
        let result = session.read().await.compact().await;
        if let Err(ref e) = result {
            tracing::error!("Failed to compact session {}: {}", session_id.0, e);
        } else {
            tracing::info!("Compaction requested for session {}", session_id.0);
        }
        result
    }

    /// Get file state snapshot for a session
    pub async fn get_file_state_snapshot(
        &self,
        session_id: &SessionId,
    ) -> Option<crate::tools::file_state::FileStateSnapshot> {
        let session = self.get_session(session_id).await?;
        let snapshot = session.read().await.file_state_snapshot();
        Some(snapshot)
    }

    /// Delete a session from storage
    pub async fn delete_session(&self, session_id: &SessionId) -> Result<()> {
        self.storage.delete_session(session_id).await
    }

    /// Get messages for a session from storage
    pub async fn get_session_messages(
        &self,
        session_id: &SessionId,
    ) -> Result<Vec<crate::types::Message>> {
        self.storage.get_messages(session_id).await
    }

    /// Get storage reference
    pub fn storage(&self) -> &Arc<dyn Storage> {
        &self.storage
    }
}
