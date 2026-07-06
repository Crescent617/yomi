use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use ulid::Ulid;

// ─── Prefix constants ───────────────────────────────────────────────────────

pub const SESS_PREFIX: &str = "sess_";
pub const SUB_PREFIX: &str = "sub_";
pub const PROJ_PREFIX: &str = "proj_";
pub const MSG_PREFIX: &str = "msg_";
pub const CRON_PREFIX: &str = "cron_";
pub const EVT_PREFIX: &str = "evt_";

// ─── Macro: generate a distinct newtype for each ID ───────────────────────

macro_rules! define_id {
    (
        $(#[$meta:meta])*
        $name:ident => $prefix:literal
    ) => {
        $(#[$meta])*
        #[allow(clippy::unsafe_derive_deserialize)]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        #[repr(transparent)]
        pub struct $name(pub SmolStr);

        impl $name {
            pub const PREFIX: &'static str = $prefix;

            pub fn new() -> Self {
                thread_local! {
                    static GEN: std::cell::RefCell<ulid::Generator> = std::cell::RefCell::new(ulid::Generator::new());
                }
                let ulid = GEN.with(|g| g.borrow_mut().generate().unwrap());
                Self(SmolStr::new(format!("{}{}", Self::PREFIX, ulid)))
            }

            pub fn as_str(&self) -> &str {
                self.0.as_str()
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::ops::Deref for $name {
            type Target = str;
            fn deref(&self) -> &str {
                self.0.as_str()
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(SmolStr::new(s))
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(SmolStr::new(s))
            }
        }

        impl From<SmolStr> for $name {
            fn from(s: SmolStr) -> Self {
                Self(s)
            }
        }

        impl From<&SmolStr> for $name {
            fn from(s: &SmolStr) -> Self {
                Self(s.clone())
            }
        }
    };
}

// ─── Generate all ID types ──────────────────────────────────────────────

define_id!(SessionId => "sess_");
define_id!(ProjectId => "proj_");
define_id!(MessageId => "msg_");
define_id!(CronJobId => "cron_");
define_id!(EventId => "evt_");

// ─── Specialised extensions ─────────────────────────────────────────────

impl SessionId {
    pub fn new_subagent() -> Self {
        Self(SmolStr::new(format!("sub_{}", Ulid::new())))
    }

    pub fn is_subagent(&self) -> bool {
        self.0.starts_with("sub_")
    }
}

impl ProjectId {
    pub fn default_workspace() -> Self {
        Self(SmolStr::new("00000000-0000-0000-0000-000000000000")) // for compatibility
    }
}

/// Project entity
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Project {
    pub id: ProjectId,
    pub name: String,
    pub dir: std::path::PathBuf,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

/// Session response with metadata and runtime status
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SessionResponse {
    pub id: SessionId,
    pub phase: String,
    pub title: Option<String>,
    pub parent_id: Option<SessionId>,
    pub project_id: Option<ProjectId>,
    pub working_dir: Option<String>,
    pub message_count: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub auto_approve_level: Option<String>,
}

impl Default for SessionResponse {
    fn default() -> Self {
        Self {
            id: SessionId::new(),
            phase: "idle".to_string(),
            title: None,
            parent_id: None,
            project_id: None,
            working_dir: None,
            message_count: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            auto_approve_level: None,
        }
    }
}

/// Message role
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
    Internal,
}

/// Finish reason - normalized across providers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinishReason {
    /// Normal completion (`OpenAI`: "stop", Anthropic: "`end_turn`")
    Stop,
    /// Max tokens reached (`OpenAI`: "length", Anthropic: "`max_tokens`")
    MaxTokens,
    /// Content filter triggered (`OpenAI`: "`content_filter`")
    ContentFilter,
    /// `ToolCall` finished (custom reason for tool calls)
    ToolCalls,
    /// `Unknown` finish reason
    Unknown,
}

impl FinishReason {
    /// Parse from provider-specific string
    pub fn from_provider_str(s: &str) -> Option<Self> {
        match s {
            "" => {
                tracing::warn!("empty finish_reason");
                None
            }
            "length" | "max_tokens" => Some(Self::MaxTokens), // length is used by OpenAI, max_tokens by Anthropic
            "content_filter" => Some(Self::ContentFilter),
            "tool_calls" | "tool_use" => Some(Self::ToolCalls), // Custom reasons for tool calls
            "stop" | "end_turn" => Some(Self::Stop),
            _ => {
                tracing::warn!("unknown finish_reason {s}");
                Some(Self::Unknown)
            }
        }
    }
}

/// Content block - similar to `OpenAI`'s content format
/// Supports text, thinking, images, etc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text content
    Text { text: String },
    /// Model's thinking/reasoning process (shown in UI but not sent back to model)
    Thinking {
        thinking: String,
        signature: Option<String>,
    },
    /// Redacted thinking (for Claude 3.7 Sonnet)
    RedactedThinking { data: String },
    /// Image URL or base64 data
    ImageUrl { image_url: ImageUrl },
    /// Audio content
    Audio { audio: AudioData },
}

impl ContentBlock {
    /// Get text content if this is a text block
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Get thinking content if this is a thinking block
    pub fn as_thinking(&self) -> Option<&str> {
        match self {
            Self::Thinking { thinking, .. } => Some(thinking),
            _ => None,
        }
    }

    /// Check if this is a text block
    pub const fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    /// Check if this is a thinking block
    pub const fn is_thinking(&self) -> bool {
        matches!(self, Self::Thinking { .. })
    }
}

impl From<String> for ContentBlock {
    fn from(text: String) -> Self {
        Self::Text { text }
    }
}

impl From<&str> for ContentBlock {
    fn from(text: &str) -> Self {
        Self::Text {
            text: text.to_string(),
        }
    }
}

/// Image URL structure
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>, // auto, low, high
}

impl From<String> for ImageUrl {
    fn from(url: String) -> Self {
        Self {
            url,
            detail: Some("auto".to_string()),
        }
    }
}

/// Audio data structure
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioData {
    pub data: String,   // base64 encoded
    pub format: String, // mp3, wav, etc.
}

/// Token usage for a message (from API response)
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageTokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

/// Chat message with content blocks (OpenAI-style)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    #[serde(default)]
    pub id: MessageId,
    pub role: Role,
    /// Content blocks - can be single string (simple) or array of blocks (rich)
    /// For serialization, we use a custom format that handles both
    #[serde(with = "content_serde")]
    pub content: Vec<ContentBlock>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    pub created_at: DateTime<Utc>,
    /// Token usage for this message (from API response, only set for assistant messages)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_usage: Option<MessageTokenUsage>,
    /// API response ID (e.g., "chatcmpl-xxx" or "`msg_xxx`", only set for assistant messages from API)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_id: Option<String>,
    /// Finish/stop reason from API response (normalized across providers)
    /// Only set for assistant messages from API
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<FinishReason>,
    /// Internal metadata for UI display (not sent to model).
    /// E.g., {"`subagent_session_id"`: "sess-xxx"} for subagent tool.
    #[serde(skip_serializing_if = "Option::is_none", rename = "_meta")]
    pub metadata: Option<std::collections::HashMap<String, String>>,
}

