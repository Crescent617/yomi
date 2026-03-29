use crate::session::{Session, SessionConfig};
use anyhow::Result;
use nekoclaw_core::{
    bus::EventBus,
    provider::ModelProvider,
    storage::Storage,
    tool::{ToolRegistry, ToolSandbox},
};
use nekoclaw_shared::types::SessionId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct Coordinator {
    event_bus: EventBus,
    storage: Arc<dyn Storage>,
    provider: Arc<dyn ModelProvider>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    sessions: RwLock<HashMap<SessionId, Arc<RwLock<Session>>>>,
}

impl Coordinator {
    pub fn new(
        event_bus: EventBus,
        storage: Arc<dyn Storage>,
        provider: Arc<dyn ModelProvider>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
    ) -> Self {
        Self {
            event_bus,
            storage,
            provider,
            tool_registry,
            sandbox,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    pub async fn create_session(
        &self, config: SessionConfig
    ) -> Result<SessionId> {
        let id = SessionId::new();
        let mut session = Session::new(
            id.clone(),
            config,
            self.event_bus.clone(),
            self.storage.clone(),
            self.provider.clone(),
            self.tool_registry.clone(),
            self.sandbox.clone(),
        );
        session.init().await?;
        let session_id = session.id().clone();
        self.sessions.write().await
            .insert(session_id.clone(), Arc::new(RwLock::new(session)));
        tracing::info!("Session {} created", session_id.0);
        Ok(session_id)
    }

    pub async fn get_session(
        &self, id: &SessionId
    ) -> Option<Arc<RwLock<Session>>> {
        self.sessions.read().await.get(id).cloned()
    }

    pub async fn list_sessions(&self
    ) -> Vec<SessionId> {
        self.sessions.read().await.keys().cloned().collect()
    }

    pub async fn send_message(
        &self, session_id: &SessionId, content: String
    ) -> Result<()> {
        let session = self.get_session(session_id).await
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id.0))?;
        let result = session.read().await.send_message(content).await;
        result
    }

    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }
}
