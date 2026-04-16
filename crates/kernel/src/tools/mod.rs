use crate::task::{SharedTaskStore, TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
use crate::types::{ToolDefinition, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

pub mod base;
pub mod bash;
pub mod edit;
pub mod edit_utils;
pub mod file_state;
pub mod glob;
pub mod grep;
pub mod line_numbers;
pub mod parallel;
pub mod read;
pub mod subagent;
pub mod write;

pub use base::{FileTool, MAX_FILE_SIZE};

pub use bash::{BashTool, BashToolCtx, BASH_TOOL_NAME};
pub use edit::{EditTool, EDIT_TOOL_NAME};
pub use glob::{GlobTool, GLOB_TOOL_NAME};
pub use grep::{GrepTool, GREP_TOOL_NAME};
pub use parallel::execute_tools_parallel;
pub use read::{ReadTool, READ_TOOL_NAME};
pub use subagent::{SubagentTool, SUBAGENT_TOOL_NAME};
pub use write::{WriteTool, WRITE_TOOL_NAME};

/// Context provided to tools during execution
pub struct ToolExecCtx<'a> {
    /// The ID of this tool call
    pub tool_call_id: &'a str,
    /// Parent agent's message history (for context inheritance)
    pub parent_messages: Option<&'a [Arc<crate::types::Message>]>,
    /// Runtime cancel token for checking cancellation requests (tokio native)
    pub cancel_token: Option<tokio_util::sync::CancellationToken>,
}

impl<'a> ToolExecCtx<'a> {
    /// Create a new context with just the tool call ID
    pub fn new(tool_call_id: &'a str) -> Self {
        Self {
            tool_call_id,
            parent_messages: None,
            cancel_token: None,
        }
    }

    /// Create a context with tool call ID, parent messages, and runtime token
    /// This is a convenience constructor for the common case where both
    /// `parent_messages` and `cancel_token` are available
    pub fn with_parent_ctx(
        tool_call_id: &'a str,
        parent_messages: Option<&'a [Arc<crate::types::Message>]>,
        cancel_token: Option<tokio_util::sync::CancellationToken>,
    ) -> Self {
        Self {
            tool_call_id,
            parent_messages,
            cancel_token,
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

    /// Check if cancellation has been requested
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.as_ref().is_some_and(|t| t.is_cancelled())
    }

    /// Get a future that completes when cancellation is requested
    pub fn cancelled(&self) -> impl std::future::Future<Output = ()> + 'static {
        match self.cancel_token.clone() {
            Some(token) => {
                Either::Left(async move { token.cancelled().await })
            }
            None => {
                // If no token, never complete (always pending)
                Either::Right(std::future::pending())
            }
        }
    }
}

use futures::future::Either;

/// Core trait for tools
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn desc(&self) -> &str;
    fn params(&self) -> Value;

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
                    parameters: tool.params(),
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
    pub fn register_task_tools(&mut self, store: SharedTaskStore, task_list_id: String) {
        self.register(TaskCreateTool::new(store.clone(), task_list_id.clone()));
        self.register(TaskUpdateTool::new(store.clone(), task_list_id.clone()));
        self.register(TaskListTool::new(store.clone(), task_list_id.clone()));
        self.register(TaskGetTool::new(store, task_list_id));
    }
}
