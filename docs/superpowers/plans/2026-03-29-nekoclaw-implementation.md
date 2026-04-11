# Nekoclaw Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a production-grade AI coding assistant CLI tool with async agent runtime, trait-based architecture (core defines interfaces, adapters implement I/O), and elegant TUI.

**Architecture:** Core crate defines traits (ModelProvider, Storage, Tool) + async agent event loop. Adapters crate implements concrete I/O (OpenAI HTTP client, SQLite storage, bash tool). App layer orchestrates. Dependencies flow inward: adapters → core.

**Tech Stack:** Rust, tokio (async), serde (serialization), broadcast channels (event bus), ratatui (TUI), reqwest (HTTP in adapters only), sqlx (DB in adapters only)

---

## File Structure Overview

```
crates/
├── shared/
│   └── src/types.rs           # Common types (AgentId, Message, etc.)
│
├── core/
│   ├── src/lib.rs             # Module re-exports
│   ├── src/event.rs           # Event types (Event, UserEvent, AgentEvent, etc.)
│   ├── src/bus.rs             # EventBus with broadcast
│   ├── src/provider.rs        # ModelProvider trait
│   ├── src/storage.rs         # Storage trait
│   ├── src/tool.rs            # Tool trait, ToolRegistry, ToolSandbox
│   ├── src/agent/
│   │   ├── mod.rs             # Agent struct, spawn, run loop
│   │   ├── state.rs           # AgentState enum, state transitions
│   │   └── config.rs          # AgentConfig
│   └── src/prompt.rs          # PromptBuilder
│
├── adapters/
│   ├── src/lib.rs             # Module re-exports
│   ├── src/openai.rs          # OpenAI ModelProvider impl
│   ├── src/anthropic.rs       # Anthropic ModelProvider impl
│   ├── src/sqlite.rs          # SQLite Storage impl
│   ├── src/bash_tool.rs       # Bash Tool impl
│   └── src/file_tool.rs       # File Tool impl
│
├── app/
│   └── src/
│       ├── session.rs         # Session management
│       ├── config.rs          # App configuration
│       └── coordinator.rs     # High-level workflow
│
├── cli/
│   └── src/main.rs            # CLI entry point
│
└── tui/
    └── src/...                # Terminal UI components
```

---

## Phase 1: Workspace Setup & Shared Types

### Task 1: Create Workspace Structure

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `crates/shared/Cargo.toml`
- Create: `crates/shared/src/lib.rs`
- Create: `crates/core/Cargo.toml`
- Create: `crates/adapters/Cargo.toml`

- [ ] **Step 1: Create root workspace Cargo.toml**

```toml
[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
authors = ["Your Name <you@example.com>"]
license = "MIT OR Apache-2.0"
repository = "https://github.com/yourusername/nekoclaw"

[workspace.dependencies]
# Core deps
tokio = { version = "1.40", features = ["full"] }
tokio-util = "0.7"
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
thiserror = "2.0"
anyhow = "1.0"
tracing = "0.1"
async-trait = "0.1"
chrono = { version = "0.4", features = ["serde"] }
uuid = { version = "1.10", features = ["v4", "serde"] }

# TUI deps
ratatui = "0.29"
crossterm = "0.28"
pulldown-cmark = "0.12"

# CLI deps
clap = { version = "4.5", features = ["derive"] }
config = "0.14"
tracing-subscriber = "0.3"

# Adapter deps (only used in adapters crate)
reqwest = { version = "0.12", features = ["rustls-tls", "stream", "json"] }
eventsource-stream = "0.2"
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate"] }
futures = "0.3"

# Internal
nekoclaw-shared = { path = "crates/shared" }
nekoclaw-core = { path = "crates/core" }
nekoclaw-adapters = { path = "crates/adapters" }
nekoclaw-app = { path = "crates/app" }
```

- [ ] **Step 2: Create shared crate**

Create `crates/shared/Cargo.toml`:
```toml
[package]
name = "nekoclaw-shared"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[dependencies]
serde = { workspace = true }
uuid = { workspace = true }
chrono = { workspace = true }
```

Create `crates/shared/src/lib.rs`:
```rust
pub mod types;
```

- [ ] **Step 3: Create shared types**

Create `crates/shared/src/types.rs`:
```rust
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Unique identifier for agents
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentId(pub String);

impl AgentId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
    }
}

impl Default for AgentId {
    fn default() -> Self {
        Self::new()
    }
}

/// Unique identifier for sessions
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4().to_string())
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

/// Content block - similar to OpenAI's content format
/// Supports text, thinking, images, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    /// Plain text content
    Text { text: String },
    /// Model's thinking/reasoning process (shown in UI but not sent back to model)
    Thinking { thinking: String, signature: Option<String> },
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
            ContentBlock::Text { text } => Some(text),
            _ => None,
        }
    }

    /// Get thinking content if this is a thinking block
    pub fn as_thinking(&self) -> Option<&str> {
        match self {
            ContentBlock::Thinking { thinking, .. } => Some(thinking),
            _ => None,
        }
    }

    /// Check if this is a text block
    pub fn is_text(&self) -> bool {
        matches!(self, ContentBlock::Text { .. })
    }

    /// Check if this is a thinking block
    pub fn is_thinking(&self) -> bool {
        matches!(self, ContentBlock::Thinking { .. })
    }
}

impl From<String> for ContentBlock {
    fn from(text: String) -> Self {
        ContentBlock::Text { text }
    }
}

impl From<&str> for ContentBlock {
    fn from(text: &str) -> Self {
        ContentBlock::Text { text: text.to_string() }
    }
}

/// Image URL structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>, // auto, low, high
}

/// Audio data structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AudioData {
    pub data: String, // base64 encoded
    pub format: String, // mp3, wav, etc.
}

/// Chat message with content blocks (OpenAI-style)
#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

impl Message {
    /// Create a message with single text content
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: Role::System,
            content: vec![ContentBlock::Text { text: content.into() }],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
        }
    }

    /// Create a user message with text content
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: vec![ContentBlock::Text { text: content.into() }],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
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
                    }
                },
            ],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
        }
    }

    /// Create an assistant message with text
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![ContentBlock::Text { text: content.into() }],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
        }
    }

    /// Create an assistant message with thinking
    pub fn assistant_with_thinking(text: impl Into<String>, thinking: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Thinking { thinking: thinking.into(), signature: None },
                ContentBlock::Text { text: text.into() },
            ],
            tool_calls: None,
            tool_call_id: None,
            created_at: Utc::now(),
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
        let thinking: Vec<_> = self.content
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
        if let Some(last) = self.content.last_mut() {
            if let ContentBlock::Text { text: existing } = last {
                existing.push_str(&text);
                return;
            }
        }
        self.content.push(ContentBlock::Text { text });
    }
}

/// Custom serialization for content to support both string and array formats
mod content_serde {
    use super::ContentBlock;
    use serde::{Deserialize, Deserializer, Serializer};

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
            return Ok(vec![ContentBlock::Text { text: s.to_string() }]);
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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

    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Tool definition for model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

/// Session record metadata
#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub project_path: String,
    pub message_count: i32,
    pub parent_session_id: Option<String>,
}
```

- [ ] **Step 4: Test shared types compile**

Run: `cd /home/hrli/repos/nekoclaw && cargo check -p nekoclaw-shared`
Expected: Clean compile, no errors

- [ ] **Step 5: Commit**

```bash
cd /home/hrli/repos/nekoclaw
git add Cargo.toml crates/shared/
git commit -m "feat: setup workspace and shared types

- Create workspace structure with 5 crates
- Define core types: AgentId, SessionId, Message, ToolCall
- Add Message constructors for system/user/assistant"
```

---

## Phase 2: Core Crate - Events & Bus

### Task 2: Event Types

**Files:**
- Create: `crates/core/Cargo.toml`
- Create: `crates/core/src/lib.rs`
- Create: `crates/core/src/event.rs`

- [ ] **Step 1: Create core crate Cargo.toml**

```toml
[package]
name = "nekoclaw-core"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[dependencies]
nekoclaw-shared = { workspace = true }

tokio = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }
async-trait = { workspace = true }
chrono = { workspace = true }
uuid = { workspace = true }
futures = { workspace = true }

[dev-dependencies]
tokio-test = "0.4"
```

- [ ] **Step 2: Create event types**

Create `crates/core/src/event.rs`:
```rust
use nekoclaw_shared::types::{AgentId, Message, SessionId, ToolCall, ToolOutput};
use serde::{Deserialize, Serialize};

/// Top-level event wrapper - modular design prevents enum explosion
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Event {
    User(UserEvent),
    Agent(AgentEvent),
    Model(ModelEvent),
    Tool(ToolEvent),
    System(SystemEvent),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserEvent {
    Message { content: String },
    Confirm { tool_id: String, approved: bool },
    Interrupt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentEvent {
    Started { agent_id: AgentId },
    StateChanged { agent_id: AgentId, state: String }, // State as string for serialization
    Completed { agent_id: AgentId, result: AgentResult },
    Failed { agent_id: AgentId, error: String },
    Cancelled { agent_id: AgentId },
    SubAgentSpawned { parent_id: AgentId, child_id: AgentId, mode: String },
    Progress { agent_id: AgentId, update: ProgressUpdate },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModelEvent {
    Request { agent_id: AgentId, message_count: usize },
    /// Content chunk (text or thinking)
    Chunk { agent_id: AgentId, content: ContentChunk },
    Complete { agent_id: AgentId },
    Error { agent_id: AgentId, error: String },
    Fallback { agent_id: AgentId, from: String, to: String },
}

/// Content chunk for streaming
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentChunk {
    Text(String),
    Thinking { thinking: String, signature: Option<String> },
    RedactedThinking,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolEvent {
    Started { agent_id: AgentId, tool_id: String, tool_name: String },
    Output { agent_id: AgentId, tool_id: String, output: String },
    Error { agent_id: AgentId, tool_id: String, error: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SystemEvent {
    Shutdown,
    ConfigReloaded,
    SessionForked { from: SessionId, to: SessionId },
}

/// Agent execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResult {
    pub messages: Vec<Message>,
    pub tool_calls: usize,
}

/// Progress update for long-running operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressUpdate {
    pub step: usize,
    pub total: Option<usize>,
    pub message: String,
}
```

- [ ] **Step 3: Test events compile**

Run: `cargo check -p nekoclaw-core`
Expected: Clean compile

- [ ] **Step 4: Commit**

```bash
git add crates/core/Cargo.toml crates/core/src/event.rs crates/core/src/lib.rs
git commit -m "feat(core): add modular event types

- Event enum with User/Agent/Model/Tool/System variants
- Each variant is a separate enum for extensibility
- Serializable for potential network transport"
```

