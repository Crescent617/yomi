//! yomi-core - Core library for the yomi AI coding assistant

/// Environment variable prefix - change this to rebrand the entire CLI
/// Default: "YOMI_" (produces env vars like `YOMI_API_KEY`)
pub const ENV_PREFIX: &str = "YOMI_";

/// Compile-time string concatenation for env var names
/// Usage: `env_name!("API_KEY")` expands to `"YOMI_API_KEY"`
#[macro_export]
macro_rules! env_name {
    ($suffix:expr) => {
        std::concat!("YOMI_", $suffix)
    };
}

pub mod agent;
pub mod app;
pub mod compactor;
pub mod config;
pub mod event;
pub mod misc;
pub mod permissions;
pub mod project_memory;
pub mod prompt;
pub mod providers;
pub mod skill;
pub mod storage;
pub mod task;
pub mod tools;
pub mod types;
pub mod utils;

// Re-export permissions types
pub use permissions::{Checker, Level, ToolLevelResolver};

// Re-export commonly used types
pub use app::{Coordinator, Session, SessionConfig};
pub use config::{env_names, expand_tilde, Config, ModelProvider, DEFAULT_DATA_DIR};
pub use event::{
    AgentEvent, AgentResult, ContentChunk, Event, ModelEvent, ProgressUpdate, SystemEvent,
    ToolEvent, UserEvent,
};
pub use misc::plugin::{Plugin, PluginLoader};
pub use prompt::{PromptBuilder, SystemPromptBuilder};
pub use providers::{
    HttpError, ModelConfig, ModelStream, ModelStreamItem, Provider, RetryingProvider,
    ThinkingConfig, ToolCallRequest,
};
pub use skill::{Skill, SkillLoader};
pub use storage::{Storage, StorageConfig};
pub use tools::{Tool, ToolRegistry};
pub use types::*;

// Conditional re-exports for convenience
#[cfg(feature = "providers")]
pub use providers::{AnthropicProvider, OpenAIProvider};
pub use tools::{
    execute_tools_parallel, BashTool, EditTool, GlobTool, GrepTool, ReadTool, SubagentTool,
    WriteTool,
};

// Task system re-exports
pub use task::{
    CreateTaskInput, CreateTaskOutput, GetTaskOutput, ListTasksOutput, SharedTaskStore,
    SqliteTaskStorage, StatusChange, Task, TaskCreateTool, TaskEvent, TaskGetTool, TaskListItem,
    TaskListTool, TaskStatus, TaskStore, TaskSummary, TaskUpdateTool, TaskUpdates,
    UpdateTaskOutput, TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME,
    TASK_UPDATE_TOOL_NAME,
};
