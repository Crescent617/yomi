use crate::provider::ModelConfig;
use crate::skill::Skill;
use crate::storage::StorageConfig;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub model: ModelConfig,
    pub storage: StorageConfig,
    pub max_iterations: usize,
    pub enable_sub_agents: bool,
    pub system_prompt: String,
    #[serde(skip)]
    pub skills: Vec<Arc<Skill>>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig::default(),
            storage: StorageConfig::default(),
            max_iterations: 50,
            enable_sub_agents: true,
            system_prompt: "You are a helpful AI coding assistant.".to_string(),
            skills: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubAgentMode {
    Async,
    Sync,
}

impl std::fmt::Display for SubAgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Async => write!(f, "async"),
            Self::Sync => write!(f, "sync"),
        }
    }
}
