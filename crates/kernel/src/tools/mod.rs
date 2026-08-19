use crate::types::{Result, ToolDefinition, ToolOutput};
use async_trait::async_trait;
use futures::future::Either;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub mod ask_user;
pub mod cron;
pub mod edit;
pub mod executor;
pub mod glob;
pub mod grep;
pub mod helper;
pub mod post_message;
pub mod read;
pub mod reminder;
pub mod shell;
pub mod skill_load;
pub mod sleep;
pub mod subagent;
pub mod task;
pub mod todo;
pub mod update_goal;
pub mod webfetch;
pub mod websearch;
pub mod write;

// Re-export from helper module
pub use helper::{FileStateStore, DEFAULT_MAX_TOOL_OUTPUT_LENGTH, MAX_FILE_SIZE};

// Re-export from executor directly
pub use executor::{build_tool_result, ToolExecutionResult};

pub use ask_user::{AskOption, AskQuestion, AskUserResponse, AskUserTool, ASK_USER_TOOL_NAME};
pub use cron::{CronTool, CRON_TOOL_NAME};
pub use edit::{EditTool, EDIT_TOOL_NAME};
pub use glob::{GlobTool, GLOB_TOOL_NAME};
pub use grep::{GrepTool, GREP_TOOL_NAME};
pub use post_message::{PostMessageTool, POST_MESSAGE_TOOL_NAME};
pub use read::{ReadTool, READ_TOOL_NAME};
pub use reminder::{ReminderTool, REMINDER_TOOL_NAME};
pub use shell::{ShellTool, ShellToolCtx, SHELL_TOOL_NAME};
pub use skill_load::{SkillTool, SKILL_FILENAME, SKILL_TOOL_NAME};
pub use sleep::{SleepTool, SLEEP_TOOL_NAME};
pub use subagent::{SubagentTool, SUBAGENT_TOOL_NAME};
pub use todo::{TodoTool, TODO_TOOL_NAME};
pub use update_goal::{UpdateGoalTool, UPDATE_GOAL_TOOL_NAME};
pub use webfetch::{WebFetchTool, WEBFETCH_TOOL_NAME};
pub use websearch::{WebSearchTool, WEBSEARCH_TOOL_NAME};
pub use write::{WriteTool, WRITE_TOOL_NAME};

/// Guidance for tools that launch async/background tasks.
/// Tells the agent to end its turn immediately and wait for the result notification.
pub const ASYNC_LAUNCH_GUIDE: &str = "After launching, end your current turn immediately — do not sleep, poll, or re-launch. The result will be sent automatically when complete.";

/// Prefix content sent from one agent so receivers can identify and reply to it.
pub(crate) fn format_agent_message(
    agent_id: impl std::fmt::Display,
    content: impl std::fmt::Display,
) -> String {
    format!("[From Agent: {agent_id}] {content}")
}

/// Prefix system output from a background shell task.
pub(crate) fn format_shell_message(
    task_id: impl std::fmt::Display,
    content: impl std::fmt::Display,
) -> String {
    format!("[From Shell: {task_id}] {content}")
}

/// Context provided to tools during execution
pub struct ToolExecCtx<'a> {
    /// The ID of this tool call
    pub tool_call_id: &'a str,
    /// Runtime cancel token for checking cancellation requests (tokio native)
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
    /// Working directory for file-based operations
    pub working_dir: std::path::PathBuf,
    /// Session ID for session-scoped operations (e.g., todo storage)
    pub session_id: String,
    /// Pre-generated `MessageId` for the tool result message, allowing progress
    /// events and the final result to share a consistent identifier.
    pub message_id: crate::types::MessageId,
    /// Current turn for file tracking and checkpointing.
    /// Tools call `ctx.track_edit()` before modifying a file.
    pub turn: Option<std::sync::Arc<crate::agent::Turn>>,
    /// Maximum tool output length in bytes
    pub max_tool_output_length: usize,
}

impl<'a> ToolExecCtx<'a> {
    /// Create a minimal context with just the IDs and working directory.
    pub fn new(
        tool_call_id: &'a str,
        working_dir: impl Into<std::path::PathBuf>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id,
            cancel_token: None,
            working_dir: working_dir.into(),
            session_id: session_id.into(),
            message_id: crate::types::MessageId::default(),
            turn: None,
            max_tool_output_length: 40_000,
        }
    }

    #[must_use]
    pub fn with_cancel_token(mut self, token: Option<tokio_util::sync::CancellationToken>) -> Self {
        self.cancel_token = token;
        self
    }

    /// Check if cancellation has been requested
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.as_ref().is_some_and(|t| t.is_cancelled())
    }

    /// Get a future that completes when cancellation is requested
    pub fn cancelled(&self) -> impl std::future::Future<Output = ()> + 'static {
        match self.cancel_token.clone() {
            Some(token) => Either::Left(async move { token.cancelled().await }),
            None => {
                // If no token, never complete (always pending)
                Either::Right(std::future::pending())
            }
        }
    }

    /// Track a file edit (backup current state BEFORE modifying).
    /// Must be called BEFORE the file is modified.
    pub async fn track_edit(&self, path: &std::path::Path) {
        if let Some(ref turn) = self.turn {
            if let Err(e) = turn.track_file(path).await {
                tracing::warn!("Failed to track file {}: {}", path.display(), e);
            }
        }
    }
}

