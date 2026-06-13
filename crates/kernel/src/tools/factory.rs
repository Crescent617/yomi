//! Tool registry factory for creating pre-configured tool registries.
//!
//! This module provides a factory for creating tool registries without depending
//! on the Agent type, avoiding circular dependencies.

use crate::agent::AgentInput;
use crate::event::Event;
use crate::tools::{
    AskUserTool, EditTool, GlobTool, GrepTool, ReadTool, ReminderTool, ShellTool, ShellToolCtx,
    SleepTool, SubagentTool, ToolRegistry, UpdateGoalTool, WebFetchTool, WebSearchTool, WriteTool,
};
use crate::tools::helper::file_state::FileStateStore;
use crate::types::AgentId;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Feature flags for tool registry configuration.
#[derive(Debug, Clone, Copy, Default)]
pub struct ToolFlags {
    pub sub_agents: bool,
    pub reminder: bool,
    pub sleep: bool,
    pub update_goal: bool,
}

/// Configuration for creating a tool registry.
pub struct ToolRegistryConfig<'a> {
    pub agent_id: &'a AgentId,
    pub shared: &'a Arc<crate::agent::AgentShared>,
    pub event_tx: &'a mpsc::Sender<Event>,
    pub session_id: &'a str,
    pub input_tx: Option<&'a mpsc::Sender<AgentInput>>,
    pub file_state_store: Option<Arc<crate::tools::helper::file_state::FileStateStore>>,
    pub flags: ToolFlags,
    pub ask_user_state: Option<crate::tools::AskUserState>,
    pub tool_blocklist: Vec<String>,
}

impl<'a> ToolRegistryConfig<'a> {
    /// Create config for a main agent.
    pub fn for_main_agent(
        agent_id: &'a AgentId,
        shared: &'a Arc<crate::agent::AgentShared>,
        input_tx: &'a mpsc::Sender<AgentInput>,
        event_tx: &'a mpsc::Sender<Event>,
        session_id: &'a str,
    ) -> Self {
        Self {
            agent_id,
            shared,
            event_tx,
            session_id,
            input_tx: Some(input_tx),
            file_state_store: None,
            flags: ToolFlags::default(),
            ask_user_state: None,
            tool_blocklist: shared.tool_blocklist.clone(),
        }
    }

    /// Create config for a subagent.
    pub fn for_subagent(
        agent_id: &'a AgentId,
        shared: &'a Arc<crate::agent::AgentShared>,
        event_tx: &'a mpsc::Sender<Event>,
        session_id: &'a str,
    ) -> Self {
        Self {
            agent_id,
            shared,
            event_tx,
            session_id,
            input_tx: None,
            file_state_store: None,
            flags: ToolFlags::default(),
            ask_user_state: None,
            tool_blocklist: shared.tool_blocklist.clone(),
        }
    }

    /// Set whether to enable subagents.
    #[must_use]
    pub fn with_enable_sub_agents(mut self, enable: bool) -> Self {
        self.flags.sub_agents = enable;
        self
    }

    /// Set the file state store.
    #[must_use]
    pub fn with_file_state_store(
        mut self,
        store: Option<Arc<FileStateStore>>,
    ) -> Self {
        self.file_state_store = store;
        self
    }

    /// Set the ask-user state.
    #[must_use]
    pub fn with_ask_user_state(mut self, state: crate::tools::AskUserState) -> Self {
        self.ask_user_state = Some(state);
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
            .unwrap_or_else(|| Arc::new(FileStateStore::new()));

        // Register Bash tool
        let bash_ctx = ShellToolCtx::new(config.input_tx.cloned());
        let bash_tool = ShellTool::new().with_ctx(bash_ctx);
        registry.register(bash_tool);

        // Register Read tool with file state store
        let read_tool = ReadTool::new(Arc::clone(&file_state_store));
        registry.register(read_tool);

        // Register Edit tool with file state store
        let edit_tool = EditTool::new(Arc::clone(&file_state_store));
        registry.register(edit_tool);

        // Register Write tool with file state store
        let write_tool = WriteTool::new(Arc::clone(&file_state_store));
        registry.register(write_tool);

        // Register Glob tool
        registry.register(GlobTool::new());

        // Register Grep tool with file state store
        let grep_tool = GrepTool::new(Arc::clone(&file_state_store));
        registry.register(grep_tool);

        // Register WebFetch tool
        registry.register(WebFetchTool::new());

        // Register WebSearch tool
        registry.register(WebSearchTool::new());

        // Register SubAgent tool if enabled
        if config.flags.sub_agents {
            if let Some(tx) = config.input_tx {
                let subagent_tool = SubagentTool::new(
                    config.agent_id.clone(),
                    Arc::clone(config.shared),
                    tx.clone(),
                    config.shared.session_store.clone(),
                    config.session_id.to_owned(),
                    config.event_tx.clone(),
                );
                registry.register(subagent_tool);
            } else {
                tracing::warn!("SubAgent tool enabled but input_tx not provided; skipping registration");
            }
        }

        // Register todo tool
        if let Some(todo_storage) = config.shared.todo_storage.clone() {
            registry.register_todo_tool(todo_storage);
        }

        // Register Reminder tool if enabled (main agent only)
        if config.flags.reminder {
            if let Some(tx) = config.input_tx {
                registry.register(ReminderTool::new(tx.clone()));
            }
        }

        // Register update_goal tool if goal store is available
        if config.flags.update_goal {
            if let Some(ref store) = config.shared.goal_store {
                registry.register(UpdateGoalTool::new(Arc::clone(store)));
            }
        }

        // Register Sleep tool if enabled
        if config.flags.sleep {
            registry.register(SleepTool::new());
        }

        // Register ask_user tool if state is provided
        if let Some(ask_user_state) = config.ask_user_state {
            registry.register(AskUserTool::new(
                config.agent_id.clone(),
                config.event_tx.clone(),
                ask_user_state,
            ));
        }

        // Apply tool blocklist (regex patterns) — remove matching tools from the registry
        if !config.tool_blocklist.is_empty() {
            if let Ok(set) = regex::RegexSet::new(&config.tool_blocklist) {
                let to_remove: Vec<String> = registry
                    .list()
                    .into_iter()
                    .filter(|name| set.is_match(name))
                    .collect();
                for name in &to_remove {
                    registry.remove(name);
                    tracing::info!("Tool '{}' blocked by blocklist pattern", name);
                }
            } else {
                tracing::warn!(
                    "Invalid regex in tool_blocklist: {:?}",
                    config.tool_blocklist
                );
            }
        }

        registry
    }
}