### Task 3: Broadcast Event Bus

**Files:**
- Create: `crates/core/src/bus.rs`
- Modify: `crates/core/src/lib.rs`

- [ ] **Step 1: Implement broadcast event bus**

Create `crates/core/src/bus.rs`:
```rust
use crate::event::Event;
use anyhow::Result;
use tokio::sync::broadcast;

/// Async event bus with broadcast semantics
/// Multiple subscribers can receive the same event (TUI, Logger, Recorder, etc.)
#[derive(Debug, Clone)]
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    /// Create new event bus with specified capacity
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publish event to all subscribers (non-blocking)
    /// Returns error only if all receivers have been dropped
    pub fn send(&self, event: Event) -> Result<()> {
        match self.tx.send(event) {
            Ok(_) => Ok(()),
            Err(broadcast::error::SendError(_)) => {
                // No active receivers, that's ok
                tracing::debug!("Event sent but no active receivers");
                Ok(())
            }
        }
    }

    /// Subscribe to events
    /// Returns a receiver that will receive all future events
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Get number of active subscribers
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{AgentEvent, UserEvent};
    use nekoclaw_shared::types::AgentId;

    #[tokio::test]
    async fn test_broadcast_multiple_subscribers() {
        let bus = EventBus::new(10);

        // Two subscribers
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();

        // Send one event
        let event = Event::User(UserEvent::Message {
            content: "hello".to_string(),
        });
        bus.send(event.clone()).unwrap();

        // Both receive it
        let recv1 = rx1.recv().await.unwrap();
        let recv2 = rx2.recv().await.unwrap();

        assert!(matches!(recv1, Event::User(UserEvent::Message { .. })));
        assert!(matches!(recv2, Event::User(UserEvent::Message { .. })));
    }

    #[tokio::test]
    async fn test_subscriber_count() {
        let bus = EventBus::new(10);
        assert_eq!(bus.subscriber_count(), 0);

        let _rx1 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 1);

        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);

        drop(_rx1);
        // Count updates after drop (may need tokio::task::yield_now)
        tokio::task::yield_now().await;
        assert_eq!(bus.subscriber_count(), 1);
    }
}
```

- [ ] **Step 2: Update lib.rs to export modules**

Create `crates/core/src/lib.rs`:
```rust
pub mod agent;
pub mod bus;
pub mod event;
pub mod prompt;
pub mod provider;
pub mod storage;
pub mod tool;

// Re-export commonly used types
pub use event::{AgentEvent, Event, ModelEvent, SystemEvent, ToolEvent, UserEvent, ContentChunk, ProgressUpdate, AgentResult};
pub use bus::EventBus;
```

- [ ] **Step 3: Run tests**

Run: `cargo test -p nekoclaw-core`
Expected: 2 tests pass

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/bus.rs crates/core/src/lib.rs
git commit -m "feat(core): add broadcast event bus

- Uses tokio::sync::broadcast for true fan-out
- Multiple subscribers can receive same event
- Includes tests for multi-consumer scenario"
```

---

## Phase 3: Core Crate - Provider & Storage Traits

### Task 4: ModelProvider Trait

**Files:**
- Create: `crates/core/src/provider.rs`
- Modify: `crates/core/src/lib.rs`

- [ ] **Step 1: Implement ModelProvider trait**

Create `crates/core/src/provider.rs`:
```rust
use crate::event::Event;
use anyhow::Result;
use async_trait::async_trait;
use futures::stream::BoxStream;
use nekoclaw_shared::types::{Message, ToolDefinition};
use serde::{Deserialize, Serialize};
use std::pin::Pin;

/// Stream of model events (chunks, errors, completion)
pub type ModelStream = Pin<Box<dyn futures::Stream<Item = Result<ModelStreamItem>> + Send>>;

/// Items emitted by model stream
#[derive(Debug, Clone)]
pub enum ModelStreamItem {
    /// Content chunk (text or thinking)
    Chunk(ContentChunk),
    /// Tool call requested by model
    ToolCall(ToolCallRequest),
    /// Stream completed successfully
    Complete,
    /// Model switched to fallback
    Fallback { from: String, to: String },
}

/// Tool call request from model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Model configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    pub model_id: String,
    pub endpoint: String,
    pub api_key: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    /// Fallback model if primary fails
    pub fallback_model_id: Option<String>,
    /// SSE idle timeout in seconds
    pub sse_timeout_secs: u64,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            model_id: String::new(),
            endpoint: String::new(),
            api_key: String::new(),
            max_tokens: None,
            temperature: None,
            fallback_model_id: None,
            sse_timeout_secs: 30,
        }
    }
}

/// Core trait for model providers
/// Implementations live in adapters crate (OpenAI, Anthropic, etc.)
#[async_trait]
pub trait ModelProvider: Send + Sync {
    /// Stream completions from the model
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> Result<ModelStream>;

    /// Check if provider supports streaming
    fn supports_streaming(&self) -> bool {
        true
    }

    /// Provider name for logging/debugging
    fn name(&self) -> &str;
}

/// Wrapper that adds rate limit retry with exponential backoff
pub struct RetryingProvider<P: ModelProvider> {
    inner: P,
    max_retries: u32,
    base_delay_ms: u64,
}

impl<P: ModelProvider> RetryingProvider<P> {
    pub fn new(inner: P) -> Self {
        Self {
            inner,
            max_retries: 3,
            base_delay_ms: 1000,
        }
    }

    pub fn with_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }
}

#[async_trait]
impl<P: ModelProvider> ModelProvider for RetryingProvider<P> {
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> Result<ModelStream> {
        let mut attempt = 0;
        loop {
            match self.inner.stream(messages, tools, config).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    attempt += 1;
                    if attempt > self.max_retries {
                        return Err(e);
                    }
                    // Check if error is rate limit (429)
                    let err_str = e.to_string();
                    if err_str.contains("429") || err_str.contains("rate limit") {
                        let delay = self.base_delay_ms * 2_u64.pow(attempt - 1);
                        tracing::warn!(
                            "Rate limited, retrying in {}ms (attempt {}/{})",
                            delay,
                            attempt,
                            self.max_retries
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay)).await;
                        continue;
                    }
                    return Err(e);
                }
            }
        }
    }

    fn name(&self) -> &str {
        self.inner.name()
    }
}
```

- [ ] **Step 2: Add provider module to lib.rs**

Update `crates/core/src/lib.rs`:
```rust
pub mod agent;
pub mod bus;
pub mod event;
pub mod prompt;
pub mod provider;
pub mod storage;
pub mod tool;

// Re-export commonly used types
pub use bus::EventBus;
pub use event::{AgentEvent, Event, ModelEvent, SystemEvent, ToolEvent, UserEvent};
pub use provider::{ModelConfig, ModelProvider, ModelStream, ModelStreamItem, RetryingProvider, ToolCallRequest, ContentChunk};
```

- [ ] **Step 3: Test provider compiles**

Run: `cargo check -p nekoclaw-core`
Expected: Clean compile

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/provider.rs crates/core/src/lib.rs
git commit -m "feat(core): add ModelProvider trait with retry wrapper

- ModelProvider trait for adapter implementations
- ModelStream type for streaming responses
- RetryingProvider with exponential backoff for rate limits
- Configurable SSE timeout and fallback model support"
```

### Task 5: Storage Trait

**Files:**
- Create: `crates/core/src/storage.rs`

- [ ] **Step 1: Implement Storage trait**

Create `crates/core/src/storage.rs`:
```rust
use anyhow::Result;
use async_trait::async_trait;
use nekoclaw_shared::types::{Message, SessionRecord, SessionId};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Core trait for persistence
/// Implementations live in adapters crate (SQLite, filesystem, etc.)
#[async_trait]
pub trait Storage: Send + Sync {
    /// Create a new session
    async fn create_session(&self, project_path: &Path) -> Result<SessionId>;

    /// Fork an existing session
    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId>;

    /// Get session metadata
    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>>;

    /// List all sessions for a project
    async fn list_sessions(&self, project_path: &Path) -> Result<Vec<SessionRecord>>;

    /// Delete a session
    async fn delete_session(&self, id: &SessionId) -> Result<()>;

    /// Append messages to session
    async fn append_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()>;

    /// Get all messages for a session
    async fn get_messages(&self, session_id: &SessionId) -> Result<Vec<Message>>;

    /// Update session summary (for context compaction)
    async fn update_summary(&self, session_id: &SessionId, summary: &str) -> Result<()>;

    /// Get session summary if exists
    async fn get_summary(&self, session_id: &SessionId) -> Result<Option<String>>;
}

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Database URL or directory path
    pub url: String,
    /// Max messages before triggering compaction
    pub compaction_threshold: usize,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            url: "~/.nekoclaw/sessions.db".to_string(),
            compaction_threshold: 100,
        }
    }
}
```

- [ ] **Step 2: Export storage module**

Update `crates/core/src/lib.rs`:
```rust
pub use storage::{Storage, StorageConfig};
```

- [ ] **Step 3: Test storage compiles**

Run: `cargo check -p nekoclaw-core`
Expected: Clean compile

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/storage.rs crates/core/src/lib.rs
git commit -m "feat(core): add Storage trait for persistence

- Storage trait with session/message operations
- Support for session forking
- Context compaction via summary storage
- Configurable compaction threshold"
```

---

## Phase 4: Core Crate - Tool System

### Task 6: Tool Trait & Registry

**Files:**
- Create: `crates/core/src/tool.rs`

- [ ] **Step 1: Implement Tool trait and registry**

Create `crates/core/src/tool.rs`:
```rust
use anyhow::Result;
use async_trait::async_trait;
use nekoclaw_shared::types::{ToolDefinition, ToolOutput};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

/// Core trait for tools
/// Implementations live in adapters crate (Bash, File, etc.)
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name (must be unique)
    fn name(&self) -> &str;

    /// Tool description for model
    fn description(&self) -> &str;

    /// JSON schema for tool parameters
    fn parameters_schema(&self) -> Value;

    /// Execute the tool
    async fn execute(&self, args: Value) -> Result<ToolOutput>;

    /// Whether this tool requires user confirmation
    fn requires_confirmation(&self) -> bool {
        true
    }

    /// Check if execution should be allowed (sandbox check)
    async fn is_allowed(&self, _args: &Value) -> Result<bool> {
        Ok(true)
    }
}

