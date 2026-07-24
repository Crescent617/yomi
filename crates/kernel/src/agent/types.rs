use super::BgTaskTracker;
use crate::compactor::Compactor;
use crate::provider::{ModelConfig, ProviderError};
use crate::skill::Skill;
use crate::types::Message;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use thiserror::Error;
/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// Default model name for new sessions (points to a model in Config.models)
    pub default_model: String,
    pub max_iterations: usize,
    pub enable_subagent: bool,
    pub system_prompt: String,
    #[serde(skip)]
    pub skills: Vec<Arc<Skill>>,
    /// Tool blocklist (regex patterns) for disabling tools
    pub tool_blocklist: Vec<String>,
    /// Compactor configuration for context management
    pub compactor: Compactor,
    /// Maximum tool output length in bytes (default `40_000`)
    pub max_tool_output_length: usize,
    /// Enable the cron tool for agents. Plumbed from `[features] cron_tool`
    /// in `build_agent_config`; not settable via the `[agent]` section.
    #[serde(skip)]
    pub enable_cron_tool: bool,
}

/// Configuration for spawning a new agent
#[derive(Clone)]
pub struct AgentSpawnArgs {
    pub base_prompt: String,
    pub skills: Vec<Arc<Skill>>,
    pub history: Vec<Arc<Message>>,
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub max_iterations: usize,
    pub enable_subagent: bool,
    pub working_dir: std::path::PathBuf,
    /// Optional cancel token to share with parent (for cascading cancellation)
    pub cancel_token: Option<super::CancelToken>,
    /// Enable the cron tool for this agent.
    pub enable_cron_tool: bool,
    /// Optional file state store (for restoring from previous session)
    pub file_state_store: Option<Arc<crate::tools::helper::FileStateStore>>,
    pub tool_blocklist: Vec<String>,
    /// Maximum tool output length in bytes
    pub max_tool_output_length: usize,
    /// Mailbox for receiving input messages. Created by Conductor or subagent caller.
    pub mailbox: Arc<crate::comms::Mailbox>,
    pub input_bus: Option<Arc<crate::comms::InputBus>>,
}

impl std::fmt::Debug for AgentSpawnArgs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSpawnArgs")
            .field("base_prompt", &self.base_prompt)
            .field("skills", &self.skills)
            .field("history", &self.history)
            .field("session_id", &self.session_id)
            .field("parent_session_id", &self.parent_session_id)
            .field("max_iterations", &self.max_iterations)
            .field("enable_sub_agents", &self.enable_subagent)
            .field("working_dir", &self.working_dir)
            .field("cancel_token", &self.cancel_token.is_some())
            .field("enable_cron_tool", &self.enable_cron_tool)
            .field("file_state_store", &self.file_state_store.is_some())
            .field("tool_blocklist", &self.tool_blocklist)
            .field("max_tool_output_length", &self.max_tool_output_length)
            .field("mailbox", &self.mailbox)
            .field("input_bus", &self.input_bus.is_some())
            .finish()
    }
}

