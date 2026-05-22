use crate::permissions::Level;
use crate::types::{AgentId, MessageId, SessionId, ToolOutputBlock};
use serde::{Deserialize, Serialize};

/// Top-level event wrapper - modular design prevents enum explosion
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    User(UserEvent),
    Agent(AgentEvent),
    Model(ModelEvent),
    Tool(ToolEvent),
    System(SystemEvent),
}

/// Control command from TUI to kernel
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ty", rename_all = "snake_case")]
pub enum ControlCommand {
    /// Cancel current operation
    Cancel,
    /// Response to a permission request
    Response {
        req_id: String,
        approved: bool,
        remember: bool,
    },
    /// Set permission level (for YOLO mode toggle)
    SetLevel(Level),
    /// Force message compaction
    Compact,
    /// Start autonomous goal-mode execution
    StartGoal(crate::goal::GoalState),
    /// Stop autonomous goal-mode execution
    StopGoal,
    /// Rewind to a specific checkpoint
    Rewind {
        message_id: crate::types::MessageId,
        target: crate::checkpoint::RewindTarget,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserEvent {
    /// User message with multi-modal content blocks
    Message {
        message_id: MessageId,
        content: Vec<crate::types::ContentBlock>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEvent {
    /// Agent 生命周期状态变化
    Lifecycle {
        agent_id: AgentId,
        state: AgentStatus,
    },
    /// Permission request for tool execution approval
    PermissionRequest {
        agent_id: AgentId,
        req_id: String, // independent request ID (not tool_call_id, guarantees uniqueness)
        tool_id: String,
        tool_name: String,
        tool_args: String, // tool arguments (for display, e.g. Bash command)
        tool_level: String,
        reason: String,
    },
    /// Recoverable or non-recoverable operation error
    Error {
        agent_id: AgentId,
        /// Phase where the error occurred
        phase: ErrorPhase,
        /// Error details
        error: String,
        /// Whether the error is recoverable (will be retried)
        is_recoverable: bool,
    },
    /// Currently retrying
    Retrying {
        agent_id: AgentId,
        attempt: u32,
        max_attempts: u32,
        reason: String,
    },
}

/// Agent lifecycle state change (business-level, distinct from internal `AgentState`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    /// Agent started running
    Running,
    /// Task completed naturally (was `ReActLoopEnd`)
    TurnCompleted {
        total_iterations: usize,
        /// Finish reason from the API (e.g. `MaxTokens`, `ContentFilter`)
        finish_reason: Option<crate::types::FinishReason>,
        /// ID of the last assistant message
        last_message_id: Option<MessageId>,
    },
    /// Agent stopped (includes various end reasons)
    Stopped { reason: StopReason },
}

/// Reasons why the Agent stopped
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// Normal completion
    Completed,
    /// User cancelled
    Cancelled {
        /// Name of the cancelled operation (e.g. "streaming", "compaction")
        operation: Option<String>,
    },
    /// Execution failed
    Failed { error: String },
    /// Reached maximum iterations
    MaxIterations { reached: usize },
}

/// Agent execution phase, used for error reporting
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorPhase {
    Streaming,
    ToolExecution,
    Compaction,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModelEvent {
    Request {
        agent_id: AgentId,
        message_id: MessageId,
        message_count: usize,
    },
    /// Content chunk (text or thinking)
    Chunk {
        agent_id: AgentId,
        message_id: MessageId,
        content: ContentChunk,
    },
    /// Incremental tool call update (for UI feedback during argument streaming)
    /// Only contains the newly added fragment, not the accumulated arguments.
    ToolCallDelta {
        agent_id: AgentId,
        message_id: MessageId,
        tool_id: String,
        tool_name: String,
        /// Newly added argument fragment (delta), not the full accumulated string
        arguments_delta: String,
    },
    Completed {
        agent_id: AgentId,
        message_id: MessageId,
    },
    Error {
        agent_id: AgentId,
        message_id: MessageId,
        error: String,
    },
    Fallback {
        agent_id: AgentId,
        message_id: MessageId,
        from: String,
        to: String,
    },
    /// Token usage update from provider
    TokenUsage {
        agent_id: AgentId,
        message_id: MessageId,
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
        /// Context window size (max tokens)
        context_window: u32,
    },
    /// Context compaction in progress
    Compacting { agent_id: AgentId, active: bool },
}

/// Content chunk for streaming
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentChunk {
    Text(String),
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    RedactedThinking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolEvent {
    Start {
        agent_id: AgentId,
        message_id: MessageId,
        tool_id: String,
        tool_name: String,
        arguments: Option<String>,
    },
    End {
        agent_id: AgentId,
        message_id: MessageId,
        tool_id: String,
        tool_name: String,
        /// Content blocks for multimodal support (text, images, etc.)
        content_blocks: Vec<ToolOutputBlock>,
        elapsed_ms: u64,
        /// Whether this output represents an error
        is_error: bool,
    },
    /// Progress update for long-running tools (e.g., sub-agent)
    Progress {
        agent_id: AgentId,
        message_id: MessageId,
        tool_id: String,
        /// Progress message (e.g., "iteration 3/20", "streaming...")
        message: String,
        /// Optional total token count
        tokens: Option<u32>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SystemEvent {
    /// Session shutdown (main agent ended)
    Shutdown {
        session_id: SessionId,
        /// Error message if session exited with an error
        error: Option<String>,
    },
    /// Session rewound to a checkpoint
    Rewound {
        session_id: SessionId,
        /// Updated messages after rewind (truncated history)
        messages: Vec<std::sync::Arc<crate::types::Message>>,
    },
    /// Connection to daemon is active (initial connect or after recovery)
    Connected {
        session_id: SessionId,
    },
    /// Connection to daemon was lost (reader/heartbeat detected an error)
    ConnectionLost {
        session_id: SessionId,
    },
}