/// Tool registry - manages available tools
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    /// Register a tool
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// Get a tool by name
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    /// Get all tool definitions for model
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .values()
            .map(|tool| ToolDefinition {
                name: tool.name().to_string(),
                description: tool.description().to_string(),
                parameters: tool.parameters_schema(),
            })
            .collect()
    }

    /// List all tool names
    pub fn list(&self) -> Vec<String> {
        self.tools.keys().cloned().collect()
    }

    /// Check if tool exists
    pub fn has(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }
}

/// Tool sandbox for permission management
#[derive(Clone)]
pub struct ToolSandbox {
    /// Globally disabled (opt-in via --sandbox)
    enabled: bool,
    /// Tools that require confirmation (can be overridden)
    require_confirmation: HashMap<String, bool>,
    /// YOLO mode - skip all confirmations
    yolo_mode: bool,
}

impl Default for ToolSandbox {
    fn default() -> Self {
        Self {
            enabled: false, // Disabled by default
            require_confirmation: HashMap::new(),
            yolo_mode: false,
        }
    }
}

impl ToolSandbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enable sandbox mode
    pub fn enable(mut self) -> Self {
        self.enabled = true;
        self
    }

    /// Enable YOLO mode (skip all confirmations)
    pub fn yolo(mut self) -> Self {
        self.yolo_mode = true;
        self
    }

    /// Set confirmation requirement for a specific tool
    pub fn set_confirmation(&mut self, tool_name: &str, required: bool) {
        self.require_confirmation.insert(tool_name.to_string(), required);
    }

    /// Check if tool execution should prompt for confirmation
    pub fn needs_confirmation(&self, tool_name: &str, tool_requires: bool) -> bool {
        if !self.enabled || self.yolo_mode {
            return false;
        }

        // Check per-tool override
        if let Some(&required) = self.require_confirmation.get(tool_name) {
            return required;
        }

        tool_requires
    }
}

/// Global YOLO mode flag (set via --yolo CLI flag)
static YOLO_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Enable global YOLO mode
pub fn enable_yolo_mode() {
    YOLO_MODE.store(true, std::sync::atomic::Ordering::SeqCst);
}

/// Check if YOLO mode is enabled
pub fn is_yolo_mode() -> bool {
    YOLO_MODE.load(std::sync::atomic::Ordering::SeqCst)
}
```

- [ ] **Step 2: Export tool module**

Update `crates/core/src/lib.rs`:
```rust
pub use tool::{Tool, ToolRegistry, ToolSandbox, enable_yolo_mode, is_yolo_mode};
```

- [ ] **Step 3: Test tool compiles**

Run: `cargo check -p nekoclaw-core`
Expected: Clean compile

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/tool.rs crates/core/src/lib.rs
git commit -m "feat(core): add Tool trait, Registry, and Sandbox

- Tool trait with execute, schema, and confirmation methods
- ToolRegistry for managing available tools
- ToolSandbox for permission management
- Global YOLO mode flag for skipping confirmations
- Sandbox disabled by default"
```

---

## Phase 5: Core Crate - Agent Implementation

### Task 7: Agent State Machine

**Files:**
- Create: `crates/core/src/agent/state.rs`
- Create: `crates/core/src/agent/config.rs`
- Create: `crates/core/src/agent/mod.rs`

- [ ] **Step 1: Implement AgentState**

Create `crates/core/src/agent/state.rs`:
```rust
/// Agent state machine - single threaded, no locks needed
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Agent created but not started
    Idle,
    /// Waiting for user input
    WaitingForInput,
    /// Streaming response from model
    Streaming,
    /// Executing tool
    ExecutingTool,
    /// Terminal: Completed successfully
    Completed,
    /// Terminal: Failed with error
    Failed,
    /// Terminal: Cancelled by user
    Cancelled,
}

impl AgentState {
    /// Check if state is terminal (agent loop should stop)
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }

    /// Get string representation for events
    pub fn as_str(&self) -> &'static str {
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
```

- [ ] **Step 2: Implement AgentConfig**

Create `crates/core/src/agent/config.rs`:
```rust
use crate::provider::ModelConfig;
use crate::storage::StorageConfig;
use serde::{Deserialize, Serialize};

/// Agent behavior configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Model configuration
    pub model: ModelConfig,
    /// Storage configuration
    pub storage: StorageConfig,
    /// Max iterations before forcing completion
    pub max_iterations: usize,
    /// Enable sub-agent spawning
    pub enable_sub_agents: bool,
    /// Sub-agent mode
    pub sub_agent_mode: SubAgentMode,
    /// System prompt
    pub system_prompt: String,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            model: ModelConfig::default(),
            storage: StorageConfig::default(),
            max_iterations: 50,
            enable_sub_agents: true,
            sub_agent_mode: SubAgentMode::Async,
            system_prompt: "You are a helpful AI coding assistant.".to_string(),
        }
    }
}

/// Sub-agent execution mode
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SubAgentMode {
    /// Sub-agents run in parallel (fire and forget)
    Async,
    /// Sub-agent must complete before parent continues
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
```

- [ ] **Step 3: Implement Agent struct and main loop**

Create `crates/core/src/agent/mod.rs`:
```rust
mod config;
mod state;

pub use config::{AgentConfig, SubAgentMode};
pub use state::AgentState;

use crate::bus::EventBus;
use crate::event::{AgentEvent, AgentResult, ModelEvent, ProgressUpdate, ToolEvent, UserEvent};
use crate::provider::{ModelProvider, ModelStreamItem};
use crate::storage::Storage;
use crate::tool::{ToolRegistry, ToolSandbox};
use anyhow::Result;
use nekoclaw_shared::types::{AgentId, Message, Role, ToolCall};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Agent - single-threaded async event loop
pub struct Agent {
    id: AgentId,
    state: AgentState,
    config: AgentConfig,
    messages: Vec<Message>,
    event_bus: EventBus,
    provider: Arc<dyn ModelProvider>,
    storage: Arc<dyn Storage>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    /// Channel for user input
    input_rx: mpsc::Receiver<String>,
    /// Sub-agents spawned by this agent
    sub_agents: Vec<AgentHandle>,
    iteration_count: usize,
}

/// Handle to a running sub-agent
pub struct AgentHandle {
    pub id: AgentId,
    pub mode: SubAgentMode,
    // TODO: Add join handle for sync mode
}

impl Agent {
    /// Create a new agent
    pub fn new(
        id: AgentId,
        config: AgentConfig,
        event_bus: EventBus,
        provider: Arc<dyn ModelProvider>,
        storage: Arc<dyn Storage>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
        input_rx: mpsc::Receiver<String>,
    ) -> Self {
        let mut messages = Vec::new();
        messages.push(Message::system(&config.system_prompt));

        Self {
            id,
            state: AgentState::Idle,
            config,
            messages,
            event_bus,
            provider,
            storage,
            tool_registry,
            sandbox,
            input_rx,
            sub_agents: Vec::new(),
            iteration_count: 0,
        }
    }

    /// Get agent ID
    pub fn id(&self) -> &AgentId {
        &self.id
    }

    /// Get current state
    pub fn state(&self) -> AgentState {
        self.state
    }

    /// Spawn the agent as an async task
    pub fn spawn(mut self) -> AgentId {
        let id = self.id.clone();
        tokio::spawn(async move {
            if let Err(e) = self.run().await {
                tracing::error!("Agent {} failed: {}", id.0, e);
                self.transition_to(AgentState::Failed);
            }
        });
        id
    }

    /// Main agent loop
    async fn run(&mut self) -> Result<()> {
        self.transition_to(AgentState::WaitingForInput);

        loop {
            // Check for terminal state
            if self.state.is_terminal() {
                break;
            }

            // Check iteration limit
            if self.iteration_count >= self.config.max_iterations {
                tracing::warn!("Max iterations reached, forcing completion");
                self.transition_to(AgentState::Completed);
                break;
            }

            match self.state {
                AgentState::WaitingForInput => {
                    self.handle_wait_for_input().await?;
                }
                AgentState::Streaming => {
                    self.handle_streaming().await?;
                }
                AgentState::ExecutingTool => {
                    self.handle_execute_tool().await?;
                }
                _ => {
                    // Idle or terminal states
                    tokio::task::yield_now().await;
                }
            }

            self.iteration_count += 1;
        }

        // Emit completion event
        let result = AgentResult {
            messages: self.messages.clone(),
            tool_calls: self.count_tool_calls(),
        };
        self.event_bus.send(crate::event::Event::Agent(
            AgentEvent::Completed {
                agent_id: self.id.clone(),
                result,
            },
        ))?;

        Ok(())
    }

    /// State: Wait for user input
    async fn handle_wait_for_input(&mut self) -> Result<()> {
        match self.input_rx.recv().await {
            Some(content) => {
                self.messages.push(Message::user(content));
                self.transition_to(AgentState::Streaming);
                Ok(())
            }
            None => {
                // Channel closed
                self.transition_to(AgentState::Cancelled);
                Ok(())
            }
        }
    }

    /// State: Stream from model
    async fn handle_streaming(&mut self) -> Result<()> {
        let tools = self.tool_registry.definitions();

        // Emit request event
        self.event_bus.send(crate::event::Event::Model(ModelEvent::Request {
            agent_id: self.id.clone(),
            message_count: self.messages.len(),
        }))?;

        // Start streaming
        let mut stream = self
            .provider
            .stream(&self.messages, &tools, &self.config.model)
            .await?;

        let mut content_blocks: Vec<ContentBlock> = Vec::new();
        let mut current_text = String::new();
        let mut current_thinking = String::new();
        let mut pending_tool_calls: Vec<ToolCall> = Vec::new();

        // Process stream with batching to prevent starvation
        while let Some(item) = stream.try_next().await? {
            match item {
                ModelStreamItem::Chunk(ContentChunk::Text(text)) => {
                    current_text.push_str(&text);
                    self.event_bus.send(crate::event::Event::Model(ModelEvent::Chunk {
                        agent_id: self.id.clone(),
                        content: ContentChunk::Text(text),
                    }))?;
                }
                ModelStreamItem::Chunk(ContentChunk::Thinking { thinking, signature }) => {
                    current_thinking.push_str(&thinking);
                    self.event_bus.send(crate::event::Event::Model(ModelEvent::Chunk {
                        agent_id: self.id.clone(),
                        content: ContentChunk::Thinking { thinking, signature },
                    }))?;
                }
                ModelStreamItem::Chunk(ContentChunk::RedactedThinking) => {
                    content_blocks.push(ContentBlock::RedactedThinking { data: String::new() });
                }
                ModelStreamItem::ToolCall(request) => {
                    pending_tool_calls.push(ToolCall {
                        id: request.id,
                        name: request.name,
                        arguments: request.arguments,
                    });
                }
                ModelStreamItem::Complete => break,
                ModelStreamItem::Fallback { from, to } => {
                    self.event_bus.send(crate::event::Event::Model(ModelEvent::Fallback {
                        agent_id: self.id.clone(),
                        from,
                        to,
                    }))?;
                }
            }

            // Yield periodically to prevent starvation
            if current_text.len() + current_thinking.len() % 1000 == 0 {
                tokio::task::yield_now().await;
            }
        }

        // Build content blocks for the message
        if !current_thinking.is_empty() {
            content_blocks.push(ContentBlock::Thinking {
                thinking: current_thinking,
                signature: None
            });
        }
        if !current_text.is_empty() {
            content_blocks.push(ContentBlock::Text { text: current_text });
        }

        // Add assistant message
        if !content_blocks.is_empty() || !pending_tool_calls.is_empty() {
            let mut msg = Message::with_blocks(Role::Assistant, content_blocks);
            if !pending_tool_calls.is_empty() {
                msg.tool_calls = Some(pending_tool_calls);
            }
            self.messages.push(msg);
        }

        // Transition to tool execution or back to waiting
        if self.messages.last().and_then(|m| m.tool_calls.as_ref()).is_some() {
            self.transition_to(AgentState::ExecutingTool);
        } else {
            self.transition_to(AgentState::WaitingForInput);
        }

        Ok(())
    }

    /// State: Execute tool calls
    async fn handle_execute_tool(&mut self) -> Result<()> {
        let tool_calls = self
            .messages
            .last()
            .and_then(|m| m.tool_calls.clone())
            .unwrap_or_default();

        for call in tool_calls {
            // Emit started event
            self.event_bus.send(crate::event::Event::Tool(ToolEvent::Started {
                agent_id: self.id.clone(),
                tool_id: call.id.clone(),
                tool_name: call.name.clone(),
            }))?;

            // Check sandbox/confirmation
            let tool = match self.tool_registry.get(&call.name) {
                Some(t) => t,
                None => {
                    self.event_bus.send(crate::event::Event::Tool(ToolEvent::Error {
                        agent_id: self.id.clone(),
                        tool_id: call.id.clone(),
                        error: format!("Unknown tool: {}", call.name),
                    }))?;
                    continue;
                }
            };

            // Check if allowed
            if let Err(e) = tool.is_allowed(&call.arguments).await {
                self.event_bus.send(crate::event::Event::Tool(ToolEvent::Error {
                    agent_id: self.id.clone(),
                    tool_id: call.id.clone(),
                    error: e.to_string(),
                }))?;
                continue;
            }

            // Execute tool
            match tool.execute(call.arguments).await {
                Ok(output) => {
                    let content = if output.success() {
                        output.stdout.clone()
                    } else {
                        format!("Exit code: {}\n{}\n{}", output.exit_code, output.stdout, output.stderr)
                    };

                    self.event_bus.send(crate::event::Event::Tool(ToolEvent::Output {
                        agent_id: self.id.clone(),
                        tool_id: call.id.clone(),
                        output: content.clone(),
                    }))?;

                    // Add tool result as message
                    self.messages.push(Message {
                        role: Role::Tool,
                        content,
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                        created_at: chrono::Utc::now(),
                    });
                }
                Err(e) => {
                    self.event_bus.send(crate::event::Event::Tool(ToolEvent::Error {
                        agent_id: self.id.clone(),
                        tool_id: call.id.clone(),
                        error: e.to_string(),
                    }))?;
                }
            }

            // Yield between tools
            tokio::task::yield_now().await;
        }

        // After executing tools, stream again for model's response
        self.transition_to(AgentState::Streaming);
        Ok(())
    }

    /// Transition to new state
    fn transition_to(&mut self, new_state: AgentState) {
        let old_state = self.state;
        self.state = new_state;

        if old_state != new_state {
            self.event_bus
                .send(crate::event::Event::Agent(AgentEvent::StateChanged {
                    agent_id: self.id.clone(),
                    state: new_state.to_string(),
                }))
                .ok();
        }
    }

    /// Count total tool calls in conversation
    fn count_tool_calls(&self) -> usize {
        self.messages
            .iter()
            .filter_map(|m| m.tool_calls.as_ref().map(|c| c.len()))
            .sum()
    }

    /// Spawn a sub-agent
    pub fn spawn_sub_agent(&mut self, mode: SubAgentMode) -> AgentId {
        let child_id = AgentId::new();

        // TODO: Create sub-agent with copy-on-write context
        // For now, just record the spawn

        self.sub_agents.push(AgentHandle {
            id: child_id.clone(),
            mode,
        });

        self.event_bus
            .send(crate::event::Event::Agent(AgentEvent::SubAgentSpawned {
                parent_id: self.id.clone(),
                child_id: child_id.clone(),
                mode: mode.to_string(),
            }))
            .ok();

        child_id
    }
}
```

