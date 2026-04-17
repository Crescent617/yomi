use crate::permissions::Level;
use crate::types::{AgentId, Message, SessionId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

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
    Message { content: String },
    Confirm { tool_id: String, approved: bool },
    Interrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEvent {
    Completed {
        agent_id: AgentId,
        result: AgentResult,
    },
    Failed {
        agent_id: AgentId,
        error: String,
    },
    Cancelled {
        agent_id: AgentId,
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
        output: String,
        elapsed_ms: u64,
    },
    Error {
        agent_id: AgentId,
        tool_id: String,
        error: String,
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
    Shutdown,
    ConfigReloaded,
    SessionForked { from: SessionId, to: SessionId },
}

/// Agent execution result
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentResult {
    pub messages: Vec<Arc<Message>>,
    pub tool_calls: usize,
}

/// Progress update for long-running operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressUpdate {
    pub step: usize,
    pub total: Option<usize>,
    pub message: String,
}
