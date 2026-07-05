//! Tool registry for creating pre-configured tool registries.
//!
//! This module provides a factory for creating tool registries without depending
//! on the Agent type, avoiding circular dependencies.

use crate::comms::EventBusHandle;
use crate::tools::helper::file_state::FileStateStore;
use crate::tools::{
    AskUserTool, EditTool, GlobTool, GrepTool, ReadTool, ReminderTool, ShellTool, ShellToolCtx,
    SleepTool, SubagentTool, ToolRegistry, UpdateGoalTool, WebFetchTool, WebSearchTool, WriteTool,
};
use std::sync::Arc;

/// Feature flags for tool registry configuration.
/// These are independent on/off switches, so a simple struct is intentional.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone, Copy)]
pub struct ToolFlags {
    pub subagent: bool,
    pub reminder: bool,
    pub sleep: bool,
    pub goal: bool,
}

impl ToolFlags {
    pub fn for_agent(enable_subagent: bool) -> Self {
        Self {
            subagent: enable_subagent,
            sleep: true,
            goal: true,
            reminder: false,
        }
    }
}

/// Configuration for creating a tool registry.
pub struct ToolRegistryConfig<'a> {
    pub shared: &'a Arc<crate::agent::AgentShared>,
    pub event_bus: &'a EventBusHandle,
    pub session_id: &'a str,
    pub input_bus: Option<&'a Arc<crate::comms::InputBus>>,
    pub file_state_store: Option<Arc<crate::tools::helper::file_state::FileStateStore>>,
    pub tool_blocklist: Vec<String>,
    pub flags: ToolFlags,
}

impl ToolRegistryConfig<'_> {
    /// Set the file state store.
    #[must_use]
    pub fn with_file_state_store(mut self, store: Option<Arc<FileStateStore>>) -> Self {
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
            .unwrap_or_else(|| Arc::new(FileStateStore::new()));

        // Register Bash tool
        let bash_ctx = ShellToolCtx::new(config.input_bus.cloned());
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

        // Register SubAgent tool if enabled and this is not a sub-agent session
        let session_id = crate::types::SessionId::from(config.session_id);
        if config.flags.subagent && !session_id.starts_with(crate::types::SUB_PREFIX) {
            if let Some(bus) = config.input_bus {
                let subagent_tool =
                    SubagentTool::new(Arc::clone(config.shared), bus.clone(), session_id);
                registry.register(subagent_tool);
            } else {
                tracing::warn!(
                    "SubAgent tool enabled but input_bus not provided; skipping registration"
                );
            }
        }

        // Register todo tool
        if let Some(todo_storage) = config.shared.todo_storage.clone() {
            registry.register_todo_tool(todo_storage);
        }

        // Register Reminder tool if enabled (main agent only)
        if config.flags.reminder {
            if let Some(bus) = config.input_bus {
                registry.register(ReminderTool::new(bus.clone()));
            }
        }

        // Register goal tool if goal store is available
        if config.flags.goal {
            if let Some(ref store) = config.shared.goal_store {
                registry.register(UpdateGoalTool::new(Arc::clone(store)));
            }
        }

        // Register Sleep tool if enabled
        if config.flags.sleep {
            registry.register(SleepTool::new());
        }

        // Register send_message tool if a channel hub is configured
        // if let Some(ref cm) = config.shared.channel_hub {
        //     registry.register(SendMessageTool::new(Arc::clone(cm)));
        // }

        // Register ask_user tool if input_bus is available
        if let Some(input_bus) = config.input_bus {
            registry.register(AskUserTool::new(config.event_bus.clone(), Arc::clone(input_bus)));
        }

        // Apply tool blocklist (regex patterns) — remove matching tools from the registry
        if !config.tool_blocklist.is_empty() {
            if let Ok(set) = regex::RegexSetBuilder::new(&config.tool_blocklist)
                .case_insensitive(true)
                .build()
            {
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