- [ ] **Step 4: Fix imports in agent module**

Add to `crates/core/src/agent/mod.rs` at the top:
```rust
use futures::TryStreamExt;
```

- [ ] **Step 5: Test agent compiles**

Run: `cargo check -p nekoclaw-core`
Expected: Clean compile (may need to add futures dependency features)

If needed, update `crates/core/Cargo.toml`:
```toml
futures = { workspace = true, features = ["std"] }
```

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/agent/
git commit -m "feat(core): add Agent implementation with state machine

- AgentState enum with terminal states (Completed, Failed, Cancelled)
- AgentConfig with sub-agent mode (async/sync)
- Single-threaded async event loop with state transitions
- Streaming with periodic yield to prevent starvation
- Tool execution with sandbox checks
- Sub-agent spawning support"
```

---

## Phase 6: Adapters Crate - OpenAI & SQLite

### Task 8: Create Adapters Crate Structure

**Files:**
- Create: `crates/adapters/Cargo.toml`
- Create: `crates/adapters/src/lib.rs`

- [ ] **Step 1: Create adapters Cargo.toml**

Create `crates/adapters/Cargo.toml`:
```toml
[package]
name = "nekoclaw-adapters"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[dependencies]
nekoclaw-shared = { workspace = true }
nekoclaw-core = { workspace = true }

# Async
tokio = { workspace = true }
async-trait = { workspace = true }
futures = { workspace = true }

# Serialization
serde = { workspace = true }
serde_json = { workspace = true }

# Error handling
thiserror = { workspace = true }
anyhow = { workspace = true }
tracing = { workspace = true }

# HTTP (OpenAI client)
reqwest = { workspace = true }
eventsource-stream = { workspace = true }

# Database (SQLite)
sqlx = { workspace = true }

# Utils
chrono = { workspace = true }
uuid = { workspace = true }
```

- [ ] **Step 2: Create adapters lib.rs**

Create `crates/adapters/src/lib.rs`:
```rust
pub mod anthropic;
pub mod bash_tool;
pub mod file_tool;
pub mod openai;
pub mod sqlite;

// Re-export main implementations
pub use openai::OpenAIProvider;
pub use sqlite::SqliteStorage;
pub use bash_tool::BashTool;
pub use file_tool::FileTool;
```

- [ ] **Step 3: Commit**

```bash
git add crates/adapters/
git commit -m "feat(adapters): create adapters crate structure

- OpenAI provider (HTTP client)
- Anthropic provider placeholder
- SQLite storage implementation
- Bash and File tools
- All I/O implementations isolated in adapters"
```

### Task 9: OpenAI Provider Implementation

**Files:**
- Create: `crates/adapters/src/openai.rs`

- [ ] **Step 1: Implement OpenAI provider**

Create `crates/adapters/src/openai.rs`:
```rust
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use eventsource_stream::Eventsource;
use futures::stream::{BoxStream, StreamExt};
use nekoclaw_core::provider::{ModelConfig, ModelProvider, ModelStream, ModelStreamItem, ToolCallRequest};
use nekoclaw_shared::types::{Message, Role, ToolDefinition};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub struct OpenAIProvider {
    client: Client,
    name: String,
}

impl OpenAIProvider {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()?,
            name: "openai".to_string(),
        })
    }

    /// Convert internal Message to OpenAI format
    fn convert_messages(&self, messages: &[Message]) -> Vec<OpenAIMessage> {
        messages
            .iter()
            .map(|m| OpenAIMessage {
                role: match m.role {
                    Role::System => "system".to_string(),
                    Role::User => "user".to_string(),
                    Role::Assistant => "assistant".to_string(),
                    Role::Tool => "tool".to_string(),
                },
                content: m.content.clone(),
                tool_calls: m.tool_calls.as_ref().map(|calls| {
                    calls
                        .iter()
                        .map(|c| OpenAIToolCall {
                            id: c.id.clone(),
                            type_: "function".to_string(),
                            function: OpenAIFunction {
                                name: c.name.clone(),
                                arguments: c.arguments.to_string(),
                            },
                        })
                        .collect()
                }),
                tool_call_id: m.tool_call_id.clone(),
            })
            .collect()
    }

    /// Convert ToolDefinition to OpenAI format
    fn convert_tools(&self, tools: &[ToolDefinition]) -> Vec<OpenAITool> {
        tools
            .iter()
            .map(|t| OpenAITool {
                type_: "function".to_string(),
                function: OpenAIFunctionDef {
                    name: t.name.clone(),
                    description: t.description.clone(),
                    parameters: t.parameters.clone(),
                },
            })
            .collect()
    }
}

#[async_trait]
impl ModelProvider for OpenAIProvider {
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
        config: &ModelConfig,
    ) -> Result<ModelStream> {
        let url = if config.endpoint.is_empty() {
            "https://api.openai.com/v1/chat/completions".to_string()
        } else {
            format!("{}/chat/completions", config.endpoint.trim_end_matches('/'))
        };

        let request_body = OpenAIRequest {
            model: config.model_id.clone(),
            messages: self.convert_messages(messages),
            tools: if tools.is_empty() { None } else { Some(self.convert_tools(tools)) },
            stream: true,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
        };

        let request = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", config.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body);

        let response = request.send().await?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(anyhow!("OpenAI API error: {} - {}", status, text));
        }

        let sse_timeout = config.sse_timeout_secs;

        let stream = response
            .bytes_stream()
            .eventsource()
            .filter_map(move |event| {
                let result = match event {
                    Ok(event) => {
                        if event.data == "[DONE]" {
                            Some(Ok(ModelStreamItem::Complete))
                        } else {
                            match parse_sse_chunk(&event.data) {
                                Ok(Some(item)) => Some(Ok(item)),
                                Ok(None) => None,
                                Err(e) => Some(Err(e)),
                            }
                        }
                    }
                    Err(e) => Some(Err(anyhow!("SSE error: {}", e))),
                };
                async move { result }
            })
            .boxed();

        Ok(stream)
    }

    fn name(&self) -> &str {
        &self.name
    }
}