impl AgentSpawnArgs {
    /// Create a new config with the given base prompt, session, mailbox and working directory
    pub fn new(
        base_prompt: impl Into<String>,
        session_id: impl Into<String>,
        mailbox: impl Into<Arc<crate::comms::Mailbox>>,
        working_dir: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            base_prompt: base_prompt.into(),
            skills: Vec::new(),
            history: Vec::<Arc<Message>>::new(),
            session_id: session_id.into(),
            parent_session_id: None,
            max_iterations: 100,
            enable_subagent: true,
            working_dir: working_dir.into(),
            cancel_token: None,
            enable_cron_tool: false,
            file_state_store: None,
            tool_blocklist: Vec::new(),
            max_tool_output_length: 40_000,
            input_bus: None,
            mailbox: mailbox.into(),
        }
    }

    /// Set skills to include
    #[must_use]
    pub fn with_skills(mut self, skills: Vec<Arc<Skill>>) -> Self {
        self.skills = skills;
        self
    }

    /// Set history messages
    #[must_use]
    pub fn with_history(mut self, history: Vec<Message>) -> Self {
        self.history = history.into_iter().map(Arc::new).collect();
        self
    }

    /// Set history messages from Arc (internal use)
    #[must_use]
    pub fn with_arc_history(mut self, history: Vec<Arc<Message>>) -> Self {
        self.history = history;
        self
    }

    /// Set parent session ID for task sharing
    #[must_use]
    pub fn with_parent_session(mut self, parent_session_id: impl Into<String>) -> Self {
        self.parent_session_id = Some(parent_session_id.into());
        self
    }

    /// Set max iterations
    #[must_use]
    pub const fn with_max_iterations(mut self, max: usize) -> Self {
        self.max_iterations = max;
        self
    }

    #[must_use]
    pub const fn with_subagent(mut self, enabled: bool) -> Self {
        self.enable_subagent = enabled;
        self
    }

    /// Enable the cron tool
    #[must_use]
    pub const fn with_cron_tool(mut self, enabled: bool) -> Self {
        self.enable_cron_tool = enabled;
        self
    }

    #[must_use]
    pub fn with_cancel_token(mut self, token: super::CancelToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    /// Set file state store (for restoring from previous session)
    #[must_use]
    pub fn with_file_state_store(
        mut self,
        store: Arc<crate::tools::helper::FileStateStore>,
    ) -> Self {
        self.file_state_store = Some(store);
        self
    }

    /// Set tool blocklist (regex patterns)
    #[must_use]
    pub fn with_tool_blocklist(mut self, blocklist: Vec<String>) -> Self {
        self.tool_blocklist = blocklist;
        self
    }

    /// Set the input bus for publishing messages (needed by subagent tool)
    #[must_use]
    pub fn with_input_bus(mut self, input_bus: Arc<crate::comms::InputBus>) -> Self {
        self.input_bus = Some(input_bus);
        self
    }

    /// Set max tool output length
    #[must_use]
    pub const fn with_max_tool_output_length(mut self, max: usize) -> Self {
        self.max_tool_output_length = max;
        self
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            default_model: "default".to_string(),
            max_iterations: 100,
            enable_subagent: true,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            skills: Vec::new(),
            tool_blocklist: Vec::new(),
            compactor: Compactor::default(),
            max_tool_output_length: 40_000,
            enable_cron_tool: false,
        }
    }
}

/// Default system prompt for the agent
const DEFAULT_SYSTEM_PROMPT: &str = include_str!("prompts/sp.md");

/// Sub-agent execution mode
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubAgentMode {
    Async,
    Sync,
}

impl std::fmt::Display for SubAgentMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Async => write!(f, "async"),
            Self::Sync => write!(f, "sync"),
        }
    }
}

/// Agent state machine
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentState {
    Idle,
    Streaming,
    ExecutingTool,
    Compacting,
}

impl AgentState {
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Streaming => "streaming",
            Self::ExecutingTool => "executing_tool",
            Self::Compacting => "compacting",
        }
    }
}

/// Agent execution context for state management
#[derive(Debug, Clone)]
pub struct AgentExecutionContext {
    inner: Arc<AgentExecutionContextInner>,
}

struct AgentExecutionContextInner {
    state_tx: tokio::sync::watch::Sender<AgentState>,
    iteration_count: AtomicUsize,
    on_state_change: Option<Box<dyn Fn(AgentState) + Send + Sync>>,
}

impl std::fmt::Debug for AgentExecutionContextInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentExecutionContextInner")
            .field("state_tx", &self.state_tx)
            .field("iteration_count", &self.iteration_count)
            .field("on_state_change", &self.on_state_change.is_some())
            .finish()
    }
}

impl AgentExecutionContext {
    pub fn new(
        initial_state: AgentState,
        on_state_change: Option<Box<dyn Fn(AgentState) + Send + Sync>>,
    ) -> Self {
        let (state_tx, _) = tokio::sync::watch::channel(initial_state);
        Self {
            inner: Arc::new(AgentExecutionContextInner {
                state_tx,
                iteration_count: AtomicUsize::new(0),
                on_state_change,
            }),
        }
    }

    pub fn transition_to(&self, new_state: AgentState) -> bool {
        let current = *self.inner.state_tx.borrow();
        if current == new_state {
            return true;
        }
        self.inner.state_tx.send_replace(new_state);
        if let Some(ref hook) = self.inner.on_state_change {
            hook(new_state);
        }
        true
    }

