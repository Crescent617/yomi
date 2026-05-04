use crate::permissions::Level;
use crate::types::{AgentId, SessionId, ToolOutputBlock};
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
#[derive(Debug, Clone, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserEvent {
    /// User message with multi-modal content blocks
    Message {
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
        req_id: String, // 独立请求ID（非tool_call_id，保证唯一）
        tool_id: String,
        tool_name: String,
        tool_args: String, // 工具参数（用于显示，如 Bash 命令）
        tool_level: String,
        reason: String,
    },
    /// 操作错误（可恢复或不可恢复）
    Error {
        agent_id: AgentId,
        /// 错误发生的阶段
        phase: ErrorPhase,
        /// 错误详情
        error: String,
        /// 是否可恢复（会重试）
        is_recoverable: bool,
    },
    /// 正在重试
    Retrying {
        agent_id: AgentId,
        attempt: u32,
        max_attempts: u32,
        reason: String,
    },
}

/// Agent 生命周期状态（业务层面，区别于内部执行状态 `AgentState`）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    /// Agent 开始运行
    Running,
    /// 一次 `ReAct` 迭代完成（原 Completed）
    IterationCompleted { iteration: usize, messages: usize },
    /// 任务自然结束（原 `ReActLoopEnd`）
    TurnCompleted {
        total_iterations: usize,
        /// API 返回的 finish reason（如 `MaxTokens`, `ContentFilter`）
        finish_reason: Option<crate::types::FinishReason>,
    },
    /// Agent 停止（包含各种结束原因）
    Stopped { reason: StopReason },
}

/// Agent 停止原因
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopReason {
    /// 正常完成
    Completed,
    /// 用户取消
    Cancelled {
        /// 被取消的操作名称（如 "streaming", "compaction"）
        operation: Option<String>,
    },
    /// 执行失败
    Failed { error: String },
    /// 达到最大迭代次数
    MaxIterations { reached: usize },
}

/// Agent 执行阶段，用于错误报告
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
        message_count: usize,
    },
    /// Content chunk (text or thinking)
    Chunk {
        agent_id: AgentId,
        content: ContentChunk,
    },
    /// Incremental tool call update (for UI feedback during argument streaming)
    /// Only contains the newly added fragment, not the accumulated arguments.
    ToolCallDelta {
        agent_id: AgentId,
        tool_id: String,
        tool_name: String,
        /// Newly added argument fragment (delta), not the full accumulated string
        arguments_delta: String,
    },
    Completed {
        agent_id: AgentId,
    },
    Error {
        agent_id: AgentId,
        error: String,
    },
    Fallback {
        agent_id: AgentId,
        from: String,
        to: String,
    },
    /// Token usage update from provider
    TokenUsage {
        agent_id: AgentId,
        prompt_tokens: u32,
        completion_tokens: u32,
        total_tokens: u32,
        /// Context window size (max tokens)
        context_window: u32,
    },
    /// Context compaction in progress
    Compacting {
        agent_id: AgentId,
        active: bool,
    },
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
    Started {
        agent_id: AgentId,
        tool_id: String,
        tool_name: String,
        arguments: Option<String>,
    },
    Output {
        agent_id: AgentId,
        tool_id: String,
        /// Text output for backward compatibility
        output: String,
        /// Content blocks for multimodal support (images, etc.)
        content_blocks: Vec<ToolOutputBlock>,
        elapsed_ms: u64,
    },
    Error {
        agent_id: AgentId,
        tool_id: String,
        /// Text error for backward compatibility
        error: String,
        /// Content blocks for multimodal support (e.g., screenshots of errors)
        content_blocks: Vec<ToolOutputBlock>,
        elapsed_ms: u64,
    },
    /// Progress update for long-running tools (e.g., sub-agent)
    Progress {
        agent_id: AgentId,
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
}