/// Parse SSE chunk from OpenAI/Anthropic
fn parse_sse_chunk(data: &str) -> Result<Option<ModelStreamItem>> {
    let response: OpenAIStreamResponse = serde_json::from_str(data)
        .map_err(|e| anyhow!("Failed to parse SSE chunk: {} - data: {}", e, data))?;

    if let Some(choice) = response.choices.first() {
        if let Some(delta) = &choice.delta {
            // Check for thinking content (Claude 3.7 style)
            if let Some(thinking) = &delta.thinking {
                return Ok(Some(ModelStreamItem::Chunk(ContentChunk::Thinking {
                    thinking: thinking.clone(),
                    signature: delta.thinking_signature.clone(),
                })));
            }

            // Check for redacted thinking
            if delta.thinking_redacted.unwrap_or(false) {
                return Ok(Some(ModelStreamItem::Chunk(ContentChunk::RedactedThinking)));
            }

            // Check for content
            if let Some(content) = &delta.content {
                if !content.is_empty() {
                    return Ok(Some(ModelStreamItem::Chunk(ContentChunk::Text(content.clone()))));
                }
            }

            // Check for tool calls
            if let Some(tool_calls) = &delta.tool_calls {
                if let Some(call) = tool_calls.first() {
                    if let (Some(id), Some(name), Some(args)) = (
                        call.id.clone(),
                        call.function.as_ref().map(|f| f.name.clone()).flatten(),
                        call.function.as_ref().map(|f| f.arguments.clone()).flatten(),
                    ) {
                        let args_json: Value = serde_json::from_str(&args)
                            .unwrap_or_else(|_| Value::String(args));
                        return Ok(Some(ModelStreamItem::ToolCall(ToolCallRequest {
                            id,
                            name,
                            arguments: args_json,
                        })));
                    }
                }
            }
        }
    }

    Ok(None)
}

#[derive(Debug, Deserialize)]
struct OpenAIDelta {
    content: Option<String>,
    thinking: Option<String>,
    #[serde(rename = "thinking_signature")]
    thinking_signature: Option<String>,
    #[serde(rename = "thinking_redacted")]
    thinking_redacted: Option<bool>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}

// OpenAI API types
#[derive(Debug, Serialize)]
struct OpenAIRequest {
    model: String,
    messages: Vec<OpenAIMessage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<OpenAITool>>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIMessage {
    role: String,
    content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_calls: Option<Vec<OpenAIToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_call_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAITool {
    type_: String,
    function: OpenAIFunctionDef,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunctionDef {
    name: String,
    description: String,
    parameters: Value,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIToolCall {
    id: String,
    #[serde(rename = "type")]
    type_: String,
    function: OpenAIFunction,
}

#[derive(Debug, Serialize, Deserialize)]
struct OpenAIFunction {
    name: String,
    arguments: String,
}

#[derive(Debug, Deserialize)]
struct OpenAIStreamResponse {
    choices: Vec<OpenAIChoice>,
}

#[derive(Debug, Deserialize)]
struct OpenAIChoice {
    delta: Option<OpenAIDelta>,
}

#[derive(Debug, Deserialize)]
struct OpenAIDelta {
    content: Option<String>,
    tool_calls: Option<Vec<OpenAIToolCall>>,
}
```

- [ ] **Step 2: Test OpenAI compiles**

Run: `cargo check -p nekoclaw-adapters`
Expected: Clean compile

- [ ] **Step 3: Commit**

```bash
git add crates/adapters/src/openai.rs
git commit -m "feat(adapters): add OpenAI provider implementation

- HTTP client with SSE streaming
- Tool call parsing from stream
- Configurable endpoint, model, API key
- SSE timeout support
- Error handling for rate limits"
```

### Task 10: SQLite Storage Implementation

**Files:**
- Create: `crates/adapters/src/sqlite.rs`

- [ ] **Step 1: Implement SQLite storage**

Create `crates/adapters/src/sqlite.rs`:
```rust
use anyhow::Result;
use async_trait::async_trait;
use nekoclaw_core::storage::{Storage, StorageConfig};
use nekoclaw_shared::types::{Message, SessionRecord, SessionId};
use sqlx::{migrate::MigrateDatabase, sqlite::SqlitePoolOptions, Pool, Sqlite};
use std::path::Path;

pub struct SqliteStorage {
    pool: Pool<Sqlite>,
    config: StorageConfig,
}

impl SqliteStorage {
    pub async fn new(config: &StorageConfig) -> Result<Self> {
        let db_url = expand_path(&config.url);

        // Create database if it doesn't exist
        if !Sqlite::database_exists(&db_url).await.unwrap_or(false) {
            Sqlite::create_database(&db_url).await?;
        }

        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect(&db_url)
            .await?;

        let storage = Self {
            pool,
            config: config.clone(),
        };

        storage.migrate().await?;
        Ok(storage)
    }

    /// Run database migrations
    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                project_path TEXT NOT NULL,
                message_count INTEGER DEFAULT 0,
                parent_session_id TEXT,
                summary TEXT
            );

            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                tool_calls TEXT,
                tool_call_id TEXT,
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );

            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[async_trait]
impl Storage for SqliteStorage {
    async fn create_session(&self, project_path: &Path) -> Result<SessionId> {
        let id = SessionId::new();
        let path_str = project_path.to_string_lossy();

        sqlx::query(
            r#"
            INSERT INTO sessions (id, project_path, message_count)
            VALUES (?, ?, 0)
            "#,
        )
        .bind(&id.0)
        .bind(path_str.as_ref())
        .execute(&self.pool)
        .await?;

        Ok(id)
    }

    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId> {
        let new_id = SessionId::new();

        sqlx::query(
            r#"
            INSERT INTO sessions (id, project_path, parent_session_id, message_count)
            SELECT ?, project_path, id, message_count
            FROM sessions WHERE id = ?
            "#,
        )
        .bind(&new_id.0)
        .bind(&parent_id.0)
        .execute(&self.pool)
        .await?;

        // Copy messages from parent
        sqlx::query(
            r#"
            INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id, created_at)
            SELECT ?, role, content, tool_calls, tool_call_id, created_at
            FROM messages WHERE session_id = ?
            "#,
        )
        .bind(&new_id.0)
        .bind(&parent_id.0)
        .execute(&self.pool)
        .await?;

        Ok(new_id)
    }

    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>> {
        let row = sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT id, created_at, updated_at, project_path, message_count, parent_session_id
            FROM sessions WHERE id = ?
            "#,
        )
        .bind(&id.0)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(|r| SessionRecord {
            id: r.id,
            created_at: r.created_at,
            updated_at: r.updated_at,
            project_path: r.project_path,
            message_count: r.message_count,
            parent_session_id: r.parent_session_id,
        }))
    }

    async fn list_sessions(&self, project_path: &Path) -> Result<Vec<SessionRecord>> {
        let path_str = project_path.to_string_lossy();

        let rows = sqlx::query_as::<_, SessionRow>(
            r#"
            SELECT id, created_at, updated_at, project_path, message_count, parent_session_id
            FROM sessions WHERE project_path = ?
            ORDER BY updated_at DESC
            "#,
        )
        .bind(path_str.as_ref())
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| SessionRecord {
                id: r.id,
                created_at: r.created_at,
                updated_at: r.updated_at,
                project_path: r.project_path,
                message_count: r.message_count,
                parent_session_id: r.parent_session_id,
            })
            .collect())
    }

    async fn delete_session(&self, id: &SessionId) -> Result<()> {
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(&id.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn append_messages(&self, session_id: &SessionId, messages: &[Message]) -> Result<()> {
        for msg in messages {
            let tool_calls_json = msg
                .tool_calls
                .as_ref()
                .map(|tc| serde_json::to_string(tc).unwrap_or_default());

            sqlx::query(
                r#"
                INSERT INTO messages (session_id, role, content, tool_calls, tool_call_id)
                VALUES (?, ?, ?, ?, ?)
                "#,
            )
            .bind(&session_id.0)
            .bind(format!("{:?}", msg.role).to_lowercase())
            .bind(&msg.content)
            .bind(tool_calls_json)
            .bind(&msg.tool_call_id)
            .execute(&self.pool)
            .await?;
        }

        // Update message count
        sqlx::query(
            r#"
            UPDATE sessions
            SET message_count = (SELECT COUNT(*) FROM messages WHERE session_id = ?),
                updated_at = CURRENT_TIMESTAMP
            WHERE id = ?
            "#,
        )
        .bind(&session_id.0)
        .bind(&session_id.0)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    async fn get_messages(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        let rows = sqlx::query_as::<_, MessageRow>(
            r#"
            SELECT role, content, tool_calls, tool_call_id, created_at
            FROM messages WHERE session_id = ?
            ORDER BY id ASC
            "#,
        )
        .bind(&session_id.0)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|r| Message {
                role: parse_role(&r.role),
                content: r.content,
                tool_calls: r
                    .tool_calls
                    .and_then(|tc| serde_json::from_str(&tc).ok()),
                tool_call_id: r.tool_call_id,
                created_at: r.created_at,
            })
            .collect())
    }

    async fn update_summary(&self, session_id: &SessionId, summary: &str) -> Result<()> {
        sqlx::query("UPDATE sessions SET summary = ? WHERE id = ?")
            .bind(summary)
            .bind(&session_id.0)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn get_summary(&self, session_id: &SessionId) -> Result<Option<String>> {
        let row: Option<(String,)> = sqlx::query_as("SELECT summary FROM sessions WHERE id = ?")
            .bind(&session_id.0)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(|r| r.0))
    }
}

#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    project_path: String,
    message_count: i32,
    parent_session_id: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MessageRow {
    role: String,
    content: String,
    tool_calls: Option<String>,
    tool_call_id: Option<String>,
    created_at: chrono::DateTime<chrono::Utc>,
}

fn expand_path(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return path.replacen("~", &home, 1);
        }
    }
    path.to_string()
}

fn parse_role(role: &str) -> nekoclaw_shared::types::Role {
    match role {
        "system" => nekoclaw_shared::types::Role::System,
        "user" => nekoclaw_shared::types::Role::User,
        "assistant" => nekoclaw_shared::types::Role::Assistant,
        "tool" => nekoclaw_shared::types::Role::Tool,
        _ => nekoclaw_shared::types::Role::User,
    }
}
```

- [ ] **Step 2: Test SQLite compiles**

Run: `cargo check -p nekoclaw-adapters`
Expected: Clean compile

- [ ] **Step 3: Commit**

```bash
git add crates/adapters/src/sqlite.rs
git commit -m "feat(adapters): add SQLite storage implementation

