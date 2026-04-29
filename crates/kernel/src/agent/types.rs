use crate::compactor::Compactor;
use crate::providers::{ModelConfig, ProviderError};
use crate::skill::Skill;
use crate::types::Message;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use thiserror::Error;

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub model: ModelConfig,
    pub max_iterations: usize,
    pub enable_subagent: bool,
    pub system_prompt: String,
    #[serde(skip)]
    pub skills: Vec<Arc<Skill>>,
    /// Compactor configuration for context management
    pub compactor: Compactor,
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
    pub enable_sub_agents: bool,
    pub working_dir: std::path::PathBuf,
    /// Optional cancel token to share with parent (for cascading cancellation)
    pub cancel_token: Option<super::CancelToken>,
    /// Optional file state store (for restoring from previous session)
    pub file_state_store: Option<Arc<crate::tools::file_state::FileStateStore>>,
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
            .field("enable_sub_agents", &self.enable_sub_agents)
            .field("working_dir", &self.working_dir)
            .field("cancel_token", &self.cancel_token.is_some())
            .field("file_state_store", &self.file_state_store.is_some())
            .finish()
    }
}

impl AgentSpawnArgs {
    /// Create a new config with the given base prompt and session
    pub fn new(base_prompt: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            base_prompt: base_prompt.into(),
            skills: Vec::new(),
            history: Vec::<Arc<Message>>::new(),
            session_id: session_id.into(),
            parent_session_id: None,
            max_iterations: 100,
            enable_sub_agents: true,
            working_dir: std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from(".")),
            cancel_token: None,
            file_state_store: None,
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
        self.enable_sub_agents = enabled;
        self
    }

    /// Set working directory
    #[must_use]
    pub fn with_working_dir(mut self, dir: impl Into<std::path::PathBuf>) -> Self {
        self.working_dir = dir.into();
        self
    }

    /// Set cancel token to share with parent (for cascading cancellation)
    #[must_use]
    pub fn with_cancel_token(mut self, token: super::CancelToken) -> Self {
        self.cancel_token = Some(token);
        self
    }

    /// Set file state store (for restoring from previous session)
    #[must_use]
    pub fn with_file_state_store(
        mut self,
        store: Arc<crate::tools::file_state::FileStateStore>,
    ) -> Self {
        self.file_state_store = Some(store);
        self
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig::default(),
            max_iterations: 100,
            enable_subagent: true,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            skills: Vec::new(),
            compactor: Compactor::default(),
        }
    }
}

/// Default system prompt for the agent
const DEFAULT_SYSTEM_PROMPT: &str = r"You are Yomi, a helpful assistant.

# System
- All text you output outside of tool use is displayed to the user. Output text to communicate with the user.

# Doing Tasks
- The user will primarily request software engineering tasks: solving bugs, adding functionality, refactoring, explaining code, etc. When given an unclear instruction, consider it in this context.
- In general, do not propose changes to code you haven't read. If a user asks about or wants to modify a file, read it first. Understand existing code before suggesting modifications.
- Do not create files unless absolutely necessary. Prefer editing existing files to creating new ones.
- If an approach fails, diagnose why before switching tactics—read the error, check your assumptions, try a focused fix. Escalate to the user only when genuinely stuck after investigation.
- Be careful not to introduce security vulnerabilities (command injection, XSS, SQL injection, etc.). Prioritize writing safe, secure, and correct code.

# Executing Actions
Carefully consider reversibility and blast radius:
- Local, reversible actions (editing files, running tests): proceed freely
- Destructive operations (deleting files/branches, rm -rf, overwriting uncommitted changes): confirm first
- Actions visible to others (pushing code, creating PRs/issues): confirm first
- Measure twice, cut once. Only take risky actions carefully, and when in doubt, ask before acting.

# Tone and Style
- Your responses should be short and concise.
";

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    Idle,
    WaitingForInput,
    Streaming,
    ExecutingTool,
    Closed,
}

impl AgentState {
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Closed)
    }

    pub const fn valid_transitions(&self) -> &'static [Self] {
        match self {
            Self::Idle => &[Self::WaitingForInput],
            Self::WaitingForInput => &[Self::Streaming, Self::Closed],
            Self::Streaming => &[Self::ExecutingTool, Self::WaitingForInput],
            Self::ExecutingTool => &[Self::Streaming],
            Self::Closed => &[],
        }
    }

    pub fn can_transition_to(&self, target: Self) -> bool {
        self.valid_transitions().contains(&target)
    }

    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::WaitingForInput => "waiting_for_input",
            Self::Streaming => "streaming",
            Self::ExecutingTool => "executing_tool",
            Self::Closed => "completed",
        }
    }
}

impl std::fmt::Display for AgentState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Agent execution context for state management
#[derive(Debug, Clone)]
pub struct AgentExecutionContext {
    inner: Arc<AgentExecutionContextInner>,
}

#[derive(Debug)]
struct AgentExecutionContextInner {
    state_tx: tokio::sync::watch::Sender<AgentState>,
    iteration_count: AtomicUsize,
}