impl Default for Message {
    fn default() -> Self {
        Self {
            id: MessageId::new(),
            role: Role::User,
            content: Vec::new(),
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
            response_id: None,
            finish_reason: None,
            metadata: None,
        }
    }
}

impl Message {
    /// Create a system message with text content
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![content.into().into()],
            ..Default::default()
        }
    }

    /// Create a user message with text content
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![content.into().into()],
            ..Default::default()
        }
    }

    /// Create a user message with image
    pub fn user_with_image(text: impl Into<String>, image_url: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![
                ContentBlock::Text { text: text.into() },
                ContentBlock::ImageUrl {
                    image_url: ImageUrl {
                        url: image_url.into(),
                        detail: None,
                    },
                },
            ],
            ..Default::default()
        }
    }

    /// Create an assistant message with text
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![content.into().into()],
            ..Default::default()
        }
    }

    /// Create an assistant message with thinking
    pub fn assistant_with_thinking(text: impl Into<String>, thinking: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking {
                    thinking: thinking.into(),
                    signature: None,
                },
                ContentBlock::Text { text: text.into() },
            ],
            ..Default::default()
        }
    }

    /// Create a message with multiple content blocks
    pub fn with_blocks(role: Role, blocks: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content: blocks,
            ..Default::default()
        }
    }

    /// Get all text content concatenated
    pub fn text_content(&self) -> String {
        self.content
            .iter()
            .filter_map(|block| block.as_text())
            .collect::<Vec<_>>()
            .join("")
    }

    /// Get thinking content if any
    pub fn thinking_content(&self) -> Option<String> {
        let thinking: Vec<_> = self
            .content
            .iter()
            .filter_map(|block| block.as_thinking())
            .collect();
        if thinking.is_empty() {
            None
        } else {
            Some(thinking.join(""))
        }
    }

    /// Add a content block
    pub fn add_block(&mut self, block: ContentBlock) {
        self.content.push(block);
    }

    /// Append text to the last text block, or create new one
    pub fn append_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if let Some(ContentBlock::Text { text: existing }) = self.content.last_mut() {
            existing.push_str(&text);
        } else {
            self.content.push(ContentBlock::Text { text });
        }
    }

    /// Create a tool result message with a pre-assigned identifier.
    pub fn tool_result(
        message_id: MessageId,
        tool_call_id: impl Into<String>,
        output: impl Into<String>,
    ) -> Self {
        Self {
            id: message_id,
            role: Role::Tool,
            content: vec![output.into().into()],
            tool_call_id: Some(tool_call_id.into()),
            ..Default::default()
        }
    }

    /// Set the `tool_call_id` for this message (builder pattern)
    #[must_use]
    pub fn with_tool_call_id(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
        self
    }

    /// Set token usage for this message (builder pattern, for assistant messages)
    #[must_use]
    pub fn with_token_usage(mut self, usage: MessageTokenUsage) -> Self {
        self.token_usage = Some(usage);
        self
    }

    /// Set the API response ID for this message (builder pattern, for assistant messages)
    #[must_use]
    pub fn with_response_id(mut self, response_id: impl Into<String>) -> Self {
        self.response_id = Some(response_id.into());
        self
    }
}