- Session CRUD operations
- Message storage with JSON tool_calls
- Session forking with message copy
- Summary storage for context compaction
- Database migrations on startup"
```

### Task 11: Bash & File Tools

**Files:**
- Create: `crates/adapters/src/bash_tool.rs`
- Create: `crates/adapters/src/file_tool.rs`

- [ ] **Step 1: Implement BashTool**

Create `crates/adapters/src/bash_tool.rs`:
```rust
use anyhow::Result;
use async_trait::async_trait;
use nekoclaw_core::tool::Tool;
use nekoclaw_shared::types::ToolOutput;
use serde_json::Value;
use std::process::Stdio;
use tokio::process::Command;

pub struct BashTool {
    working_dir: std::path::PathBuf,
}

impl BashTool {
    pub fn new(working_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
        }
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command in the working directory"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The bash command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in seconds (default: 60)",
                    "default": 60
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput> {
        let command = args["command"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'command' argument"))?;

        let timeout_secs = args["timeout"].as_u64().unwrap_or(60);

        let output = Command::new("bash")
            .arg("-c")
            .arg(command)
            .current_dir(&self.working_dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .timeout(tokio::time::Duration::from_secs(timeout_secs))
            .output()
            .await?;

        Ok(ToolOutput {
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            exit_code: output.status.code().unwrap_or(-1),
        })
    }

    fn requires_confirmation(&self) -> bool {
        true
    }
}
```

- [ ] **Step 2: Implement FileTool**

Create `crates/adapters/src/file_tool.rs`:
```rust
use anyhow::Result;
use async_trait::async_trait;
use nekoclaw_core::tool::Tool;
use nekoclaw_shared::types::ToolOutput;
use serde_json::Value;
use std::path::PathBuf;

pub struct FileTool {
    base_dir: PathBuf,
}

impl FileTool {
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    /// Resolve path relative to base_dir, preventing directory traversal
    fn resolve_path(&self, relative: &str) -> Result<PathBuf> {
        let path = self.base_dir.join(relative);
        let canonical = path.canonicalize().unwrap_or(path);

        // Prevent directory traversal
        if !canonical.starts_with(&self.base_dir) {
            return Err(anyhow::anyhow!("Path escapes base directory: {}", relative));
        }

        Ok(canonical)
    }
}

#[async_trait]
impl Tool for FileTool {
    fn name(&self) -> &str {
        "file"
    }

    fn description(&self) -> &str {
        "Read, write, or modify files in the working directory"
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["read", "write", "append", "delete", "list"],
                    "description": "The file operation to perform"
                },
                "path": {
                    "type": "string",
                    "description": "Relative path to the file"
                },
                "content": {
                    "type": "string",
                    "description": "Content for write/append operations"
                }
            },
            "required": ["action", "path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<ToolOutput> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'action' argument"))?;
        let path_str = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing 'path' argument"))?;

        let path = self.resolve_path(path_str)?;

        match action {
            "read" => {
                let content = tokio::fs::read_to_string(&path).await?;
                Ok(ToolOutput::new(content, ""))
            }
            "write" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'content' for write"))?;

                // Ensure parent directory exists
                if let Some(parent) = path.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }

                tokio::fs::write(&path, content).await?;
                Ok(ToolOutput::new("File written successfully", ""))
            }
            "append" => {
                let content = args["content"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("Missing 'content' for append"))?;
                tokio::fs::OpenOptions::new()
                    .append(true)
                    .create(true)
                    .open(&path)
                    .await?;
                tokio::fs::write(&path, content).await?;
                Ok(ToolOutput::new("Content appended successfully", ""))
            }
            "delete" => {
                tokio::fs::remove_file(&path).await?;
                Ok(ToolOutput::new("File deleted successfully", ""))
            }
            "list" => {
                let mut entries = tokio::fs::read_dir(&path).await?;
                let mut result = String::new();
                while let Some(entry) = entries.next_entry().await? {
                    let name = entry.file_name();
                    let meta = entry.metadata().await.ok();
                    let is_dir = meta.map(|m| m.is_dir()).unwrap_or(false);
                    result.push_str(&format!("{}{}\n", name.to_string_lossy(), if is_dir { "/" } else { "" }));
                }
                Ok(ToolOutput::new(result, ""))
            }
            _ => Err(anyhow::anyhow!("Unknown action: {}", action)),
        }
    }

    fn requires_confirmation(&self) -> bool {
        true
    }

    async fn is_allowed(&self, args: &Value) -> Result<bool> {
        // Sandbox check - validate path doesn't escape base_dir
        if let Some(path) = args["path"].as_str() {
            return self.resolve_path(path).map(|_| true);
        }
        Ok(false)
    }
}
```

- [ ] **Step 3: Test tools compile**

Run: `cargo check -p nekoclaw-adapters`
Expected: Clean compile

- [ ] **Step 4: Commit**

```bash
git add crates/adapters/src/bash_tool.rs crates/adapters/src/file_tool.rs
git commit -m "feat(adapters): add Bash and File tools

- BashTool: execute commands with timeout
- FileTool: read/write/append/delete/list operations
- Path validation to prevent directory traversal
- Both require user confirmation by default"
```

---

## Phase 7: App Crate - Session & Coordinator

### Task 12: Create App Crate

**Files:**
- Create: `crates/app/Cargo.toml`
- Create: `crates/app/src/lib.rs`
- Create: `crates/app/src/session.rs`
- Create: `crates/app/src/coordinator.rs`

- [ ] **Step 1: Create app Cargo.toml**

Create `crates/app/Cargo.toml`:
```toml
[package]
name = "nekoclaw-app"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[dependencies]
nekoclaw-shared = { workspace = true }
nekoclaw-core = { workspace = true }
nekoclaw-adapters = { workspace = true }

# Async
tokio = { workspace = true }
async-trait = { workspace = true }

# Error handling
anyhow = { workspace = true }
tracing = { workspace = true }

# Utils
serde = { workspace = true }
serde_json = { workspace = true }
chrono = { workspace = true }
```

- [ ] **Step 2: Implement Session management**

Create `crates/app/src/session.rs`:
```rust
use anyhow::Result;
use nekoclaw_core::{
    agent::{Agent, AgentConfig, AgentState},
    bus::EventBus,
    provider::ModelProvider,
    storage::Storage,
    tool::{ToolRegistry, ToolSandbox},
};
use nekoclaw_shared::types::{SessionId, AgentId};
use std::sync::Arc;
use tokio::sync::mpsc;

/// Manages an active session with one or more agents
pub struct Session {
    id: SessionId,
    config: SessionConfig,
    event_bus: EventBus,
    storage: Arc<dyn Storage>,
    provider: Arc<dyn ModelProvider>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    /// Main agent for this session
    main_agent: Option<AgentId>,
    /// Channel for sending input to the main agent
    input_tx: Option<mpsc::Sender<String>>,
}

#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub agent: AgentConfig,
    pub project_path: std::path::PathBuf,
}

impl Session {
    pub fn new(
        id: SessionId,
        config: SessionConfig,
        event_bus: EventBus,
        storage: Arc<dyn Storage>,
        provider: Arc<dyn ModelProvider>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
    ) -> Self {
        Self {
            id,
            config,
            event_bus,
            storage,
            provider,
            tool_registry,
            sandbox,
            main_agent: None,
            input_tx: None,
        }
    }

    /// Initialize the session - create in storage and spawn main agent
    pub async fn init(&mut self) -> Result<()> {
        // Create session in storage
        self.storage.create_session(&self.config.project_path).await?;

        // Spawn main agent
        self.spawn_main_agent().await?;

        Ok(())
    }

    /// Spawn the main agent for this session
    async fn spawn_main_agent(&mut self) -> Result<()> {
        let (input_tx, input_rx) = mpsc::channel(100);
        self.input_tx = Some(input_tx);

        let agent = Agent::new(
            AgentId::new(),
            self.config.agent.clone(),
            self.event_bus.clone(),
            self.provider.clone(),
            self.storage.clone(),
            self.tool_registry.clone(),
            self.sandbox.clone(),
            input_rx,
        );

        let agent_id = agent.id().clone();
        agent.spawn();
        self.main_agent = Some(agent_id);

        tracing::info!("Main agent {} spawned for session {}", agent_id, self.id.0);
        Ok(())
    }

    /// Send user message to the session
    pub async fn send_message(&self, content: String) -> Result<()> {
        if let Some(ref tx) = self.input_tx {
            tx.send(content).await?;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Session not initialized"))
        }
    }

    /// Get session ID
    pub fn id(&self) -> &SessionId {
        &self.id
    }

    /// Get main agent ID
    pub fn main_agent_id(&self) -> Option<&AgentId> {
        self.main_agent.as_ref()
    }
}
```

- [ ] **Step 3: Implement Coordinator**

Create `crates/app/src/coordinator.rs`:
```rust
use crate::session::{Session, SessionConfig};
use anyhow::Result;
use nekoclaw_core::{
    bus::EventBus,
    provider::ModelProvider,
    storage::Storage,
    tool::{ToolRegistry, ToolSandbox},
};
use nekoclaw_shared::types::SessionId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Coordinates multiple sessions - the "server" in client/server architecture
pub struct Coordinator {
    event_bus: EventBus,
    storage: Arc<dyn Storage>,
    provider: Arc<dyn ModelProvider>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    sessions: RwLock<HashMap<SessionId, Arc<RwLock<Session>>>>,
}

impl Coordinator {
    pub fn new(
        event_bus: EventBus,
        storage: Arc<dyn Storage>,
        provider: Arc<dyn ModelProvider>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
    ) -> Self {
        Self {
            event_bus,
            storage,
            provider,
            tool_registry,
            sandbox,
            sessions: RwLock::new(HashMap::new()),
        }
    }

    /// Create and initialize a new session
    pub async fn create_session(&self, config: SessionConfig) -> Result<SessionId> {
        let id = SessionId::new();
        let mut session = Session::new(
            id.clone(),
            config,
            self.event_bus.clone(),
            self.storage.clone(),
            self.provider.clone(),
            self.tool_registry.clone(),
            self.sandbox.clone(),
        );

        session.init().await?;

        let session_id = session.id().clone();
        self.sessions
            .write()
            .await
            .insert(session_id.clone(), Arc::new(RwLock::new(session)));

        tracing::info!("Session {} created", session_id.0);
        Ok(session_id)
    }

