//! yomi-core - Core library for the yomi AI coding assistant

/// Environment variable prefix - change this to rebrand the entire CLI.
/// **IMPORTANT**: When changing this, also update the `env_name!` macro below
/// (Rust `concat!` only accepts string literals, so they cannot be derived
/// from a `const` automatically).
pub const ENV_PREFIX: &str = "YOMI_";

/// Compile-time string concatenation for env var names.
/// Usage: `env_name!("API_KEY")` expands to `"YOMI_API_KEY"`
#[macro_export]
macro_rules! env_name {
    ($suffix:expr) => {
        std::concat!("YOMI_", $suffix)
    };
}

pub mod agent;
pub mod app;
pub mod checkpoint;
pub mod client;
pub mod compactor;
pub mod config;
pub mod event;
pub mod goal;
pub mod hooks;
pub use hooks::{HookContext, HookEvent, HookHandler, HookRegistry, HookResult};
pub mod memory;
pub mod permissions;
pub mod prompt;
pub mod providers;
pub mod server;
pub mod skill;
pub mod storage;
pub mod task;
pub mod tools;
pub mod transport;
pub mod types;
pub mod utils;
pub mod wire;

// Re-export permissions types
pub use permissions::{Checker, Level, ToolLevelResolver};

// Re-export checkpoint types
pub use checkpoint::{Checkpoint, CheckpointStore, FileOp, RewindTarget, TrackedFileInfo};

// Re-export commonly used types
pub use app::coordinator::CreateSessionInput;
pub use app::{Coordinator, Session, SessionConfig};
pub use config::{env_names, Config, ModelProvider};
pub use event::{AgentEvent, ContentChunk, Event, ModelEvent, SystemEvent, ToolEvent, UserEvent};

pub use prompt::SystemPromptBuilder;
pub use providers::{
    HttpError, ModelConfig, ModelStream, ModelStreamItem, Provider, RetryingProvider,
    ThinkingConfig, ToolCallRequest,
};
pub use skill::{deduplicate_skills, Skill, SkillLoader};
// Re-export storage domains
pub use storage::{
    file_state::{FileState, FileStateStore, JsonlFileStateStore},
    message::{JsonlMessageStore, MessageStore},
    project::{ProjectStore, SqliteProjectStore},
    session::{SessionInfo, SessionStore, SqliteSessionStore},
    todo::{
        strip_system_reminders, JsonTodoStore, TodoItem, TodoListData, TodoStatus, TodoStore,
        SYSTEM_REMINDER_END, SYSTEM_REMINDER_START,
    },
    usage::{SqliteUsageStore, UsageRecord, UsageStore, UsageSummary, UsageType},
    StorageSet,
};
pub use tools::{Tool, ToolRegistry};
pub use types::*;
pub use utils::path::{default_skill_folders, expand_tilde, DEFAULT_DATA_DIR};

// Re-exports for providers
pub use providers::{AnthropicProvider, NoKeyProvider, OpenAIProvider};
pub use tools::{
    execute_tools_parallel, EditTool, GlobTool, GrepTool, ReadTool, ShellTool, SkillTool,
    SubagentTool, WriteTool,
};

// Task system re-exports
pub use task::{
    CreateTaskInput, CreateTaskOutput, GetTaskOutput, ListTasksOutput, SharedTaskStore,
    SqliteTaskStorage, StatusChange, Task, TaskCreateTool, TaskEvent, TaskGetTool, TaskListItem,
    TaskListTool, TaskStatus, TaskStore, TaskSummary, TaskUpdateTool, TaskUpdates,
    UpdateTaskOutput, TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME,
    TASK_UPDATE_TOOL_NAME,
};
