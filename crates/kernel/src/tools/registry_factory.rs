//! Tool registry factory for creating pre-configured tool registries.
//!
//! This module provides a factory for creating tool registries without depending
//! on the Agent type, avoiding circular dependencies.

use crate::agent::AgentInput;
use crate::event::Event;
use crate::skill::Skill;
use crate::tools::{
    EditTool, GlobTool, GrepTool, ReadTool, ReminderTool, ShellTool, ShellToolCtx, SkillTool,
    SubagentTool, ToolRegistry, WebFetchTool, WebSearchTool, WriteTool,
};
use crate::types::AgentId;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Configuration for creating a tool registry.
pub struct ToolRegistryConfig<'a> {
    pub agent_id: &'a AgentId,
    pub shared: &'a Arc<crate::agent::AgentShared>,
    pub event_tx: &'a mpsc::Sender<Event>,
    pub skills: Vec<Arc<Skill>>,
    pub session_id: &'a str,
    pub skill_folders: Vec<PathBuf>,
    pub input_tx: Option<&'a mpsc::Sender<AgentInput>>,
    pub parent_session_id: Option<&'a str>,
    pub file_state_store: Option<Arc<crate::tools::file_state::FileStateStore>>,
    pub enable_sub_agents: bool,
    pub enable_reminder: bool,
}

impl<'a> ToolRegistryConfig<'a> {
    /// Create config for a main agent.
    pub fn for_main_agent(
        agent_id: &'a AgentId,
        shared: &'a Arc<crate::agent::AgentShared>,
        input_tx: &'a mpsc::Sender<AgentInput>,
        event_tx: &'a mpsc::Sender<Event>,
        skills: Vec<Arc<Skill>>,
        session_id: &'a str,
    ) -> Self {
        Self {
            agent_id,
            shared,
            event_tx,
            skills,
            session_id,
            skill_folders: shared.skill_folders.clone(),
            input_tx: Some(input_tx),
            parent_session_id: None,
            file_state_store: None,
            enable_sub_agents: true,
            enable_reminder: true,
        }
    }

    /// Create config for a subagent.
    pub fn for_subagent(
        parent_id: &'a AgentId,
        shared: &'a Arc<crate::agent::AgentShared>,
        event_tx: &'a mpsc::Sender<Event>,
        skills: Vec<Arc<Skill>>,
        session_id: &'a str,
        parent_session_id: &'a str,
    ) -> Self {
        Self {
            agent_id: parent_id,
            shared,
            event_tx,
            skills,
            session_id,
            skill_folders: shared.skill_folders.clone(),
            input_tx: None,
            parent_session_id: Some(parent_session_id),
            file_state_store: None,
            enable_sub_agents: false,
            enable_reminder: false,
        }
    }

    /// Set whether to enable subagents.
    #[must_use]
    pub fn with_enable_sub_agents(mut self, enable: bool) -> Self {
        self.enable_sub_agents = enable;
        self
    }

    /// Set the file state store.
    #[must_use]
    pub fn with_file_state_store(
        mut self,
        store: Option<Arc<crate::tools::file_state::FileStateStore>>,
    ) -> Self {
        self.file_state_store = store;
        self
    }
}

/// Factory for creating tool registries with standard configuration.
///
/// This factory decouples tool registry creation from the Agent type,
/// allowing `SubagentTool` and other components to create registries
/// without circular dependencies.
pub struct ToolRegistryFactory;

impl ToolRegistryFactory {
    /// Create a tool registry with standard tools.
    pub fn create(config: ToolRegistryConfig<'_>) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        let file_state_store = config
            .file_state_store
            .unwrap_or_else(|| Arc::new(crate::tools::file_state::FileStateStore::new()));

        // Register Bash tool
        let bash_ctx = ShellToolCtx::new(config.agent_id.clone(), config.input_tx.cloned());
        let bash_tool = ShellTool::new().with_ctx(bash_ctx);
        registry.register(bash_tool);

        // Register Read tool with file state store
        let read_tool = ReadTool::new().with_file_state_store(Arc::clone(&file_state_store));
        registry.register(read_tool);

        // Register Edit tool with file state store
        let edit_tool = EditTool::new().with_file_state_store(Arc::clone(&file_state_store));
        registry.register(edit_tool);

        // Register Write tool with file state store
        let write_tool = WriteTool::new().with_file_state_store(Arc::clone(&file_state_store));
        registry.register(write_tool);

        // Register Glob tool
        registry.register(GlobTool::new());

        // Register Grep tool
        registry.register(GrepTool::new());

        // Register WebFetch tool
        registry.register(WebFetchTool::new());

        // Register WebSearch tool
        registry.register(WebSearchTool::new());

        // Register SkillLoad tool
        registry.register(SkillTool::new(config.skill_folders));

        // Register SubAgent tool if enabled
        if config.enable_sub_agents {
            let subagent_tool = SubagentTool::new(
                config.agent_id.clone(),
                Arc::clone(config.shared),
                config.input_tx.cloned().unwrap(),
                config.skills,
                config.shared.storage.clone(),
                config.session_id.to_owned(),
                config.event_tx.clone(),
            );
            registry.register(subagent_tool);
        }

        // Register task tools if task_store is provided
        if let Some(task_store) = &config.shared.task_store {
            let task_list_id = config
                .parent_session_id
                .unwrap_or(config.session_id)
                .to_owned();
            registry.register_task_tools(task_store.clone(), task_list_id);
        }

        // Register Reminder tool if enabled (main agent only)
        if config.enable_reminder {
            if let Some(tx) = config.input_tx {
                registry.register(ReminderTool::new(tx.clone()));
            }
        }

        registry
    }
}