    /// Get a session by ID
    pub async fn get_session(&self, id: &SessionId) -> Option<Arc<RwLock<Session>>> {
        self.sessions.read().await.get(id).cloned()
    }

    /// List all active sessions
    pub async fn list_sessions(&self) -> Vec<SessionId> {
        self.sessions.read().await.keys().cloned().collect()
    }

    /// Send message to a session
    pub async fn send_message(&self, session_id: &SessionId, content: String) -> Result<()> {
        let session = self
            .get_session(session_id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Session not found: {}", session_id.0))?;

        session.read().await.send_message(content).await
    }

    /// Get event bus for subscribing to events
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }
}
```

- [ ] **Step 4: Create app lib.rs**

Create `crates/app/src/lib.rs`:
```rust
pub mod coordinator;
pub mod session;

pub use coordinator::Coordinator;
pub use session::{Session, SessionConfig};
```

- [ ] **Step 5: Test app compiles**

Run: `cargo check -p nekoclaw-app`
Expected: Clean compile

- [ ] **Step 6: Commit**

```bash
git add crates/app/
git commit -m "feat(app): add Session and Coordinator

- Session manages a conversation with main agent
- Coordinator manages multiple sessions
- Supports multi-session architecture for future server mode
- Async RwLock for concurrent session access"
```

---

## Phase 8: CLI & TUI

### Task 13: CLI Entry Point

**Files:**
- Create: `crates/cli/Cargo.toml`
- Create: `crates/cli/src/main.rs`

- [ ] **Step 1: Create CLI crate**

Create `crates/cli/Cargo.toml`:
```toml
[package]
name = "nekoclaw-cli"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[[bin]]
name = "nekoclaw"
path = "src/main.rs"

[dependencies]
nekoclaw-shared = { workspace = true }
nekoclaw-core = { workspace = true }
nekoclaw-adapters = { workspace = true }
nekoclaw-app = { workspace = true }

# CLI
clap = { workspace = true }
config = { workspace = true }

# Async
tokio = { workspace = true }

# Error handling
anyhow = { workspace = true }
tracing = { workspace = true }
tracing-subscriber = { workspace = true }

# Utils
directories = "5.0"
```

- [ ] **Step 2: Implement main.rs**

Create `crates/cli/src/main.rs`:
```rust
use anyhow::Result;
use clap::Parser;
use nekoclaw_adapters::{BashTool, FileTool, OpenAIProvider, SqliteStorage};
use nekoclaw_app::{Coordinator, SessionConfig};
use nekoclaw_core::{
    agent::AgentConfig,
    bus::EventBus,
    storage::StorageConfig,
    tool::{ToolRegistry, ToolSandbox, enable_yolo_mode},
    provider::ModelConfig,
};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser)]
#[command(name = "nekoclaw")]
#[command(about = "AI coding assistant CLI")]
struct Args {
    /// Working directory (defaults to current)
    #[arg(short, long)]
    directory: Option<PathBuf>,

    /// Model ID to use
    #[arg(short, long)]
    model: Option<String>,

    /// API endpoint
    #[arg(long)]
    endpoint: Option<String>,

    /// API key
    #[arg(long)]
    api_key: Option<String>,

    /// Enable sandbox mode
    #[arg(long)]
    sandbox: bool,

    /// YOLO mode - skip all confirmations
    #[arg(long)]
    yolo: bool,

    /// Configuration file path
    #[arg(short, long)]
    config: Option<PathBuf>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    // Handle YOLO mode
    if args.yolo {
        enable_yolo_mode();
        tracing::warn!("YOLO mode enabled - all confirmations skipped!");
    }

    // Determine working directory
    let working_dir = args.directory.unwrap_or_else(|| std::env::current_dir().unwrap());
    let working_dir = working_dir.canonicalize()?;

    // Setup configuration
    let mut agent_config = AgentConfig::default();

    if let Some(model) = args.model {
        agent_config.model.model_id = model;
    }

    if let Some(endpoint) = args.endpoint {
        agent_config.model.endpoint = endpoint;
    }

    if let Some(api_key) = args.api_key {
        agent_config.model.api_key = api_key;
    } else {
        // Try to load from environment
        if let Ok(key) = std::env::var("OPENAI_API_KEY") {
            agent_config.model.api_key = key;
        }
    }

    // Setup storage
    let data_dir = directories::ProjectDirs::from("ai", "nekoclaw", "nekoclaw")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("~/.nekoclaw"));

    tokio::fs::create_dir_all(&data_dir).await?;

    let storage_config = StorageConfig {
        url: data_dir.join("sessions.db").to_string_lossy().to_string(),
        compaction_threshold: 100,
    };

    let storage = Arc::new(SqliteStorage::new(&storage_config).await?);

    // Setup provider
    let provider = Arc::new(OpenAIProvider::new()?);

    // Setup tools
    let mut tool_registry = ToolRegistry::new();
    tool_registry.register(Arc::new(BashTool::new(&working_dir)));
    tool_registry.register(Arc::new(FileTool::new(&working_dir)));

    // Setup sandbox
    let sandbox = if args.sandbox {
        ToolSandbox::new().enable()
    } else {
        ToolSandbox::default()
    };

    // Create event bus
    let event_bus = EventBus::new(1000);

    // Create coordinator
    let coordinator = Coordinator::new(
        event_bus.clone(),
        storage,
        provider,
        tool_registry,
        sandbox,
    );

    // Create session
    let session_config = SessionConfig {
        agent: agent_config,
        project_path: working_dir.clone(),
    };

    let session_id = coordinator.create_session(session_config).await?;

    println!("Nekoclaw session started: {}", session_id.0);
    println!("Working directory: {}", working_dir.display());
    println!("Type 'exit' to quit\n");

    // Simple REPL for now (TUI comes in Task 14)
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();

    loop {
        use std::io::Write;

        print!("> ");
        stdout.flush()?;

        let mut input = String::new();
        stdin.read_line(&mut input)?;
        let input = input.trim();

        if input == "exit" {
            break;
        }

        if input.is_empty() {
            continue;
        }

        // Send message to session
        if let Err(e) = coordinator.send_message(&session_id, input.to_string()).await {
            eprintln!("Error: {}", e);
        }

        // For now, just show that message was sent
        // In Task 14, we'll add event subscription and display
    }

    println!("Goodbye!");
    Ok(())
}
```

- [ ] **Step 3: Test CLI compiles**

Run: `cargo check -p nekoclaw-cli`
Expected: Clean compile

- [ ] **Step 4: Commit**

```bash
git add crates/cli/
git commit -m "feat(cli): add CLI entry point

- Argument parsing with clap
- Configuration via args or environment
- SQLite storage setup
- Tool registration (bash, file)
- Simple REPL for message input
- YOLO mode and sandbox flags"
```

### Task 14: TUI Implementation (Basic)

**Files:**
- Create: `crates/tui/Cargo.toml`
- Create: `crates/tui/src/lib.rs`
- Create: `crates/tui/src/app.rs`
- Create: `crates/tui/src/theme.rs`

- [ ] **Step 1: Create TUI crate**

Create `crates/tui/Cargo.toml`:
```toml
[package]
name = "nekoclaw-tui"
version.workspace = true
edition.workspace = true
authors.workspace = true
license.workspace = true

[dependencies]
nekoclaw-shared = { workspace = true }
nekoclaw-core = { workspace = true }

# TUI
ratatui = { workspace = true }
crossterm = { workspace = true }
pulldown-cmark = { workspace = true }

# Async
tokio = { workspace = true }
tokio-util = { workspace = true }

# Error handling
anyhow = { workspace = true }
tracing = { workspace = true }

# Utils
serde = { workspace = true }
unicode-width = "0.1"
```

- [ ] **Step 2: Implement Theme**

Create `crates/tui/src/theme.rs`:
```rust
use ratatui::style::{Color, Modifier, Style};

/// Dark theme for the TUI
#[derive(Debug, Clone)]
pub struct Theme {
    /// Background color
    pub background: Color,
    /// Foreground (text) color
    pub foreground: Color,
    /// Accent color for highlights
    pub accent: Color,
    /// Color for user messages
    pub user_color: Color,
    /// Color for assistant messages
    pub assistant_color: Color,
    /// Color for thinking blocks
    pub thinking_color: Color,
    /// Color for system/tool messages
    pub system_color: Color,
    /// Error color
    pub error_color: Color,
    /// Warning color
    pub warning_color: Color,
    /// Success color
    pub success_color: Color,
    /// Border color
    pub border_color: Color,
    /// Selection highlight
    pub selection: Color,
}

impl Default for Theme {
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// Dark theme (default)
    pub fn dark() -> Self {
        Self {
            background: Color::Black,
            foreground: Color::Rgb(200, 200, 200),
            accent: Color::Rgb(100, 150, 255),
            user_color: Color::Rgb(100, 200, 100),
            assistant_color: Color::Rgb(150, 150, 255),
            thinking_color: Color::Rgb(120, 120, 120),
            system_color: Color::Rgb(150, 150, 150),
            error_color: Color::Rgb(255, 100, 100),
            warning_color: Color::Rgb(255, 200, 100),
            success_color: Color::Rgb(100, 255, 100),
            border_color: Color::Rgb(60, 60, 60),
            selection: Color::Rgb(40, 40, 80),
        }
    }

    /// Base style
    pub fn base(&self) -> Style {
        Style::default()
            .fg(self.foreground)
            .bg(self.background)
    }

    /// User message style
    pub fn user(&self) -> Style {
        Style::default().fg(self.user_color)
    }

    /// Assistant message style
    pub fn assistant(&self) -> Style {
        Style::default().fg(self.assistant_color)
    }

    /// Thinking block style (dimmed, italic)
    pub fn thinking(&self) -> Style {
        Style::default()
            .fg(self.thinking_color)
            .add_modifier(Modifier::ITALIC | Modifier::DIM)
    }

    /// System/tool message style
    pub fn system(&self) -> Style {
        Style::default().fg(self.system_color)
    }

    /// Error style
    pub fn error(&self) -> Style {
        Style::default()
            .fg(self.error_color)
            .add_modifier(Modifier::BOLD)
    }

    /// Warning style
    pub fn warning(&self) -> Style {
        Style::default().fg(self.warning_color)
    }

    /// Success style
    pub fn success(&self) -> Style {
        Style::default().fg(self.success_color)
    }

    /// Border style
    pub fn border(&self) -> Style {
        Style::default().fg(self.border_color)
    }