/// Custom serialization for content to support both string and array formats
mod content_serde {
    use super::ContentBlock;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S>(content: &[ContentBlock], serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // If single text block, serialize as string for compatibility
        if content.len() == 1 {
            if let ContentBlock::Text { text } = &content[0] {
                return serializer.serialize_str(text);
            }
        }
        // Otherwise serialize as array
        content.serialize(serializer)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<ContentBlock>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;

        // Handle string format
        if let Some(s) = value.as_str() {
            return Ok(vec![ContentBlock::Text {
                text: s.to_string(),
            }]);
        }

        // Handle array format
        if let Some(arr) = value.as_array() {
            let blocks: Vec<ContentBlock> = arr
                .iter()
                .map(|v| serde_json::from_value(v.clone()).map_err(serde::de::Error::custom))
                .collect::<Result<Vec<_>, _>>()?;
            return Ok(blocks);
        }

        Err(serde::de::Error::custom(
            "expected string or array of content blocks",
        ))
    }
}

/// Tool call from model
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Tool output block - represents a piece of tool output (text or image)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToolOutputBlock {
    /// Plain text content
    Text { text: String },
    /// Image content (base64 data URL or regular URL)
    Image {
        url: String,
        mime_type: Option<String>,
    },
}

impl ToolOutputBlock {
    /// Get text content if this is a text block
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Image { .. } => None,
        }
    }

    /// Check if this is a text block
    pub const fn is_text(&self) -> bool {
        matches!(self, Self::Text { .. })
    }

    /// Check if this is an image block
    pub const fn is_image(&self) -> bool {
        matches!(self, Self::Image { .. })
    }
}

impl From<String> for ToolOutputBlock {
    fn from(text: String) -> Self {
        Self::Text { text }
    }
}

impl From<&str> for ToolOutputBlock {
    fn from(text: &str) -> Self {
        Self::Text {
            text: text.to_string(),
        }
    }
}

