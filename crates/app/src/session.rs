use anyhow::Result;
use nekoclaw_core::{
    agent::{Agent, AgentConfig},
    bus::EventBus,
    provider::ModelProvider,
    storage::Storage,
    tool::{ToolRegistry, ToolSandbox},
};
use nekoclaw_shared::types::{SessionId, AgentId};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Session {
    id: SessionId,
    config: SessionConfig,
    event_bus: EventBus,
    storage: Arc<dyn Storage>,
    provider: Arc<dyn ModelProvider>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    main_agent: Option<AgentId>,
    input_tx: Option<mpsc::Sender<String>>,
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
        event_bus: EventBus,
        storage: Arc<dyn Storage>,
        provider: Arc<dyn ModelProvider>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
    ) -> Self {
        Self {
            id,
            config,
            event_bus,
            storage,
            provider,
            tool_registry,
            sandbox,
            main_agent: None,
            input_tx: None,
        }
    }

    pub async fn init(&mut self) -> Result<()> {
        self.storage.create_session(&self.config.project_path).await?;
        self.spawn_main_agent().await?;
        Ok(())
    }

    async fn spawn_main_agent(&mut self) -> Result<()> {
        let (input_tx, input_rx) = mpsc::channel(100);
        self.input_tx = Some(input_tx);
        let agent = Agent::new(
            AgentId::new(),
            self.config.agent.clone(),
            self.event_bus.clone(),
            self.provider.clone(),
            self.storage.clone(),
            self.tool_registry.clone(),
            self.sandbox.clone(),
            input_rx,
        );
        let agent_id = agent.id().clone();
        agent.spawn();
        tracing::info!("Main agent {} spawned for session {}", agent_id.0, self.id.0);
        self.main_agent = Some(agent_id);
        Ok(())
    }

    pub async fn send_message(&self, content: String
    ) -> Result<()> {
        if let Some(ref tx) = self.input_tx {
            tx.send(content).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not initialized"))
        }
    }

    pub fn id(&self) -> &SessionId {
        &self.id
    }

    pub fn main_agent_id(&self) -> Option<&AgentId> {
        self.main_agent.as_ref()
    }
}
