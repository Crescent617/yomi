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
pub mod comms;
pub mod compactor;
pub mod config;
pub mod daemon_signal;
pub mod event;
pub mod goal;
pub mod hooks;
pub use hooks::{HookContext, HookEvent, HookHandler, HookRegistry, HookResult};
pub mod channels;
pub mod cron;
pub mod logging;
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
pub use app::Coordinator;
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
pub use tools::{Tool, ToolRegistryFactory};
pub use types::*;
pub use utils::path::{default_skill_folders, expand_tilde, DEFAULT_DATA_DIR};

// Re-exports for providers
pub use providers::{AnthropicProvider, NoKeyProvider, OpenAIProvider};
pub use tools::{
    execute_tools_parallel, EditTool, GlobTool, GrepTool, ReadTool, ShellTool, SkillTool,
    SubagentTool, WriteTool,
};

// Cron system re-exports
pub use cron::{
    CreateCronJobInput, CronAction, CronError, CronJob, CronJobId, CronJobStatus, CronSchedule,
    CronScheduler, CronStore, CronWorker, SqliteCronStore, UpdateCronJobInput,
};

// Task system re-exports
pub use task::{
    CreateTaskInput, CreateTaskOutput, GetTaskOutput, ListTasksOutput, SharedTaskStore,
    SqliteTaskStorage, StatusChange, Task, TaskCreateTool, TaskEvent, TaskGetTool, TaskListItem,
    TaskListTool, TaskStatus, TaskStore, TaskSummary, TaskUpdateTool, TaskUpdates,
    UpdateTaskOutput, TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME,
    TASK_UPDATE_TOOL_NAME,
};

use std::path::PathBuf;
use std::sync::Arc;

/// Full coordinator initialisation: load config, apply env overrides,
/// finalise, and build a `Coordinator` with the given cron setting.
///
/// `config_path` overrides the default config discovery when provided.
///
/// Returns `(coordinator, config, config_file)` so callers can derive `base_dir`
/// from `config_file.parent()` if needed.
pub async fn init_coordinator(
    config_path: Option<&PathBuf>,
    enable_cron: bool,
) -> Result<(Arc<Coordinator>, Config, Option<PathBuf>)> {
    let config_file = config_path.cloned().or_else(Config::discover_file);
    let mut config = match &config_file {
        Some(path) => Config::from_file(path)
            .map_err(|e| KernelError::config(format!("Failed to load config: {e}")))?,
        None => Config::default(),
    };
    config.apply_env_overrides();
    config.finalize();

    let coordinator = build_coordinator(&config, enable_cron).await?;
    Ok((coordinator, config, config_file))
}

/// Create a provider from configuration.
/// Returns `NoKeyProvider` when no API key is configured so the application can
/// start and fail gracefully at message-send time rather than on boot.
pub fn create_provider(config: &Config) -> Result<Arc<dyn Provider>> {
    if !config.has_api_key() {
        tracing::warn!("No API key configured — using NoKeyProvider");
        return Ok(Arc::new(NoKeyProvider));
    }
    match config.agent.model.provider {
        ModelProvider::OpenAI => Ok(Arc::new(OpenAIProvider::new()?)),
        ModelProvider::Anthropic => Ok(Arc::new(AnthropicProvider::new()?)),
    }
}

/// Build a `Coordinator` from an already-finalized `Config`.
///
/// `config.data_dir` is used to resolve relative skill folders and build the agent config.
/// This function does not discover or load config — callers must do that first.
///
/// Returns the fully constructed `Coordinator` wrapped in an `Arc`.
pub async fn build_coordinator(config: &Config, enable_cron: bool) -> Result<Arc<Coordinator>> {
    tokio::fs::create_dir_all(&config.data_dir)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to create data directory: {e}")))?;

    let storage = StorageSet::open_with_config(&config.data_dir, config)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to open storage: {e}")))?;
    let provider = create_provider(config)
        .map_err(|e| KernelError::storage(format!("Failed to create provider: {e}")))?;
    let task_store = Arc::new(
        TaskStore::new(&config.data_dir)
            .await
            .map_err(|e| KernelError::storage(format!("Failed to create task store: {e}")))?,
    );
    let skill_folders = config
        .skill_folders()
        .iter()
        .map(PathBuf::from)
        .map(|p| {
            if p.is_relative() {
                config.data_dir.join(p)
            } else {
                p
            }
        })
        .collect();

    let agent_config = tokio::task::spawn_blocking({
        let config = config.clone();
        move || server::build_agent_config(&config, &config.data_dir)
    })
    .await
    .map_err(|e| {
        KernelError::storage(format!(
            "Failed to build agent config in blocking task: {e}"
        ))
    })?;

    let coordinator = Coordinator::new(
        &storage,
        provider,
        agent_config,
        Some(task_store),
        Some(config.agent.compactor.clone()),
        skill_folders,
        config
            .features
            .hooks
            .then(|| hooks::build_registry(&config.hooks, config.features.allow_command_hooks)),
        enable_cron,
        if config.channels.is_empty() {
            None
        } else {
            Some(storage.channel_store())
        },
    );

    Ok(coordinator)
}