    pub fn current_state(&self) -> AgentState {
        *self.inner.state_tx.borrow()
    }

    pub fn increment_iteration(&self) {
        self.inner.iteration_count.fetch_add(1, Ordering::SeqCst);
    }

    pub fn reset_iteration(&self) {
        self.inner.iteration_count.store(0, Ordering::SeqCst);
    }

    pub fn iteration_count(&self) -> usize {
        self.inner.iteration_count.load(Ordering::SeqCst)
    }
}

/// Shared resources across agents
#[derive(Clone)]
pub struct AgentShared {
    /// Model registry (key -> `ModelConfig`) for runtime resolution
    pub models: Arc<BTreeMap<String, ModelConfig>>,
    /// Default model key for new sessions
    pub default_model: String,
    /// Task store for task tools (legacy)
    pub task_store: Option<Arc<crate::tools::task::TaskStore>>,
    /// Todo storage for todo list persistence
    pub todo_storage: Option<Arc<dyn crate::storage::TodoStore>>,
    /// Context compactor for managing long conversations
    pub compactor: Option<crate::compactor::Compactor>,
    /// Session store for session operations
    pub session_store: Option<Arc<dyn crate::storage::SessionStore>>,
    /// Message store for message persistence
    pub message_store: Option<Arc<dyn crate::storage::MessageStore>>,
    /// Usage store for token tracking
    pub usage_store: Option<Arc<dyn crate::storage::UsageStore>>,
    /// Shared permission state for all agents in a session
    pub permission_state: Option<crate::permission::PermissionState>,
    /// Skill folders for the `skill_load` tool
    pub skill_folders: Vec<std::path::PathBuf>,
    /// File state store for tracking file modification times (cleared on compaction)
    pub file_state_store: Option<Arc<crate::tools::helper::FileStateStore>>,
    /// Checkpoint store for file history tracking
    pub checkpoint_store: Option<Arc<dyn crate::checkpoint::CheckpointStore>>,
    /// Data directory for file backup storage
    pub data_dir: std::path::PathBuf,
    /// Optional user message interceptor for injecting reminders/context
    pub message_interceptor: Option<Arc<dyn super::UserMsgInterceptor>>,
    /// Channel manager for external platform integrations (Telegram, Feishu, etc.)
    pub channel_hub: Option<Arc<crate::channels::hub::ChannelHub>>,
    /// Optional goal store for autonomous goal-mode execution
    pub goal_store: Option<Arc<dyn crate::goal::GoalStore>>,
    /// Global event bus for all agents and sessions
    pub event_bus: Option<Arc<crate::comms::EventBus>>,
    /// Runtime tracker for asynchronous background work grouped by session.
    pub background_tasks: Arc<BgTaskTracker>,
    /// Cron store for scheduled job operations (None when cron is disabled).
    pub cron_store: Option<Arc<dyn crate::cron::CronStore>>,
    /// Shared slot for the running cron scheduler. Owned by `Kernel`, filled by
    /// `KernelServer` on start; tools use it to notify the scheduler of job
    /// changes. Empty when not running under a daemon.
    pub cron_scheduler: Arc<std::sync::Mutex<Option<Arc<crate::cron::CronScheduler>>>>,
}