/// Tool output - supports multimodal content (text + images)
#[derive(Debug, Clone, Default)]
pub struct ToolOutput {
    pub contents: Vec<ToolOutputBlock>,
    pub is_error: bool,
}

impl ToolOutput {
    /// Create a new tool output with text content
    /// If summary is non-empty, it will be prepended to the text
    pub fn text_with_summary(text: impl Into<String>, summary: impl Into<String>) -> Self {
        let summary = summary.into();
        let text = text.into();
        let content = if summary.is_empty() {
            text
        } else {
            format!("{summary}\n{text}")
        };
        Self {
            contents: vec![ToolOutputBlock::Text { text: content }],
            is_error: false,
        }
    }

    /// Create a tool output with just text (simplified API)
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            contents: vec![ToolOutputBlock::Text { text: text.into() }],
            is_error: false,
        }
    }

    /// Create an error output with text
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            contents: vec![ToolOutputBlock::Text { text: text.into() }],
            is_error: true,
        }
    }

    /// Create an output with an image
    pub fn image(url: impl Into<String>) -> Self {
        Self {
            contents: vec![ToolOutputBlock::Image {
                url: url.into(),
                mime_type: None,
            }],
            is_error: false,
        }
    }

    /// Create an output with image and text
    pub fn with_image_and_text(url: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            contents: vec![
                ToolOutputBlock::Image {
                    url: url.into(),
                    mime_type: None,
                },
                ToolOutputBlock::Text { text: text.into() },
            ],
            is_error: false,
        }
    }

    /// Check if this output represents an error
    pub const fn success(&self) -> bool {
        !self.is_error
    }

    /// Get all text content concatenated (for backward compatibility)
    pub fn text_content(&self) -> String {
        self.contents
            .iter()
            .filter_map(|block| block.as_text())
            .collect()
    }

    /// Get all text content for error display
    pub fn error_text(&self) -> String {
        self.text_content()
    }

    /// Add a content block
    pub fn add_block(&mut self, block: ToolOutputBlock) {
        self.contents.push(block);
    }

    /// Append text to the output
    pub fn append_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        if let Some(ToolOutputBlock::Text { text: existing }) = self.contents.last_mut() {
            existing.push_str(&text);
        } else {
            self.contents.push(ToolOutputBlock::Text { text });
        }
    }
}

/// Tool definition for model
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Specific session-level error variants.
#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionError {
    /// The requested session does not exist in memory or storage.
    #[error("session_not_found: Session {session_id} not found")]
    NotFound { session_id: String },
    /// The session exists but its inner agent handle is not initialized.
    #[error("Session not initialized")]
    NotInitialized,
    /// Attempted to create/restore a session that is already alive.
    #[error("session_already_exists: Session {session_id} already initialized")]
    AlreadyExists { session_id: String },
    /// Message store is required but not configured.
    #[error("message store not configured")]
    StoreNotConfigured,
    /// Connection to the daemon was lost.
    #[error("Connection lost during operation")]
    ConnectionLost,
    /// RPC request timed out.
    #[error("RPC request timed out (30s)")]
    RequestTimeout,
    /// Failed to send a request across the wire.
    #[error("Failed to send request: {0}")]
    SendFailed(String),
    /// Rewind operation failed.
    #[error("Rewind failed: {0}")]
    RewindFailed(String),
    /// Request was cancelled.
    #[error("Request cancelled")]
    Cancelled,
    /// Catch-all for other session errors (migration fallback).
    #[error("{0}")]
    Other(String),
    /// Wire protocol version mismatch between client and daemon.
    #[error("daemon wire protocol too old, please upgrade and restart daemon")]
    WireProtocolMismatch,
}

/// Core error type for kernel operations
#[derive(thiserror::Error, Debug, Clone)]
pub enum KernelError {
    /// I/O operation failed
    #[error("IO error: {0}")]
    Io(String),

