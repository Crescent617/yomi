use crate::agent::AgentShared;
use crate::app::session::{Session, SessionConfig};
use crate::event::Event;
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
    pub fn new(
        storage: Arc<dyn Storage>,
        provider: Arc<dyn Provider>,
        model_config: ModelConfig,
        task_store: Option<Arc<crate::task::TaskStore>>,
        project_memory: crate::project_memory::MemoryFiles,
        compactor: Option<crate::compactor::Compactor>,
    ) -> Self {
        let agent_shared = Arc::new(AgentShared::new(
            provider,
            Arc::new(model_config),
            task_store,
            Arc::new(project_memory),
            compactor,
            Some(storage.clone()),
        ));
        Self {
            storage,
            agent_shared,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create_session(&self, config: SessionConfig) -> Result<SessionId> {
        let id = self.storage.create_session().await?;
        let mut session = Session::new(
            id.clone(),
            config,
            self.storage.clone(),
            Arc::clone(&self.agent_shared),
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
}