impl AgentShared {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        models: Arc<BTreeMap<String, ModelConfig>>,
        default_model: String,
        task_store: Option<Arc<crate::tools::task::TaskStore>>,
        todo_storage: Option<Arc<dyn crate::storage::TodoStore>>,
        compactor: Option<crate::compactor::Compactor>,
        session_store: Option<Arc<dyn crate::storage::SessionStore>>,
        message_store: Option<Arc<dyn crate::storage::MessageStore>>,
        usage_store: Option<Arc<dyn crate::storage::UsageStore>>,
        permission_state: Option<crate::permission::PermissionState>,
        skill_folders: Vec<std::path::PathBuf>,
        file_state_store: Option<Arc<crate::tools::helper::FileStateStore>>,
        checkpoint_store: Option<Arc<dyn crate::checkpoint::CheckpointStore>>,
    ) -> Self {
        Self::with_data_dir(
            models,
            default_model,
            task_store,
            todo_storage,
            compactor,
            session_store,
            message_store,
            usage_store,
            permission_state,
            skill_folders,
            file_state_store,
            checkpoint_store,
            std::path::PathBuf::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_data_dir(
        models: Arc<BTreeMap<String, ModelConfig>>,
        default_model: String,
        task_store: Option<Arc<crate::tools::task::TaskStore>>,
        todo_storage: Option<Arc<dyn crate::storage::TodoStore>>,
        compactor: Option<crate::compactor::Compactor>,
        session_store: Option<Arc<dyn crate::storage::SessionStore>>,
        message_store: Option<Arc<dyn crate::storage::MessageStore>>,
        usage_store: Option<Arc<dyn crate::storage::UsageStore>>,
        permission_state: Option<crate::permission::PermissionState>,
        skill_folders: Vec<std::path::PathBuf>,
        file_state_store: Option<Arc<crate::tools::helper::FileStateStore>>,
        checkpoint_store: Option<Arc<dyn crate::checkpoint::CheckpointStore>>,
        data_dir: std::path::PathBuf,
    ) -> Self {
        Self {
            models,
            default_model,
            task_store,
            todo_storage,
            compactor,
            session_store,
            message_store,
            usage_store,
            permission_state,
            skill_folders,
            file_state_store,
            checkpoint_store,
            data_dir,
            message_interceptor: None,
            goal_store: None,
            channel_hub: None,
            event_bus: None,
            background_tasks: Arc::new(BgTaskTracker::default()),
            cron_store: None,
            cron_scheduler: Arc::new(std::sync::Mutex::new(None)),
        }
    }

    /// Create a new `AgentShared` with per-session resources added
    #[must_use]
    pub fn with_per_session(
        &self,
        permission_state: Option<crate::permission::PermissionState>,
        file_state_store: Option<Arc<crate::tools::helper::FileStateStore>>,
        checkpoint_store: Option<Arc<dyn crate::checkpoint::CheckpointStore>>,
    ) -> Self {
        Self {
            permission_state,
            file_state_store,
            checkpoint_store,
            goal_store: self.goal_store.clone(),
            channel_hub: self.channel_hub.clone(),
            event_bus: self.event_bus.clone(),
            background_tasks: Arc::clone(&self.background_tasks),
            ..self.clone()
        }
    }

    /// Set the user message interceptor
    #[must_use]
    pub fn with_message_interceptor(
        mut self,
        interceptor: Arc<dyn super::UserMsgInterceptor>,
    ) -> Self {
        self.message_interceptor = Some(interceptor);
        self
    }

    /// Set the goal store
    #[must_use]
    pub fn with_goal_store(mut self, store: Arc<dyn crate::goal::GoalStore>) -> Self {
        self.goal_store = Some(store);
        self
    }

    /// Set the cron store and the shared scheduler slot.
    #[must_use]
    pub fn with_cron(
        mut self,
        store: Option<Arc<dyn crate::cron::CronStore>>,
        scheduler: Arc<std::sync::Mutex<Option<Arc<crate::cron::CronScheduler>>>>,
    ) -> Self {
        self.cron_store = store;
        self.cron_scheduler = scheduler;
        self
    }

    /// Set the channel manager for external platform integrations.
    #[must_use]
    pub fn with_channel_manager(
        mut self,
        manager: Option<Arc<crate::channels::hub::ChannelHub>>,
    ) -> Self {
        self.channel_hub = manager;
        self
    }

    /// Set the event bus
    #[must_use]
    pub fn with_event_bus(mut self, event_bus: Arc<crate::comms::EventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Set the skill folders
    #[must_use]
    pub fn with_skill_folders(mut self, skill_folders: Vec<std::path::PathBuf>) -> Self {
        self.skill_folders = skill_folders;
        self
    }

    /// Resolve the current provider and model config for the given session.
    /// Reads `model_key` from the session store, falls back to `default_model`
    /// (also when the stored key no longer exists in the registry).
    /// Errors only if even the fallback key is not found in the registry.
    pub async fn resolve_model(
        &self,
        session_id: &crate::types::SessionId,
    ) -> Result<(Arc<dyn crate::provider::Provider>, Arc<ModelConfig>), AgentError> {
        let stored_key = match &self.session_store {
            Some(store) => match store.get(session_id).await {
                Ok(info) => info.and_then(|i| i.model_key),
                Err(e) => {
                    tracing::warn!(
                        "Failed to read session {} from store while resolving model \
                         (falling back to default_model '{}'): {}",
                        session_id.0,
                        self.default_model,
                        e
                    );
                    None
                }
            },
            None => None,
        };

        let mut key = stored_key.unwrap_or_else(|| self.default_model.clone());

        // Stale model_key (e.g. model removed from config): fall back to default.
        if !self.models.contains_key(&key) && key != self.default_model {
            tracing::warn!(
                "Session {} references unknown model '{}' — falling back to \
                 default_model '{}'. Available: {:?}",
                session_id.0,
                key,
                self.default_model,
                self.models.keys().collect::<Vec<_>>()
            );
            key.clone_from(&self.default_model);
        }

        let model_config = self.models.get(&key).cloned().ok_or_else(|| {
            tracing::error!(
                "Model '{}' not found in registry for session {}. Available: {:?}",
                key,
                session_id.0,
                self.models.keys().collect::<Vec<_>>()
            );
            AgentError::Other(format!(
                "Model '{}' not found in registry. Available models: {}",
                key,
                self.models.keys().cloned().collect::<Vec<_>>().join(", ")
            ))
        })?;

        let provider = crate::create_provider_for_model(&model_config).map_err(|e| {
            AgentError::Other(format!("Failed to create provider for model '{key}': {e}"))
        })?;
        Ok((provider, Arc::new(model_config)))
    }
}

/// Agent error type using thiserror
#[derive(Error, Debug, Clone)]
pub enum AgentError {
    /// Agent reached maximum iterations
    #[error("Agent reached maximum iterations: {count}")]
    MaxIterationsExceeded { count: usize },

    /// Cancelled is a terminal error - agent was cancelled by user or parent
    #[error("Cancelled: {0}")]
    Cancelled(String),

    /// Stream task panicked
    #[error("Stream task panicked: {0}")]
    StreamTaskPanicked(String),

    /// Permission check failed
    #[error("Permission check failed: {0}")]
    PermissionCheckFailed(String),

    /// Agent does not have permission checker configured
    #[error("Agent does not have permission checker")]
    NoPermissionChecker,

    /// Provider error (includes HTTP, timeout, parse errors, etc.)
    #[error("{0}")]
    Provider(#[from] ProviderError),

    /// Serialization error
    #[error("Serialization error: {0}")]
    Serialization(String),

    /// Agent was shut down by request
    #[error("Shutdown")]
    Shutdown,
    /// Generic catch-all error
    #[error("{0}")]
    Other(String),
}

impl AgentError {
    pub fn is_retryable(&self) -> bool {
        use AgentError::{
            Cancelled, MaxIterationsExceeded, NoPermissionChecker, Other, PermissionCheckFailed,
            Provider, Serialization, Shutdown, StreamTaskPanicked,
        };
        match self {
            // Delegate to ProviderError's retry logic
            Provider(e) => e.is_retryable(),
            // These errors should NOT be retried
            MaxIterationsExceeded { .. }
            | Cancelled(_)
            | PermissionCheckFailed(_)
            | NoPermissionChecker
            | Serialization(_)
            | Shutdown
            | Other(_) => false,
            // Stream task panics might be transient
            StreamTaskPanicked(_) => true,
        }
    }

    /// Check if this is a provider context-window overflow.
    pub fn is_context_overflow(&self) -> bool {
        matches!(self, AgentError::Provider(error) if error.is_context_overflow())
    }

    /// Check if this is a cancellation error (terminal, not a failure)
    pub fn is_cancelled(&self) -> bool {
        matches!(self, AgentError::Cancelled(_))
    }

    /// Check if this is a shutdown error (graceful termination)
    pub fn is_shutdown(&self) -> bool {
        matches!(self, AgentError::Shutdown)
    }
}

#[cfg(test)]
#[path = "types_test.rs"]
mod tests;
