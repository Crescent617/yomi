//! Tool registry factory for creating pre-configured tool registries.
//!
//! This module provides a factory for creating tool registries without depending
//! on the Agent type, avoiding circular dependencies.

use crate::agent::AgentInput;
use crate::event::Event;
use crate::skill::Skill;
use crate::tools::{
    EditTool, GlobTool, GrepTool, ReadTool, ShellTool, ShellToolCtx, SkillTool, SubagentTool,
    ToolRegistry, WebFetchTool, WebSearchTool, WriteTool,
};
use crate::types::AgentId;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::mpsc;

/// Factory for creating tool registries with standard configuration.
///
/// This factory decouples tool registry creation from the Agent type,
/// allowing `SubagentTool` and other components to create registries
/// without circular dependencies.
pub struct ToolRegistryFactory;

impl ToolRegistryFactory {
    /// Create a tool registry with standard tools.
    ///
    /// # Arguments
    /// * `agent_id` - The agent ID for tool context
    /// * `shared` - Shared agent resources
    /// * `working_dir` - Working directory for file-based tools
    /// * `input_tx` - Optional input sender for async bash tool results
    /// * `event_tx` - Event sender for permission requests and progress
    /// * `skills` - Skills to register
    /// * `session_id` - Session ID for transcript recording
    /// * `parent_session_id` - Parent session ID for task store sharing (subagents)
    /// * `enable_sub_agents` - Whether to enable the subagent tool
    /// * `skill_folders` - Folders to search for skills (for `skill_load` tool)
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        agent_id: &AgentId,
        shared: &Arc<crate::agent::AgentShared>,
        working_dir: &Path,
        input_tx: Option<&mpsc::Sender<AgentInput>>,
        event_tx: &mpsc::Sender<Event>,
        skills: Vec<Arc<Skill>>,
        session_id: &str,
        parent_session_id: Option<&str>,
        enable_sub_agents: bool,
        skill_folders: Vec<std::path::PathBuf>,
    ) -> ToolRegistry {
        let mut registry = ToolRegistry::new();
        let file_state_store = Arc::new(crate::tools::file_state::FileStateStore::new());

        // Register Bash tool
        let bash_ctx = ShellToolCtx::new(
            agent_id.clone(),
            input_tx.cloned(),
            working_dir.to_path_buf(),
        );
        let bash_tool = ShellTool::new(working_dir).with_ctx(bash_ctx);
        registry.register(bash_tool);

        // Register Read tool with file state store
        let read_tool =
            ReadTool::new(working_dir).with_file_state_store(Arc::clone(&file_state_store));
        registry.register(read_tool);

        // Register Edit tool with file state store
        let edit_tool =
            EditTool::new(working_dir).with_file_state_store(Arc::clone(&file_state_store));
        registry.register(edit_tool);

        // Register Write tool with file state store
        let write_tool =
            WriteTool::new(working_dir).with_file_state_store(Arc::clone(&file_state_store));
        registry.register(write_tool);

        // Register Glob tool
        let glob_tool = GlobTool::new(working_dir);
        registry.register(glob_tool);

        // Register Grep tool
        let grep_tool = GrepTool::new(working_dir);
        registry.register(grep_tool);

        // Register WebFetch tool
        let webfetch_tool = WebFetchTool::new();
        registry.register(webfetch_tool);

        // Register WebSearch tool
        let websearch_tool = WebSearchTool::new();
        registry.register(websearch_tool);

        // Register SkillLoad tool
        let skill_load_tool = SkillTool::new(skill_folders);
        registry.register(skill_load_tool);

        // Register SubAgent tool if enabled
        if enable_sub_agents {
            let subagent_tool = SubagentTool::new(
                agent_id.clone(),
                Arc::clone(shared),
                input_tx.cloned().unwrap(),
                skills,
                shared.storage.clone(),
                working_dir.to_path_buf(),
                session_id.to_owned(),
                event_tx.clone(),
            );
            registry.register(subagent_tool);
        }

        // Register task tools if task_store is provided
        if let Some(task_store) = &shared.task_store {
            // Use parent_session_id for task store if available (subagents share parent's task list)
            let task_list_id = parent_session_id.unwrap_or(session_id).to_owned();
            registry.register_task_tools(task_store.clone(), task_list_id);
        }

        registry
    }
}
