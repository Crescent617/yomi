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

/// Tool output
#[derive(Debug, Clone)]
pub struct ToolOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl ToolOutput {
    pub fn new(stdout: impl Into<String>, stderr: impl Into<String>) -> Self {
        Self {
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code: 0,
        }
    }

    pub const fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Tool definition for model
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Session record metadata
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: SessionId,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Token usage tracking
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl TokenUsage {
    pub const fn add(&mut self, other: &Self) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }
}

/// 单个会话事件 - 存储在 JSONL 中
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum SessionEvent {
    Created {
        session_id: SessionId,
        created_at: DateTime<Utc>,
    },
    MessageAdded {
        message: Message,
        timestamp: DateTime<Utc>,
    },
    Forked {
        parent_id: SessionId,
        new_session_id: SessionId,
        timestamp: DateTime<Utc>,
    },
    Completed {
        completed_at: DateTime<Utc>,
    },
}

impl SessionEvent {
    pub fn created(session_id: SessionId) -> Self {
        Self::Created {
            session_id,
            created_at: Utc::now(),
        }
    }

    pub fn message_added(message: Message) -> Self {
        Self::MessageAdded {
            message,
            timestamp: Utc::now(),
        }
    }
}

/// 用于 JSONL 存储的包装类型
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEventRecord {
    pub timestamp: DateTime<Utc>,
    pub event: SessionEvent,
}
