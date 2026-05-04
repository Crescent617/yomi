use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use smol_str::SmolStr;
use uuid::Uuid;

/// Unique identifier for agents
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(SmolStr);

impl Default for AgentId {
    fn default() -> Self {
        Self::new()
    }
}

impl AgentId {
    pub fn new() -> Self {
        Self(SmolStr::new(Uuid::now_v7().to_string()))
    }

    /// Create from an existing string (used for database retrieval)
    pub fn from_string(s: impl Into<String>) -> Self {
        Self(SmolStr::new(s.into()))
    }

    /// Get the string representation
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for AgentId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for sessions
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string())
    }
}

impl Default for SessionId {
    fn default() -> Self {
        Self::new()
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
}

impl Message {
    /// Create a message with single text content
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::Text {
                text: content.into(),
            }],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
        }
    }

    /// Create a user message with text content
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: content.into(),
            }],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
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
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
        }
    }

    /// Create an assistant message with text
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text {
                text: content.into(),
            }],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
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
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
        }
    }

    /// Create a message with multiple content blocks
    pub fn with_blocks(role: Role, blocks: Vec<ContentBlock>) -> Self {
        Self {
            role,
            content: blocks,
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
            token_usage: None,
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

    /// Create a tool result message
    pub fn tool_result(tool_call_id: impl Into<String>, output: impl Into<String>) -> Self {
        Self {
            role: Role::Tool,
            content: vec![ContentBlock::Text {
                text: output.into(),
            }],
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            created_at: Utc::now(),
            token_usage: None,
        }
    }

    /// Set the `tool_call_id` for this message (builder pattern)
    #[must_use]
    pub fn with_tool_call_id(mut self, tool_call_id: impl Into<String>) -> Self {
        self.tool_call_id = Some(tool_call_id.into());
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
                .filter_map(|v| serde_json::from_value(v.clone()).ok())
                .collect();
            return Ok(blocks);
        }

        Ok(vec![])
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
#[derive(Debug, Clone)]
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

// Backward compatibility: implement Deref to allow .stdout access in tests
#[deprecated(note = "ToolOutput fields have changed, use text_content() instead")]
pub struct ToolOutputCompat {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

/// Tool definition for model
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
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
    #[error("Session error: {0}")]
    Session(String),

    /// Task operation failed
    #[error("Task error: {0}")]
    Task(String),

    /// Skill loading/parsing error
    #[error("Skill error: {0}")]
    Skill(String),

    /// Plugin error
    #[error("Plugin error: {0}")]
    Plugin(String),

    /// Cancellation error
    #[error("Cancelled: {0}")]
    Cancelled(String),

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

    /// Create a new session error
    pub fn session(msg: impl Into<String>) -> Self {
        Self::Session(msg.into())
    }

    /// Create a new task error
    pub fn task(msg: impl Into<String>) -> Self {
        Self::Task(msg.into())
    }

    /// Create a new skill error
    pub fn skill(msg: impl Into<String>) -> Self {
        Self::Skill(msg.into())
    }

    /// Create a new plugin error
    pub fn plugin(msg: impl Into<String>) -> Self {
        Self::Plugin(msg.into())
    }

    /// Create a new cancellation error
    pub fn cancelled(msg: impl Into<String>) -> Self {
        Self::Cancelled(msg.into())
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
