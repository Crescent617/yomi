use crate::types::{Result, ToolDefinition, ToolOutput};
use async_trait::async_trait;
use futures::future::Either;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub mod ask_user;
pub mod edit;
pub mod executor;
pub mod factory;
pub mod glob;
pub mod grep;
pub mod helper;
pub mod read;
pub mod reminder;
pub mod shell;
pub mod skill_load;
pub mod sleep;
pub mod subagent;
pub mod todo;
pub mod update_goal;
pub mod webfetch;
pub mod websearch;
pub mod write;

pub mod send_message;

// Re-export from helper module
pub use helper::{FileStateStore, DEFAULT_MAX_TOOL_OUTPUT_LENGTH, MAX_FILE_SIZE};

// Re-export from executor and factory directly
pub use executor::{execute_tools_parallel, ToolExecutionResult};
pub use factory::{ToolRegistryConfig, ToolRegistryFactory};

pub use ask_user::{
    AskOption, AskQuestion, AskUserResponse, AskUserTool, ASK_USER_TOOL_NAME,
};
pub use edit::{EditTool, EDIT_TOOL_NAME};
pub use glob::{GlobTool, GLOB_TOOL_NAME};
pub use grep::{GrepTool, GREP_TOOL_NAME};
pub use read::{ReadTool, READ_TOOL_NAME};
pub use reminder::{ReminderTool, REMINDER_TOOL_NAME};
pub use send_message::{SendMessageTool, SEND_MESSAGE_TOOL_NAME};
pub use shell::{ShellTool, ShellToolCtx, SHELL_TOOL_NAME};
pub use skill_load::{SkillTool, SKILL_FILENAME, SKILL_TOOL_NAME};
pub use sleep::{SleepTool, SLEEP_TOOL_NAME};
pub use subagent::{SubagentPreset, SubagentTool, SUBAGENT_TOOL_NAME};
pub use todo::{TodoTool, TODO_TOOL_NAME};
pub use update_goal::{UpdateGoalTool, UPDATE_GOAL_TOOL_NAME};
pub use webfetch::{WebFetchTool, WEBFETCH_TOOL_NAME};
pub use websearch::{WebSearchTool, WEBSEARCH_TOOL_NAME};
pub use write::{WriteTool, WRITE_TOOL_NAME};

/// Guidance for tools that launch async/background tasks.
/// Tells the agent to end its turn immediately and wait for the result notification.
pub const ASYNC_LAUNCH_GUIDE: &str = "After launching, end your current turn immediately — do not sleep, poll, or re-launch. The result will be sent automatically when complete.";

/// Context provided to tools during execution
pub struct ToolExecCtx<'a> {
    /// The ID of this tool call
    pub tool_call_id: &'a str,
    /// Parent agent's message history (for context inheritance)
    pub parent_messages: Option<&'a [Arc<crate::types::Message>]>,
    /// Runtime cancel token for checking cancellation requests (tokio native)
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
    /// Working directory for file-based operations
    pub working_dir: std::path::PathBuf,
    /// Session ID for session-scoped operations (e.g., todo storage)
    pub session_id: String,
    /// Pre-generated `MessageId` for the tool result message, allowing progress
    /// events and the final result to share a consistent identifier.
    pub message_id: crate::types::MessageId,
    /// Current turn for file tracking and checkpointing
    /// Tools use this to track modified files
    pub turn: Option<std::sync::Arc<crate::agent::Turn>>,
    /// Skills available to this agent (for tools like `SubagentTool` that need to pass them on)
    pub skills: Vec<Arc<crate::skill::Skill>>,
    /// Maximum tool output length in bytes
    pub max_tool_output_length: usize,
}

impl<'a> ToolExecCtx<'a> {
    /// Create a new context with just the tool call ID and `session_id`
    pub fn new(
        tool_call_id: &'a str,
        working_dir: impl Into<std::path::PathBuf>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            tool_call_id,
            parent_messages: None,
            cancel_token: None,
            working_dir: working_dir.into(),
            session_id: session_id.into(),
            message_id: crate::types::MessageId::default(),
            turn: None,
            skills: Vec::new(),
            max_tool_output_length: 40_000,
        }
    }

    /// Create a context with tool call ID, parent messages, runtime token, working directory and `session_id`
    /// This is a convenience constructor for the common case where both
    /// `parent_messages` and `cancel_token` are available
    pub fn with_parent_ctx(
        tool_call_id: &'a str,
        parent_messages: Option<&'a [Arc<crate::types::Message>]>,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
        working_dir: impl Into<std::path::PathBuf>,
        session_id: impl Into<String>,
        message_id: crate::types::MessageId,
    ) -> Self {
        Self {
            tool_call_id,
            parent_messages,
            cancel_token,
            working_dir: working_dir.into(),
            session_id: session_id.into(),
            message_id,
            turn: None,
            skills: Vec::new(),
            max_tool_output_length: 40_000,
        }
    }

    #[must_use]
    pub fn with_parent_messages(mut self, messages: &'a [Arc<crate::types::Message>]) -> Self {
        self.parent_messages = Some(messages);
        self
    }

    #[must_use]
    pub fn with_cancel_token(mut self, token: Option<tokio_util::sync::CancellationToken>) -> Self {
        self.cancel_token = token;
        self
    }

    /// Set available skills for tools that need to spawn sub-agents
    #[must_use]
    pub fn with_skills(mut self, skills: Vec<Arc<crate::skill::Skill>>) -> Self {
        self.skills = skills;
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

    /// Track a file edit (backup current state BEFORE modifying)
    /// Must be called BEFORE file is modified
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

/// Trait for tools that track file state for read-before-write validation
pub trait FileStateAwareTool {
    /// Get the file state store if configured
    fn file_state_store(&self) -> Option<&Arc<FileStateStore>>;

    /// Check if the file has been modified since it was last read
    /// Default implementation uses the file state store to compare mtimes
    fn check_staleness(
        &self,
        path: &std::path::Path,
    ) -> impl std::future::Future<Output = std::result::Result<(), String>> + Send
    where
        Self: Sync,
    {
        async move {
            let store = self
                .file_state_store()
                .ok_or("File state store not initialized")?;

            let current_mtime = helper::get_mtime(path).await;
            store.check_staleness(path, current_mtime.unwrap_or(0))
        }
    }
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
                Arc::new(ToolDefinition {
                    name: tool.name().to_string(),
                    description: tool.desc().to_string(),
                    parameters: tool.schema(),
                })
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
}