    /// Accent/highlight style
    pub fn accent(&self) -> Style {
        Style::default()
            .fg(self.accent)
            .add_modifier(Modifier::BOLD)
    }
}
```

- [ ] **Step 3: Implement basic TUI App**

Create `crates/tui/src/app.rs`:
```rust
use crate::theme::Theme;
use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use nekoclaw_core::bus::EventBus;
use nekoclaw_core::event::{Event as AppEvent, ContentChunk};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::Style,
    text::{Line, Span, Text},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
    Frame, Terminal,
};
use std::io;
use tokio::sync::broadcast;

/// TUI Application
pub struct App {
    /// Input buffer
    input: String,
    /// Event history for display
    events: Vec<DisplayEvent>,
    /// Theme
    theme: Theme,
    /// Event receiver
    event_rx: broadcast::Receiver<AppEvent>,
    /// Input sender (to coordinator)
    input_tx: tokio::sync::mpsc::Sender<String>,
    /// Should quit
    should_quit: bool,
}

/// Event formatted for display
#[derive(Debug, Clone)]
enum DisplayEvent {
    User(String),
    Assistant { text: String, thinking: Option<String> },
    Thinking(String),
    Tool { name: String, output: String },
    System(String),
    Error(String),
}

impl App {
    pub fn new(
        event_bus: &EventBus,
        input_tx: tokio::sync::mpsc::Sender<String>,
    ) -> Self {
        Self {
            input: String::new(),
            events: Vec::new(),
            theme: Theme::default(),
            event_rx: event_bus.subscribe(),
            input_tx,
            should_quit: false,
        }
    }

    /// Run the TUI
    pub async fn run(&mut self) -> Result<()> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;

        // Main loop
        let result = self.run_loop(&mut terminal).await;

        // Restore terminal
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
        terminal.show_cursor()?;

        result
    }

    async fn run_loop<B: Backend>(&mut self, terminal: &mut Terminal<B>) -> Result<()> {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_millis(50));

        while !self.should_quit {
            // Draw UI
            terminal.draw(|f| self.draw(f))?;

            tokio::select! {
                // Handle timer ticks
                _ = interval.tick() => {}

                // Handle app events from event bus
                Ok(event) = self.event_rx.recv() => {
                    self.handle_app_event(event);
                }

                // Handle keyboard input
                _ = tokio::task::spawn_blocking(|| event::poll(std::time::Duration::from_millis(10))) => {
                    if let Ok(true) = event::poll(std::time::Duration::from_millis(0)) {
                        if let Ok(Event::Key(key)) = event::read() {
                            if key.kind == KeyEventKind::Press {
                                self.handle_key(key.code).await?;
                            }
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn handle_app_event(&mut self, event: AppEvent) {
        use nekoclaw_core::event::*;

        match event {
            AppEvent::User(UserEvent::Message { content }) => {
                self.events.push(DisplayEvent::User(content));
            }
            AppEvent::Model(ModelEvent::Chunk { content: ContentChunk::Text(text), .. }) => {
                // Append to last assistant message or create new one
                if let Some(DisplayEvent::Assistant { text: ref mut existing, .. }) = self.events.last_mut() {
                    existing.push_str(&text);
                } else {
                    self.events.push(DisplayEvent::Assistant { text, thinking: None });
                }
            }
            AppEvent::Model(ModelEvent::Chunk { content: ContentChunk::Thinking { thinking, .. }, .. }) => {
                // Append thinking to current assistant or create separate thinking block
                if let Some(DisplayEvent::Assistant { thinking: ref mut existing, .. }) = self.events.last_mut() {
                    if let Some(ref mut t) = existing {
                        t.push_str(&thinking);
                    } else {
                        *existing = Some(thinking);
                    }
                } else {
                    self.events.push(DisplayEvent::Thinking(thinking));
                }
            }
            AppEvent::Model(ModelEvent::Chunk { content: ContentChunk::RedactedThinking, .. }) => {
                self.events.push(DisplayEvent::Thinking("[Thinking redacted]".to_string()));
            }
            AppEvent::Tool(ToolEvent::Output { tool_name, output, .. }) => {
                self.events.push(DisplayEvent::Tool {
                    name: tool_name,
                    output,
                });
            }
            AppEvent::Tool(ToolEvent::Error { error, .. }) => {
                self.events.push(DisplayEvent::Error(error));
            }
            AppEvent::Agent(AgentEvent::Failed { error, .. }) => {
                self.events.push(DisplayEvent::Error(error));
            }
            _ => {}
        }
    }

    async fn handle_key(&mut self, key: KeyCode) -> Result<()> {
        match key {
            KeyCode::Char('q') => {
                self.should_quit = true;
            }
            KeyCode::Char('c') => {
                self.input.clear();
            }
            KeyCode::Char(c) => {
                self.input.push(c);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                if !self.input.is_empty() {
                    let content = self.input.clone();
                    self.input.clear();
                    self.input_tx.send(content).await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn draw<B: Backend>(&self, frame: &mut Frame<B>) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .margin(1)
            .constraints([Constraint::Min(1), Constraint::Length(3)])
            .split(frame.size());

        // Event display area
        let event_items: Vec<ListItem> = self
            .events
            .iter()
            .map(|event| {
                let (prefix, style, content) = match event {
                    DisplayEvent::User(text) => ("You: ", self.theme.user(), text.clone()),
                    DisplayEvent::Assistant { text, thinking } => {
                        let display = if let Some(t) = thinking {
                            format!("[Thinking] {}\n\n{}", t, text)
                        } else {
                            text.clone()
                        };
                        ("AI: ", self.theme.assistant(), display)
                    }
                    DisplayEvent::Thinking(t) => {
                        ("🧠 ", self.theme.thinking(), t.clone())
                    }
                    DisplayEvent::Tool { name, output } => (
                        &format!("Tool {}: ", name),
                        self.theme.system(),
                        output.clone(),
                    ),
                    DisplayEvent::System(text) => ("System: ", self.theme.system(), text.clone()),
                    DisplayEvent::Error(text) => ("Error: ", self.theme.error(), text.clone()),
                };

                let text = Text::styled(format!("{}{}", prefix, content), style);
                ListItem::new(text)
            })
            .collect();

        let events_widget = List::new(event_items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Nekoclaw")
                    .border_style(self.theme.border()),
            )
            .style(self.theme.base());

        frame.render_widget(events_widget, chunks[0]);

        // Input area
        let input_widget = Paragraph::new(self.input.as_str())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Input (Enter to send, q to quit)")
                    .border_style(self.theme.border()),
            )
            .style(self.theme.base());

        frame.render_widget(input_widget, chunks[1]);
    }
}
```

- [ ] **Step 4: Create TUI lib.rs**

Create `crates/tui/src/lib.rs`:
```rust
pub mod app;
pub mod theme;

pub use app::App;
pub use theme::Theme;
```

- [ ] **Step 5: Test TUI compiles**

Run: `cargo check -p nekoclaw-tui`
Expected: Clean compile

- [ ] **Step 6: Commit**

```bash
git add crates/tui/
git commit -m "feat(tui): add basic TUI implementation

- Dark theme with configurable colors
- Event display with different styles per type
- User input handling
- Event bus subscription for real-time updates
- Basic layout with event list and input area"
```

---

## Phase 9: Integration & Final Steps

### Task 15: Wire Up TUI in CLI

**Files:**
- Modify: `crates/cli/Cargo.toml`
- Modify: `crates/cli/src/main.rs`

- [ ] **Step 1: Add TUI dependency**

Update `crates/cli/Cargo.toml`:
```toml
nekoclaw-tui = { workspace = true }
```

- [ ] **Step 2: Integrate TUI in main**

Replace the simple REPL in `crates/cli/src/main.rs` with:

```rust
use nekoclaw_tui::App;

// ... after creating coordinator ...

// Create channel for input
let (input_tx, mut input_rx) = tokio::sync::mpsc::channel::<String>(100);

// Spawn task to forward input to coordinator
let coord_for_input = Arc::new(coordinator);
let session_id_for_input = session_id.clone();
tokio::spawn(async move {
    while let Some(content) = input_rx.recv().await {
        if let Err(e) = coord_for_input.send_message(&session_id_for_input, content).await {
            tracing::error!("Failed to send message: {}", e);
        }
    }
});

// Run TUI
let mut app = App::new(&event_bus, input_tx);
app.run().await?;
```

- [ ] **Step 3: Test full build**

Run: `cargo build --release`
Expected: Clean build

- [ ] **Step 4: Final commit**

```bash
git add crates/cli/
git commit -m "feat(cli): integrate TUI

- Replace simple REPL with full TUI
- Event-driven architecture with real-time updates
- Input forwarding to coordinator"
```

### Task 16: Add Missing Prompt Module

**Files:**
- Create: `crates/core/src/prompt.rs`

- [ ] **Step 1: Create PromptBuilder**

Create `crates/core/src/prompt.rs`:
```rust
use nekoclaw_shared::types::Message;

/// Builds prompts for agents
#[derive(Debug, Default)]
pub struct PromptBuilder {
    system: Option<String>,
    context: Vec<Message>,
}

impl PromptBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set system prompt
    pub fn system(mut self, prompt: impl Into<String>) -> Self {
        self.system = Some(prompt.into());
        self
    }

    /// Add context messages
    pub fn with_context(mut self, messages: Vec<Message>) -> Self {
        self.context = messages;
        self
    }

    /// Build final message list
    pub fn build(self) -> Vec<Message> {
        let mut messages = Vec::new();

        if let Some(system) = self.system {
            messages.push(Message::system(system));
        }

        messages.extend(self.context);
        messages
    }
}
```

- [ ] **Step 2: Test build**

Run: `cargo check -p nekoclaw-core`

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/prompt.rs
git commit -m "feat(core): add PromptBuilder

- Build system prompts with context
- Chainable API for configuration"
```

---

## Summary

This implementation plan covers:

1. **Phase 1**: Workspace setup with shared types
2. **Phase 2**: Core event system with broadcast bus
3. **Phase 3**: Provider and Storage traits
4. **Phase 4**: Tool system with registry and sandbox
5. **Phase 5**: Agent implementation with state machine
6. **Phase 6**: OpenAI provider and SQLite storage adapters
7. **Phase 7**: App layer with Session and Coordinator
8. **Phase 8**: CLI entry point and basic TUI
9. **Phase 9**: Integration and final wiring

**Key architectural decisions:**
- Core defines traits, adapters implement I/O (UNIX philosophy)
- Dependencies flow inward: adapters → core
- Event bus with tokio broadcast for multi-consumer support
- Single-threaded agent loop with periodic yielding
- Terminal states prevent zombie agents
- Sub-agents supported with async/sync modes
- YOLO mode and sandbox disabled by default

**Next Steps:**

**Two execution options:**

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

**Which approach?**
