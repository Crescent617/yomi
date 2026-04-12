use crate::compactor::Compactor;
use crate::providers::ModelConfig;
use crate::skill::Skill;
use crate::storage::StorageConfig;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Agent configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub model: ModelConfig,
    pub storage: StorageConfig,
    pub max_iterations: usize,
    pub enable_sub_agents: bool,
    pub system_prompt: String,
    #[serde(skip)]
    pub skills: Vec<Arc<Skill>>,
    /// Compactor configuration for context management
    #[serde(skip)]
    pub compactor: Compactor,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig::default(),
            storage: StorageConfig::default(),
            max_iterations: 50,
            enable_sub_agents: true,
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
    Completed,
    Failed,
    Cancelled,
}

impl AgentState {
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    pub const fn valid_transitions(&self) -> &'static [Self] {
        match self {
            Self::Idle => &[Self::WaitingForInput],
            Self::WaitingForInput => &[Self::Streaming, Self::Cancelled],
            Self::Streaming => &[
                Self::ExecutingTool,
                Self::WaitingForInput,
                Self::Failed,
                Self::Cancelled,
            ],
            Self::ExecutingTool => &[Self::Streaming, Self::Failed, Self::Cancelled],
            Self::Completed | Self::Failed | Self::Cancelled => &[],
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
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
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

    pub fn increment_iteration(&self) -> usize {
        self.inner.iteration_count.fetch_add(1, Ordering::SeqCst)
    }

    pub fn iteration_count(&self) -> usize {
        self.inner.iteration_count.load(Ordering::SeqCst)
    }
}

/// Shared resources across agents
#[derive(Clone)]
pub struct AgentShared {
    pub provider: Arc<dyn crate::providers::Provider>,
    pub tool_registry: Arc<crate::tools::ToolRegistry>,
    pub model_config: ModelConfig,
}

impl AgentShared {
    pub fn new(
        provider: Arc<dyn crate::providers::Provider>,
        tool_registry: Arc<crate::tools::ToolRegistry>,
        model_config: ModelConfig,
    ) -> Self {
        Self {
            provider,
            tool_registry,
            model_config,
        }
    }

    #[must_use]
    pub fn with_cloned_registry(&self) -> Self {
        Self {
            provider: Arc::clone(&self.provider),
            tool_registry: Arc::new(self.tool_registry.as_ref().clone()),
            model_config: self.model_config.clone(),
        }
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
        assert!(AgentState::Completed.valid_transitions().is_empty());
        assert!(AgentState::Failed.valid_transitions().is_empty());
        assert!(AgentState::Cancelled.valid_transitions().is_empty());
    }

    #[test]
    fn test_streaming_can_execute_tool_or_finish() {
        assert!(AgentState::Streaming.can_transition_to(AgentState::ExecutingTool));
        assert!(AgentState::Streaming.can_transition_to(AgentState::WaitingForInput));
        assert!(AgentState::Streaming.can_transition_to(AgentState::Failed));
        assert!(!AgentState::Streaming.can_transition_to(AgentState::Completed));
    }

    #[test]
    fn test_executing_tool_transitions() {
        assert!(AgentState::ExecutingTool.can_transition_to(AgentState::Streaming));
        assert!(AgentState::ExecutingTool.can_transition_to(AgentState::Failed));
        assert!(AgentState::ExecutingTool.can_transition_to(AgentState::Cancelled));
        assert!(!AgentState::ExecutingTool.can_transition_to(AgentState::Completed));
    }

    #[test]
    fn test_waiting_for_input_transitions() {
        assert!(AgentState::WaitingForInput.can_transition_to(AgentState::Streaming));
        assert!(AgentState::WaitingForInput.can_transition_to(AgentState::Cancelled));
        assert!(!AgentState::WaitingForInput.can_transition_to(AgentState::ExecutingTool));
    }
}
