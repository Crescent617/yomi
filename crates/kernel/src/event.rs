use crate::types::{AgentId, Message, SessionId};
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum UserEvent {
    Message { content: String },
    Confirm { tool_id: String, approved: bool },
    Interrupt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentEvent {
    Started {
        agent_id: AgentId,
    },
    StateChanged {
        agent_id: AgentId,
        state: String,
    },
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
    SubAgentSpawned {
        parent_id: AgentId,
        child_id: AgentId,
        mode: String,
    },
    Progress {
        agent_id: AgentId,
        update: ProgressUpdate,
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
    Complete {
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
    pub messages: Vec<Message>,
    pub tool_calls: usize,
}

/// Progress update for long-running operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressUpdate {
    pub step: usize,
    pub total: Option<usize>,
    pub message: String,
}
