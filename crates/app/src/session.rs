use anyhow::Result;
use kernel::{
    agent::{Agent, AgentConfig, AgentHandle, AgentState},
    event::Event,
    provider::ModelProvider,
    storage::Storage,
    tool::{ToolRegistry, ToolSandbox},
};
use kernel::types::{SessionId, AgentId};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Session {
    id: SessionId,
    config: SessionConfig,
    storage: Arc<dyn Storage>,
    provider: Arc<dyn ModelProvider>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    main_agent: Option<AgentHandle>,
    event_rx: Option<mpsc::Receiver<Event>>,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub agent: AgentConfig,
    pub project_path: std::path::PathBuf,
}

impl Session {
    pub fn new(
        id: SessionId,
        config: SessionConfig,
        storage: Arc<dyn Storage>,
        provider: Arc<dyn ModelProvider>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
    ) -> Self {
        Self {
            id,
            config,
            storage,
            provider,
            tool_registry,
            sandbox,
            main_agent: None,
            event_rx: None,
        }
    }

    pub async fn init(&mut self) -> Result<()> {
        self.storage.create_session(&self.config.project_path).await?;
        self.spawn_main_agent().await?;
        Ok(())
    }

    async fn spawn_main_agent(&mut self) -> Result<()> {
        let (handle, event_rx) = Agent::spawn(
            AgentId::new(),
            self.config.agent.clone(),
            self.provider.clone(),
            self.tool_registry.clone(),
            self.sandbox.clone(),
            Some(self.storage.clone()),
            Some(self.id.0.clone()),
        );
        let agent_id = handle.id.clone();
        tracing::info!("Main agent {} spawned for session {}", agent_id.0, self.id.0);
        self.main_agent = Some(handle);
        self.event_rx = Some(event_rx);
        Ok(())
    }

    pub async fn send_message(&self, content: String) -> Result<()> {
        tracing::debug!("Session {} sending message ({} bytes)", self.id.0, content.len());
        match &self.main_agent {
            Some(handle) => {
                let result = handle.send_message(content).await;
                if let Err(ref e) = result {
                    tracing::error!("Session {} failed to send message: {}", self.id.0, e);
                }
                result
            }
            None => Err(anyhow::anyhow!("Session not initialized")),
        }
    }

    pub fn cancel(&self) {
        if let Some(handle) = &self.main_agent {
            tracing::info!("Cancelling session {}", self.id.0);
            handle.cancel();
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
}