    /// Storage operation failed
    #[error("Storage error: {0}")]
    Storage(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    Config(String),

    /// Tool execution error
    #[error("Tool error: {0}")]
    Tool(String),

    /// Serialization/deserialization error
    #[error("Serialization error: {0}")]
    Serde(String),

    /// Permission denied
    #[error("Permission denied: {0}")]
    Permission(String),

    /// Session not found or invalid
    #[error(transparent)]
    Session(#[from] SessionError),

    /// Task operation failed
    #[error("Task error: {0}")]
    Task(String),

    /// Skill loading/parsing error
    #[error("Skill error: {0}")]
    Skill(String),

    /// Cancellation error
    #[error("Cancelled: {0}")]
    Cancelled(String),

    /// Checkpoint/rewind error
    #[error("Checkpoint error: {0}")]
    Checkpoint(String),

    /// Agent execution error (nested for retry/cancellation checks)
    #[error("Agent error: {0}")]
    Agent(#[source] crate::agent::AgentError),
}

impl KernelError {
    /// Create a new I/O error
    pub fn io(msg: impl Into<String>) -> Self {
        Self::Io(msg.into())
    }

    /// Create a new storage error
    pub fn storage(msg: impl Into<String>) -> Self {
        Self::Storage(msg.into())
    }

    /// Create a new configuration error
    pub fn config(msg: impl Into<String>) -> Self {
        Self::Config(msg.into())
    }

    /// Create a new tool error
    pub fn tool(msg: impl Into<String>) -> Self {
        Self::Tool(msg.into())
    }

    /// Create a new serialization error
    pub fn serde(msg: impl Into<String>) -> Self {
        Self::Serde(msg.into())
    }

    /// Create a new permission error
    pub fn permission(msg: impl Into<String>) -> Self {
        Self::Permission(msg.into())
    }

    /// Create a generic session error (fallback).
    pub fn session(msg: impl Into<String>) -> Self {
        Self::Session(SessionError::Other(msg.into()))
    }

    /// Create a new task error
    pub fn task(msg: impl Into<String>) -> Self {
        Self::Task(msg.into())
    }

    /// Create a new skill error
    pub fn skill(msg: impl Into<String>) -> Self {
        Self::Skill(msg.into())
    }

    /// Create a new cancellation error
    pub fn cancelled(msg: impl Into<String>) -> Self {
        Self::Cancelled(msg.into())
    }

    /// Create a new checkpoint error
    pub fn checkpoint(msg: impl Into<String>) -> Self {
        Self::Checkpoint(msg.into())
    }

    /// Check if this is a "session not found" error.
    pub fn is_session_not_found(&self) -> bool {
        matches!(self, Self::Session(SessionError::NotFound { .. }))
    }

    /// Check if this is a "session already exists" error.
    pub fn is_session_already_exists(&self) -> bool {
        matches!(self, Self::Session(SessionError::AlreadyExists { .. }))
    }

    /// Check if this is a cancellation error
    pub fn is_cancelled(&self) -> bool {
        match self {
            Self::Cancelled(_) => true,
            Self::Agent(e) => e.is_cancelled(),
            _ => false,
        }
    }
}

impl From<crate::tools::helper::GLockError> for KernelError {
    fn from(e: crate::tools::helper::GLockError) -> Self {
        Self::Tool(e.to_string())
    }
}

impl From<std::io::Error> for KernelError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for KernelError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e.to_string())
    }
}

impl From<toml::de::Error> for KernelError {
    fn from(e: toml::de::Error) -> Self {
        Self::Serde(e.to_string())
    }
}

impl From<serde_yaml::Error> for KernelError {
    fn from(e: serde_yaml::Error) -> Self {
        Self::Serde(e.to_string())
    }
}

impl From<crate::agent::AgentError> for KernelError {
    fn from(e: crate::agent::AgentError) -> Self {
        Self::Agent(e)
    }
}

impl From<sqlx::Error> for KernelError {
    fn from(e: sqlx::Error) -> Self {
        Self::Storage(e.to_string())
    }
}

impl From<chrono::ParseError> for KernelError {
    fn from(e: chrono::ParseError) -> Self {
        Self::Serde(e.to_string())
    }
}

impl From<std::num::ParseIntError> for KernelError {
    fn from(e: std::num::ParseIntError) -> Self {
        Self::Serde(e.to_string())
    }
}

impl From<reqwest::Error> for KernelError {
    fn from(e: reqwest::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Result type alias for kernel operations
pub type Result<T> = std::result::Result<T, KernelError>;
