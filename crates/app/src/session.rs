use anyhow::Result;
use kernel::types::{AgentId, SessionId};
use kernel::{
    agent::{Agent, AgentConfig, AgentHandle, AgentShared, AgentState},
    event::Event,
    storage::Storage,
};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct Session {
    id: SessionId,
    config: SessionConfig,
    storage: Arc<dyn Storage>,
    agent_shared: Arc<AgentShared>,
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
        agent_shared: Arc<AgentShared>,
    ) -> Self {
        Self {
            id,
            config,
            storage,
            agent_shared,
            main_agent: None,
            event_rx: None,
        }
    }

    pub async fn init(&mut self) -> Result<()> {
        self.spawn_main_agent().await?;
        Ok(())
    }

    async fn spawn_main_agent(&mut self) -> Result<()> {
        // Load history from storage
        let history = self
            .storage
            .get_messages(&self.id)
            .await
            .unwrap_or_default();

        let (handle, event_rx) = Agent::spawn(
            AgentId::new(),
            &self.agent_shared,
            &self.config.agent.system_prompt, // Base prompt
            self.config.agent.skills.clone(), // Skills (Agent will build system prompt)
            history,
            Some(self.storage.clone()),
            Some(self.id.0.clone()),
            self.config.agent.max_iterations,
            self.config.agent.enable_sub_agents,
        );
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
                let result = handle.send_text(content).await;
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