/// Core trait for tools
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn desc(&self) -> &str;
    fn schema(&self) -> Value;

    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput>;
}

/// Tool registry - manages available tools for an agent
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    /// Cached tool definitions - each wrapped in Arc for cheap cloning
    cached_definitions: Option<Vec<Arc<ToolDefinition>>>,
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
            // Clone the cached Arc definitions - cheap since they're wrapped in Arc
            cached_definitions: self.cached_definitions.clone(),
        }
    }
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
            cached_definitions: None,
        }
    }

    /// Register a tool (mutable because registry is built during agent initialization)
    /// Invalidates the cached definitions since tools have changed
    pub fn register(&mut self, tool: impl Tool + 'static) {
        let name = tool.name().to_string();
        self.tools.insert(name, Arc::new(tool));
        // Invalidate cache since tools have changed
        self.cached_definitions = None;
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Remove a tool by name, returning it if it existed.
    /// Invalidates the cached definitions.
    pub fn remove(&mut self, name: &str) -> Option<Arc<dyn Tool>> {
        let removed = self.tools.remove(name);
        if removed.is_some() {
            self.cached_definitions = None;
        }
        removed
    }

    /// Returns tool definitions wrapped in Arc for cheap cloning.
    /// Cache is computed once since tools are static after registration.
    pub fn definitions(&mut self) -> Vec<Arc<ToolDefinition>> {
        // Check if cache is populated
        if let Some(cached) = &self.cached_definitions {
            tracing::debug!(
                "ToolRegistry.definitions() returning {} cached tools",
                cached.len()
            );
            return cached.clone();
        }

        // Compute definitions, wrap each in Arc
        let defs: Vec<Arc<ToolDefinition>> = self
            .tools
            .values()
            .map(|tool| {
                // Calculate once when the registry builds its cached definitions;
                // requests clone the Arc values and reuse the estimate.
                let mut definition = ToolDefinition {
                    name: tool.name().to_string(),
                    description: tool.desc().to_string(),
                    parameters: tool.schema(),
                    estimated_tokens: 0,
                };
                definition.estimated_tokens = definition.estimated_tokens();
                Arc::new(definition)
            })
            .collect();

        tracing::debug!(
            "ToolRegistry.definitions() computed and cached {} tools: {:?}",
            defs.len(),
            defs.iter().map(|d| &d.name).collect::<Vec<_>>()
        );

        // Cache for future calls
        self.cached_definitions = Some(defs.clone());
        defs
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
}

impl ToolRegistry {
    /// Register todo tool for task tracking
    pub fn register_todo_tool(&mut self, storage: std::sync::Arc<dyn crate::storage::TodoStore>) {
        self.register(TodoTool::new(storage));
    }

    /// Register standard tools with the given configuration.
    #[must_use]
    pub fn with_standard_tools(mut self, config: ToolRegistryConfig) -> Self {
        let file_state_store = config
            .file_state_store
            .unwrap_or_else(|| Arc::new(FileStateStore::new()));

        // Register Bash tool
        let bash_ctx = ShellToolCtx::new(
            config.input_bus.cloned(),
            Arc::clone(&config.shared.background_tasks),
        );
        let bash_tool = ShellTool::new().with_ctx(bash_ctx);
        self.register(bash_tool);

        // Register Read tool with file state store
        let read_tool = ReadTool::new(Arc::clone(&file_state_store));
        self.register(read_tool);

        // Register Edit tool with file state store
        let edit_tool = EditTool::new(Arc::clone(&file_state_store));
        self.register(edit_tool);

        // Register Write tool with file state store
        let write_tool = WriteTool::new(Arc::clone(&file_state_store));
        self.register(write_tool);

        // Register Glob tool
        self.register(GlobTool::new());

        // Register Grep tool with file state store
        let grep_tool = GrepTool::new(Arc::clone(&file_state_store));
        self.register(grep_tool);

        // Register WebFetch tool
        self.register(WebFetchTool::new());

        // Register WebSearch tool
        self.register(WebSearchTool::new());

        // Register SubAgent tool if enabled and this is not a sub-agent session
        let session_id = crate::types::SessionId::from(config.session_id);
        if config.flags.subagent && !session_id.starts_with(crate::types::SUB_PREFIX) {
            if let Some(bus) = config.input_bus {
                let subagent_tool =
                    SubagentTool::new(Arc::clone(config.shared), bus.clone(), session_id);
                self.register(subagent_tool);
            } else {
                tracing::warn!(
                    "SubAgent tool enabled but input_bus not provided; skipping registration"
                );
            }
        }

        // Register post_message for agent-to-agent messages.
        if let Some(input_bus) = config.input_bus {
            self.register(PostMessageTool::new(
                Arc::clone(input_bus),
                config.shared.session_store.clone(),
            ));
        }

        // Register todo tool if enabled
        if config.flags.todo {
            if let Some(todo_storage) = config.shared.todo_storage.clone() {
                self.register_todo_tool(todo_storage);
            }
        }

        // Register Reminder tool if enabled (main agent only)
        if config.flags.reminder {
            if let Some(bus) = config.input_bus {
                self.register(ReminderTool::new(bus.clone()));
            }
        }

        // Register goal tool if goal store is available
        if config.flags.goal {
            if let Some(ref store) = config.shared.goal_store {
                self.register(UpdateGoalTool::new(Arc::clone(store)));
            }
        }

        // Register cron tool if enabled and the cron store is available
        if config.flags.cron {
            if let Some(store) = config.shared.cron_store.clone() {
                self.register(CronTool::new(
                    store,
                    Arc::clone(&config.shared.cron_scheduler),
                    config.shared.session_store.clone(),
                    config.input_bus.cloned(),
                    config.shared.config_auto_approve,
                ));
            } else {
                tracing::warn!("Cron tool enabled but cron store not configured; skipping");
            }
        }

        // ask_user 整体下线（2026-08）：交互价值不抵问题（多题聚合、
        // 长 label 截断、自定义文本三缺陷），所有端一律不注册。工具
        // 本体、AskUserQuestion/Ack 事件、channel 决策卡渲染与 RPC
        // 应答面全部保留（不接线），恢复时只需还原本段注册；聚合/文
        // 本回答修复存于分支 ask-user-fix-wip。

        // Apply tool blocklist (regex patterns) — remove matching tools from the registry.
        // Patterns compile individually: a bad entry is skipped with a warn,
        // never drops the whole blocklist (which includes the sub-agent
        // ask_user guard).
        if !config.tool_blocklist.is_empty() {
            let patterns: Vec<regex::Regex> = config
                .tool_blocklist
                .iter()
                .filter_map(
                    |p| match regex::RegexBuilder::new(p).case_insensitive(true).build() {
                        Ok(re) => Some(re),
                        Err(e) => {
                            tracing::warn!("invalid tool_blocklist pattern '{p}': {e}");
                            None
                        }
                    },
                )
                .collect();
            let to_remove: Vec<String> = self
                .list()
                .into_iter()
                .filter(|name| patterns.iter().any(|re| re.is_match(name)))
                .collect();
            for name in &to_remove {
                self.remove(name);
                tracing::info!("Tool '{}' blocked by blocklist pattern", name);
            }
        }

        self
    }
}

/// Feature flags for tool registration.
#[derive(Debug, Default, Clone, Copy)]
#[allow(clippy::struct_excessive_bools)]
pub struct ToolFlags {
    /// Enable subagent tool for spawning child agents.
    pub subagent: bool,
    /// Enable reminder tool for sending notifications.
    pub reminder: bool,
    /// Enable goal tracking tool.
    pub goal: bool,
    /// Enable cron tool for managing scheduled jobs.
    pub cron: bool,
    /// Enable todo tool for agent task tracking.
    pub todo: bool,
}

impl ToolFlags {
    /// Create flags for an agent, enabling subagent and reminder by default.
    pub fn new(enable_subagent: bool) -> Self {
        Self {
            subagent: enable_subagent,
            reminder: false,
            goal: true,
            cron: false,
            todo: false,
        }
    }

    /// Set the cron tool flag.
    #[must_use]
    pub const fn with_cron(mut self, enabled: bool) -> Self {
        self.cron = enabled;
        self
    }

    /// Set the goal tool flag.
    #[must_use]
    pub const fn with_goal(mut self, enabled: bool) -> Self {
        self.goal = enabled;
        self
    }

    /// Set the todo tool flag.
    #[must_use]
    pub const fn with_todo(mut self, enabled: bool) -> Self {
        self.todo = enabled;
        self
    }
}

/// Configuration for creating a tool registry.
pub struct ToolRegistryConfig<'a> {
    pub shared: &'a Arc<crate::agent::AgentShared>,
    pub event_bus: &'a crate::comms::EventBusHandle,
    pub session_id: &'a str,
    pub input_bus: Option<&'a Arc<crate::comms::InputBus>>,
    pub file_state_store: Option<Arc<FileStateStore>>,
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