impl AgentExecutionContext {
    pub fn new(initial_state: AgentState) -> (Self, tokio::sync::watch::Receiver<AgentState>) {
        let (state_tx, state_rx) = tokio::sync::watch::channel(initial_state);
        let ctx = Self {
            inner: Arc::new(AgentExecutionContextInner {
                state_tx,
                iteration_count: AtomicUsize::new(0),
            }),
        };
        (ctx, state_rx)
    }

    pub fn transition_to(&self, new_state: AgentState) -> bool {
        let current = *self.inner.state_tx.borrow();
        if !current.can_transition_to(new_state) {
            tracing::warn!("Invalid state transition: {:?} -> {:?}", current, new_state);
            return false;
        }
        self.inner.state_tx.send_replace(new_state);
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
    pub provider: Arc<dyn crate::providers::Provider>,
    pub model_config: Arc<ModelConfig>,
    /// Task store for task tools (legacy - replaced by `todo_store`)
    pub task_store: Option<Arc<crate::task::TaskStore>>,
    /// Todo store for lightweight todo tracking
    pub todo_store: Option<crate::tools::SharedTodoStore>,
    /// Project memory (CLAUDE.md/AGENTS.md)
    pub project_memory: Arc<crate::project_memory::MemoryFiles>,
    /// Context compactor for managing long conversations
    pub compactor: Option<crate::compactor::Compactor>,
    /// Storage for message persistence
    pub storage: Option<Arc<dyn crate::storage::Storage>>,
    /// Shared permission state for all agents in a session
    pub permission_state: Option<crate::permissions::PermissionState>,
    /// Skill folders for the `skill_load` tool
    pub skill_folders: Vec<std::path::PathBuf>,
}

impl AgentShared {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn crate::providers::Provider>,
        model_config: Arc<ModelConfig>,
        task_store: Option<Arc<crate::task::TaskStore>>,
        project_memory: Arc<crate::project_memory::MemoryFiles>,
        compactor: Option<crate::compactor::Compactor>,
        storage: Option<Arc<dyn crate::storage::Storage>>,
        permission_state: Option<crate::permissions::PermissionState>,
        skill_folders: Vec<std::path::PathBuf>,
    ) -> Self {
        Self {
            provider,
            model_config,
            task_store,
            todo_store: Some(crate::tools::SharedTodoStore::default()),
            project_memory,
            compactor,
            storage,
            permission_state,
            skill_folders,
        }
    }
}

/// Agent error type using thiserror
#[derive(Error, Debug)]
pub enum AgentError {
    /// Agent reached maximum iterations
    #[error("Agent reached maximum iterations: {count}")]
    MaxIterationsExceeded { count: usize },

    /// Cancelled is a terminal error - agent was cancelled by user or parent
    #[error("Agent was cancelled")]
    Cancelled,

    /// Input channel closed unexpectedly
    #[error("Input channel closed")]
    ChannelClosed,

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
}

impl AgentError {
    pub fn is_retryable(&self) -> bool {
        use AgentError::{
            Cancelled, ChannelClosed, MaxIterationsExceeded, NoPermissionChecker,
            PermissionCheckFailed, Provider, Serialization, StreamTaskPanicked,
        };
        match self {
            // Delegate to ProviderError's retry logic
            Provider(e) => e.is_retryable(),
            // These errors should NOT be retried
            MaxIterationsExceeded { .. }
            | Cancelled
            | ChannelClosed
            | PermissionCheckFailed(_)
            | NoPermissionChecker
            | Serialization(_) => false,
            // Stream task panics might be transient
            StreamTaskPanicked(_) => true,
        }
    }

    /// Check if this is a cancellation error (terminal, not a failure)
    pub fn is_cancelled(&self) -> bool {
        matches!(self, AgentError::Cancelled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_can_only_go_to_waiting() {
        assert!(AgentState::Idle.can_transition_to(AgentState::WaitingForInput));
        assert!(!AgentState::Idle.can_transition_to(AgentState::Streaming));
    }

    #[test]
    fn test_terminal_states_have_no_transitions() {
        assert!(AgentState::Closed.valid_transitions().is_empty());
    }

    #[test]
    fn test_streaming_can_execute_tool_or_finish() {
        assert!(AgentState::Streaming.can_transition_to(AgentState::ExecutingTool));
        assert!(AgentState::Streaming.can_transition_to(AgentState::WaitingForInput));
        assert!(!AgentState::Streaming.can_transition_to(AgentState::Closed));
    }

    #[test]
    fn test_executing_tool_transitions() {
        assert!(AgentState::ExecutingTool.can_transition_to(AgentState::Streaming));
        assert!(!AgentState::ExecutingTool.can_transition_to(AgentState::Closed));
    }

    #[test]
    fn test_waiting_for_input_transitions() {
        assert!(AgentState::WaitingForInput.can_transition_to(AgentState::Streaming));
        assert!(!AgentState::WaitingForInput.can_transition_to(AgentState::ExecutingTool));
    }
}
