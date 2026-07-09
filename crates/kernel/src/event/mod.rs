use crate::permission::Level;
use crate::types::{EventId, MessageId, SessionId, ToolOutputBlock};
use serde::{Deserialize, Serialize};

/// Event envelope for wire transmission, includes session ID and event ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Envelope {
    pub session_id: SessionId,
    pub event_id: EventId,
    pub event: Event,
}

impl Envelope {
    /// Create a new envelope with a fresh monotonic event ID.
    pub fn new(session_id: SessionId, event: Event) -> Self {
        Self {
            session_id,
            event_id: EventId::new(),
            event,
        }
    }
}

/// Top-level event wrapper — modular design prevents enum explosion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    User(UserEvent),
    Agent(AgentEvent),
    Internal(InternalEvent),
    Model(ModelEvent),
    Tool(ToolEvent),
}

/// Control command from TUI to kernel
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Command {
    /// Cancel current operation
    Cancel,
    /// Response to a permission request
    Response {
        req_id: String,
        approved: bool,
        remember: bool,
    },
    /// Response to an `ask_user` question
    AskUserResponse {
        req_id: String,
        answers: Vec<(String, String)>,
    },
    /// Set permission level (for YOLO mode toggle)
    SetLevel(Level),
    /// Force message compaction
    Compact,
    /// Start autonomous goal-mode execution
    StartGoal(crate::goal::GoalState),
    /// Stop autonomous goal-mode execution
    StopGoal,
    /// Pause goal auto-continue (agent stops after current turn)
    PauseGoal,
    /// Resume goal auto-continue
    ResumeGoal,
    /// Edit goal description (restarts with updated objective)
    EditGoal { description: String },
    /// Get current goal state (returns JSON-serialized Option<GoalState>)
    GetGoal,
    /// Rewind to a specific checkpoint
    Rewind {
        message_id: crate::types::MessageId,
        target: crate::checkpoint::RewindTarget,
    },
    /// Send a steer message to be injected before the next streaming turn
    Steer {
        content: Vec<crate::types::ContentBlock>,
    },
    /// Trigger the agent to continue from idle to streaming without new user input
    Continue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum UserEvent {
    /// User message with multi-modal content blocks
    Message {
        message_id: MessageId,
        content: Vec<crate::types::ContentBlock>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum AgentEvent {
    /// Agent lifecycle state change (business-level)
    Lifecycle { state: AgentStatus },
    /// Agent internal state machine change (Idle, Streaming, `ExecutingTool`, Compacting, Closed)
    StateChanged { state: crate::agent::AgentState },
    /// Permission request for tool execution approval
    PermissionRequest {
        req_id: String, // independent request ID (not tool_call_id, guarantees uniqueness)
        session_id: String,
        tool_id: String,
        tool_name: String,
        tool_args: String, // tool arguments (for display, e.g. Bash command)
        tool_level: String,
        reason: String,
    },
    /// The user answered an `ask_user` question
    AskUserQuestion {
        req_id: String,
        session_id: String,
        questions: Vec<crate::tools::ask_user::AskQuestion>,
    },
    /// Permission request acknowledged (response received or timeout)
    PermissionAck { req_id: String },
    /// Ask user request acknowledged (response received or timeout)
    AskUserAck { req_id: String },
    Error {
        /// Phase where the error occurred
        phase: ErrorPhase,
        /// Error details
        error: String,
        /// Whether the error is recoverable (will be retried)
        is_recoverable: bool,
    },
    /// Currently retrying
    Retrying {
        attempt: u32,
        max_attempts: u32,
        reason: String,
    },
    /// Messages for a session have been replaced (e.g. after /undo or /clear).
    /// UI should reload messages from store.
    MessageReplaced { session_id: SessionId },
    /// Goal state was updated (started, paused, resumed, completed, blocked)
    GoalUpdated { description: String, status: String },
    /// Goal was stopped and removed
    GoalStopped,
}

/// Internal kernel events for persistence and state management.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum InternalEvent {
    /// A new message was added to the conversation history.
    /// Consumers (e.g. conductor) should persist this to storage.
    MessageAdded {
        message: std::sync::Arc<crate::types::Message>,
    },
    /// Messages were replaced (e.g. compaction or /clear).
    /// Consumers should replace the full persisted history.
    MessageReplaced {
        messages: Vec<std::sync::Arc<crate::types::Message>>,
    },
}

/// Agent lifecycle state change (business-level, distinct from internal `AgentState`)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum AgentStatus {
    /// Agent started running
    Running,
    /// Agent stopped (includes various end reasons)
    Stopped { reason: StopReason },
}

/// Reasons why the Agent stopped
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum StopReason {
    /// Normal completion of a step
    Completed {
        /// Finish reason from the API (e.g. `MaxTokens`, `ContentFilter`)
        finish_reason: Option<crate::types::FinishReason>,
    },
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
#[serde(rename_all = "snake_case")]
pub enum ErrorPhase {
    Streaming,
    ToolExecution,
    Compaction,
    Idle,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum ModelEvent {
    Request {
        message_id: MessageId,
        message_count: usize,
    },
    /// Content chunk (text or thinking)
    Chunk {
        message_id: MessageId,
        content: ContentChunk,
    },
    /// Incremental tool call update (for UI feedback during argument streaming)
    /// Only contains the newly added fragment, not the accumulated arguments.
    ToolCallDelta {
        message_id: MessageId,
        tool_id: String,
        tool_name: String,
        /// Newly added argument fragment (delta), not the full accumulated string
        arguments_delta: String,
    },
    /// Model response fully assembled with all content blocks.
    /// For consumers that need the complete message (e.g. channel reply),
    /// not incremental chunks.
    End {
        message_id: MessageId,
        content: Vec<crate::types::ContentBlock>,
    },
    Error {
        message_id: MessageId,
        error: String,
    },
    Fallback {
        message_id: MessageId,
        from: String,
        to: String,
    },
    /// Token usage update from provider
    TokenUsage {
        message_id: MessageId,
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
        /// Context window size (max tokens)
        context_window: u32,
    },
    /// Context compaction in progress
    Compacting { active: bool },
}

/// Content chunk for streaming
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum ContentChunk {
    Text(String),
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    RedactedThinking,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", rename_all_fields = "snake_case")]
pub enum ToolEvent {
    Start {
        message_id: MessageId,
        tool_id: String,
        tool_name: String,
        arguments: Option<String>,
    },
    Metadata {
        message_id: MessageId,
        tool_id: String,
        metadata: std::collections::HashMap<String, String>,
    },
    End {
        message_id: MessageId,
        tool_id: String,
        tool_name: String,
        /// Content blocks for multimodal support (text, images, etc.)
        content_blocks: Vec<ToolOutputBlock>,
        elapsed_ms: u64,
        /// Whether this output represents an error
        is_error: bool,
    },
}
