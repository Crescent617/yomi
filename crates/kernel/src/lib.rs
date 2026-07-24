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
pub mod channels;
pub mod checkpoint;
pub mod client;
pub mod comms;
pub mod compactor;
pub mod config;
pub mod cron;
pub mod event;
pub mod goal;
pub mod kernel;
pub mod memory;
pub mod notification;
pub mod permission;
pub mod prompt;
pub mod provider;
pub mod server;
pub mod skill;
pub mod storage;
pub mod tools;
pub mod transport;
pub mod types;
pub mod utils;
pub mod wire;
pub use checkpoint::{Checkpoint, CheckpointStore, FileOp, RewindTarget, TrackedFileInfo};
pub use config::{env_names, Config, ModelProvider};
pub use cron::{
    CreateCronJobInput, CronAction, CronError, CronJob, CronJobId, CronJobStatus, CronSchedule,
    CronScheduler, CronStore, CronWorker, SqliteCronStore, UpdateCronJobInput,
};
pub use event::{AgentEvent, ContentChunk, Event, ModelEvent, ToolEvent, UserEvent};
pub use kernel::CreateSessionInput;
pub use kernel::Kernel;
pub use permission::{Checker, Level, ToolLevelResolver};
pub use prompt::SystemPromptBuilder;
pub use provider::{AnthropicProvider, NoKeyProvider, OpenAIProvider, OpenAIResponseProvider};
pub use provider::{
    HttpError, ModelConfig, ModelStream, ModelStreamItem, Provider, RetryingProvider,
    ThinkingConfig, ToolCallRequest,
};
pub use skill::{deduplicate_skills, Skill, SkillLoader};
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
pub use tools::task::{
    CreateTaskInput, CreateTaskOutput, GetTaskOutput, ListTasksOutput, SharedTaskStore,
    SqliteTaskStorage, StatusChange, Task, TaskCreateTool, TaskEvent, TaskGetTool, TaskListItem,
    TaskListTool, TaskStatus, TaskStore, TaskSummary, TaskUpdateTool, TaskUpdates,
    UpdateTaskOutput, TASK_CREATE_TOOL_NAME, TASK_GET_TOOL_NAME, TASK_LIST_TOOL_NAME,
    TASK_UPDATE_TOOL_NAME,
};
pub use tools::{
    EditTool, GlobTool, GrepTool, PostMessageTool, ReadTool, ShellTool, SkillTool, SubagentTool,
    WriteTool,
};
pub use tools::{Tool, ToolRegistry};
pub use types::*;
pub use utils::path::{default_skill_folders, expand_tilde, DEFAULT_DATA_DIR};

use crate::agent::AgentConfig;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;

/// Full kernel initialisation: load config, apply env overrides,
/// finalise, and build a `Kernel` with the given cron setting.
///
/// `config_path` overrides the default config discovery when provided.
///
/// Returns `(kernel, config, config_file)` so callers can derive `base_dir`
/// from `config_file.parent()` if needed.
pub async fn init_kernel(
    config_path: Option<&PathBuf>,
    enable_cron: bool,
) -> Result<(Arc<Kernel>, Config, Option<PathBuf>)> {
    let config_file = config_path.cloned().or_else(Config::discover_file);
    let mut config = match &config_file {
        Some(path) => Config::from_file(path)
            .map_err(|e| KernelError::config(format!("Failed to load config: {e}")))?,
        None => Config::default(),
    };
    config.inject_env()?;
    config.apply_env_overrides();
    config.finalize();
    config.validate()?;

    let kernel = build_kernel(&config, enable_cron).await?;
    Ok((kernel, config, config_file))
}

pub fn create_provider_for_model(model: &ModelConfig) -> Result<Arc<dyn Provider>> {
    if !model.has_api_key() {
        tracing::warn!(
            "No API key for model '{}' — using NoKeyProvider",
            model.model_id
        );
        return Ok(Arc::new(NoKeyProvider));
    }
    match model.provider {
        ModelProvider::OpenAI => Ok(Arc::new(OpenAIProvider::new()?)),
        ModelProvider::OpenAIResponse => Ok(Arc::new(OpenAIResponseProvider::new()?)),
        ModelProvider::Anthropic => Ok(Arc::new(AnthropicProvider::new()?)),
    }
}

/// Load skills from disk and build a complete `AgentConfig` from a `Config`.
/// `base_dir` is used to resolve relative skill folder paths.
pub fn build_agent_config(config: &Config, base_dir: &Path) -> AgentConfig {
    let skill_folders = config
        .skill_folders()
        .iter()
        .map(PathBuf::from)
        .map(|p| if p.is_relative() { base_dir.join(p) } else { p })
        .collect::<Vec<_>>();

    let mut skills = SkillLoader::new(skill_folders)
        .load_all()
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to load skills: {e}");
            Vec::new()
        });

    deduplicate_skills(&mut skills);

    if !skills.is_empty() {
        tracing::info!("Loaded {} skill(s)", skills.len());
        for skill in &skills {
            tracing::debug!("  - {} (from {})", skill.name, skill.source_path.display());
        }
    }

    let mut agent = config.agent.clone();
    agent.skills = skills;
    agent.enable_cron_tool = config.features.cron_tool_enabled();
    agent
}

/// Build a `Kernel` from an already-finalized `Config`.
///
/// `config.data_dir` is used to resolve relative skill folders and build the agent config.
/// This function does not discover or load config — callers must do that first.
///
/// Returns the fully constructed `Kernel` wrapped in an `Arc`.
pub async fn build_kernel(config: &Config, enable_cron: bool) -> Result<Arc<Kernel>> {
    tokio::fs::create_dir_all(&config.data_dir)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to create data directory: {e}")))?;

    let storage = StorageSet::open_with_config(&config.data_dir, config)
        .await
        .map_err(|e| KernelError::storage(format!("Failed to open storage: {e}")))?;
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
        move || build_agent_config(&config, &config.data_dir)
    })
    .await
    .map_err(|e| {
        KernelError::storage(format!(
            "Failed to build agent config in blocking task: {e}"
        ))
    })?;

    let kernel = Kernel::new(
        &storage,
        agent_config,
        Some(task_store),
        Some(config.agent.compactor.clone()),
        skill_folders,
        enable_cron,
        if config.channels.is_empty() {
            None
        } else {
            Some(storage.channel_store())
        },
        config.models.clone(),
        config.tasks.clone(),
        config.features.update_session_title_enabled(),
    )?;

    Ok(kernel)
}
