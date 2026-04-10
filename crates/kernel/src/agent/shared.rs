use crate::provider::ModelConfig;
use crate::tool::ToolRegistry;
use std::sync::Arc;

/// Shared resources across agents in the same session/context.
///
/// This struct contains all resources that can be safely shared between multiple agents,
/// allowing for efficient memory usage when creating many agents. Each agent gets an
/// `Arc<AgentShared>` instead of owning its own copy of these resources.
#[derive(Clone)]
pub struct AgentShared {
    pub provider: Arc<dyn crate::provider::ModelProvider>,
    pub tool_registry: Arc<ToolRegistry>,
    pub model_config: ModelConfig,
}

impl AgentShared {
    /// Create a new `AgentShared` with the required components
    pub fn new(
        provider: Arc<dyn crate::provider::ModelProvider>,
        tool_registry: Arc<ToolRegistry>,
        model_config: ModelConfig,
    ) -> Self {
        Self {
            provider,
            tool_registry,
            model_config,
        }
    }

    /// Clone this `AgentShared` with a new `ToolRegistry` (for adding agent-specific tools)
    pub fn with_cloned_registry(&self) -> Self {
        Self {
            provider: Arc::clone(&self.provider),
            tool_registry: Arc::new(self.tool_registry.as_ref().clone()),
            model_config: self.model_config.clone(),
        }
    }
}
