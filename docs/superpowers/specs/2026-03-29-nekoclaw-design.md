# Nekoclaw Design Specification

**Status**: Approved
**Date**: 2026-03-29
**Author**: Design session with user

---

## 1. Overview

Nekoclaw is a production-grade AI coding assistant CLI tool written in Rust, inspired by Claude Code and OpenCode. It features a sophisticated agent loop with sub-agent support, elegant TUI interface with custom markdown rendering, and a flexible event-driven architecture designed for future multi-session server mode.

### Core Goals

1. **UNIX Philosophy** - Each module does one thing and does it well. Core defines interfaces, adapters implement I/O
2. **Async core with event bus** - Core has async agent runtime + trait definitions (NO concrete I/O implementations)
3. **Sophisticated agent loop** - State machine + hierarchical architecture, supporting sub-agents, async tasks, model fallback, and SSE idle timeout
4. **Elegant TUI** - Dark theme, custom markdown rendering, distinct event styles
5. **Testable core** - Core is pure logic + interfaces, easily testable with mock implementations

### Non-Goals

- Web UI (terminal-first)
- Built-in model hosting (cloud API only for MVP)
- Real-time collaboration features

---

## 2. Architecture

### 2.1 Monorepo Structure (UNIX Philosophy)

**Core Principle**: Each crate does one thing and does it well. Core defines interfaces, adapters implement them.

```
ekoclaw/
├── Cargo.toml              # Workspace definition
├── crates/
│   ├── core/               # Core business logic + async agent runtime
│   │   ├── src/
│   │   │   ├── agent/      # Agent state machine & async event loop
│   │   │   ├── bus.rs      # Event bus (async channels)
│   │   │   ├── event.rs    # Event type definitions
│   │   │   ├── provider.rs # LLM provider TRAITS (not implementations)
│   │   │   ├── tool.rs     # Tool TRAITS (not implementations)
│   │   │   ├── storage.rs  # Storage TRAITS (not implementations)
│   │   │   └── prompt.rs   # Prompt building logic
│   │   └── Cargo.toml      # Has tokio, NO reqwest, NO sqlx
│   │
│   ├── adapters/           # I/O implementations (depends on core)
│   │   ├── src/
│   │   │   ├── openai.rs   # OpenAI provider implementation
│   │   │   ├── anthropic.rs# Anthropic provider implementation
│   │   │   ├── sqlite.rs   # SQLite storage implementation
│   │   │   ├── bash_tool.rs# Bash tool implementation
│   │   │   └── file_tool.rs# File tool implementation
│   │   └── Cargo.toml      # Has reqwest, sqlx, etc.
│   │
│   ├── app/                # Application orchestration
│   │   ├── src/
│   │   │   ├── session.rs  # Session management
│   │   │   ├── config.rs   # Configuration
│   │   │   └── coordinator.rs
│   │   └── Cargo.toml
│   │
│   ├── cli/                # CLI entry point
│   │   └── src/
│   │       └── main.rs
│   │
│   ├── tui/                # Terminal UI
│   │   └── src/
│   │       └── ...
│   │
│   └── shared/             # Minimal shared types
│       └── src/
│           └── types.rs
│
└── docs/
    └── specs/
```

**Dependency Flow**:
```
cli/tui → app → core (uses traits)
                  ↑
            adapters (implements traits)
```

**Key Rules**:
1. `core` has **async** (for agent event loop), but **NO concrete I/O implementations**
2. `core` defines traits (`ModelProvider`, `Storage`, `Tool`), `adapters` implements them
3. `core` is independently testable with mock implementations
4. `adapters` depends on `core` (not the other way around)

### 2.2 Workspace Dependencies

```toml
[workspace]
members = ["crates/*"]
resolver = "2"

[workspace.dependencies]
# Core (async + traits, NO concrete I/O implementations)
tokio = { version = "1", features = ["full"] }
tokio-util = "0.7"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
anyhow = "1"
tracing = "0.1"
async-trait = "0.1"

# Adapters (concrete I/O implementations)
reqwest = { version = "0.12", features = ["rustls-tls", "stream", "json"] }
eventsource-stream = "0.2"
sqlx = { version = "0.8", features = ["runtime-tokio", "sqlite", "migrate"] }
futures = "0.3"

# TUI
ratatui = "0.29"
crossterm = "0.28"
pulldown-cmark = "0.12"

# CLI & Config
clap = { version = "4", features = ["derive"] }
config = "0.15"
tracing-subscriber = "0.3"

# Internal
nekoclaw-core = { path = "crates/core" }
nekoclaw-adapters = { path = "crates/adapters" }
```

---

## 3. Core Components (Traits + Async Agent)

Core defines **interfaces (traits)** and **async agent runtime**, but **NO concrete I/O implementations**.

### 3.1 Core Architecture

```rust
// crates/core/src/lib.rs

// Re-export domain types
pub mod event;
pub mod agent;
pub mod provider;  // Traits only
pub mod storage;   // Traits only
pub mod tool;      // Traits only
pub mod prompt;
pub mod bus;

// Core has async (for agent event loop) but NO concrete I/O deps
// - Uses tokio for async runtime
// - NO reqwest (HTTP is adapter's job)
// - NO sqlx (storage is adapter's job)
```

### 3.2 Event Types (Pure Data)

```rust
// crates/core/src/event.rs

/// Modular event types - each module defines its own
#[derive(Debug, Clone)]
pub enum Event {
    User(UserEvent),
    Agent(AgentEvent),
    Model(ModelEvent),
    Tool(ToolEvent),
    System(SystemEvent),
}

#[derive(Debug, Clone)]
pub enum UserEvent {
    Message { content: String },
    Confirm { tool_id: String, approved: bool },
    Interrupt,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    Started { agent_id: AgentId },
    StateChanged { agent_id: AgentId, state: AgentState },
    Completed { agent_id: AgentId, result: AgentResult },
    Failed { agent_id: AgentId, error: String },
    Cancelled { agent_id: AgentId },
    SubAgentSpawned { parent_id: AgentId, child_id: AgentId, mode: SubAgentMode },
    Progress { agent_id: AgentId, update: ProgressUpdate },
}

#[derive(Debug, Clone)]
pub enum ModelEvent {
    Request { agent_id: AgentId, messages: Vec<Message> },
    Chunk { agent_id: AgentId, content: String },
    Complete { agent_id: AgentId },
    Error { agent_id: AgentId, error: ProviderError },
}

#[derive(Debug, Clone)]
pub enum ToolEvent {
    Started { agent_id: AgentId, tool_call: ToolCall },
    Output { agent_id: AgentId, tool_id: String, output: ToolOutput },
    Error { agent_id: AgentId, tool_id: String, error: ToolError },
}
```

### 3.3 Async Event Bus (Multi-Consumer with Broadcast)

Event bus uses `tokio::sync::broadcast` for true fan-out - multiple subscribers can receive the same event (TUI, Logger, Recorder, etc.).

```rust
// crates/core/src/bus.rs

use tokio::sync::broadcast;

/// Async event bus with broadcast semantics
/// Multiple subscribers can receive the same event (TUI, Logger, Recorder, etc.)
pub struct EventBus {
    tx: broadcast::Sender<Event>,
}

impl EventBus {
    pub fn new(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Publish event to all subscribers (non-blocking)
    pub fn send(&self, event: Event) -> Result<()> {
        match self.tx.send(event) {
            Ok(_) => Ok(()),
            Err(broadcast::error::SendError(_)) => {
                // No active receivers, that's ok
                Ok(())
            }
        }
    }

    /// Subscribe to events
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tx.subscribe()
    }

    /// Get number of active subscribers
    pub fn subscriber_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

/// Inter-session message bus (for future server mode)
pub struct GlobalBus {
    subscribers: RwLock<HashMap<EventType, Vec<mpsc::UnboundedSender<Event>>>>,
}
```

### 3.2 Agent Loop (State Machine + Hierarchical + Terminal States)

Agent state machine with explicit terminal states for lifecycle completion.

```rust
// crates/core/src/agent/mod.rs

/// Agent states - all paths must converge to a terminal state
#[derive(Debug, Clone, PartialEq)]
pub enum AgentState {
    // Active states
    Idle,
    Thinking {
        started_at: Instant,
        timeout: Duration,
    },
    ToolExecuting {
        tool_id: String,
        abort_handle: AbortHandle,
    },
    WaitingHuman {
        question: String,
        response_tx: oneshot::Sender<String>,
    },
    Delegating {
        sub_agent_id: AgentId,
    },

    // Terminal states - agent lifecycle ends here
    Completed {
        result: AgentResult,
        finished_at: Instant,
    },
    Failed {
        error: AgentError,
        finished_at: Instant,
    },
    Cancelled {
        reason: String,
        finished_at: Instant,
    },
}

impl AgentState {
    /// Check if state is terminal (agent lifecycle complete)
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled { .. })
    }

    /// Check if state can transition to another
    pub fn can_transition_to(&self, new: &AgentState) -> bool {
        // Terminal states cannot transition to anything
        if self.is_terminal() {
            return false;
        }
        // ... other rules
        true
    }
}

/// Agent configuration (immutable)
pub struct AgentConfig {
    pub max_iterations: usize,
    pub sse_idle_timeout: Duration,
    pub thinking_timeout: Duration,
    pub tool_timeout: Duration,
    /// Global yolo mode - skip all confirmations
    pub yolo: bool,
}

/// Agent handle for external interaction
pub struct AgentHandle {
    pub id: AgentId,
    cmd_tx: mpsc::Sender<AgentCommand>,
    event_rx: mpsc::Receiver<Event>,
}

impl AgentHandle {
    /// Send command to agent
    pub async fn send(&self, cmd: AgentCommand) -> Result<()> {
        self.cmd_tx.send(cmd).await.map_err(|e| e.into())
    }

    /// Get next event from agent
    pub async fn next_event(&mut self) -> Option<Event> {
        self.event_rx.recv().await
    }
}

/// Commands that can be sent to the agent
pub enum AgentCommand {
    /// User message with pre-built system context
    UserMessage {
        content: String,
        system_context: String,  // Pre-built by PromptBuilder
    },
    ConfirmTool { tool_id: String, approved: bool },
    Cancel { reason: String },
    GetState,
}

/// Agent runtime - single-threaded state machine
/// NO locks - state is only modified by the event loop
pub struct Agent {
    id: AgentId,
    state: AgentState,           // Single owner, no RwLock
    context: Context,            // Single owner
    coordinator: Option<Arc<Coordinator>>,
    tools: Arc<ToolRegistry>,
    model: Arc<dyn ModelProvider>,
    fallback_models: Vec<Arc<dyn ModelProvider>>,
    bus: SessionBus,
    config: AgentConfig,
    cmd_rx: mpsc::Receiver<AgentCommand>,
}

impl Agent {
    /// Spawn a new agent and return handle
    pub fn spawn(
        config: AgentConfig,
        tools: Arc<ToolRegistry>,
        model: Arc<dyn ModelProvider>,
        bus: SessionBus,
    ) -> AgentHandle {
        let (cmd_tx, cmd_rx) = mpsc::channel(16);
        let (event_tx, event_rx) = mpsc::channel(100);

        let agent = Self {
            id: AgentId::new(),
            state: AgentState::Idle,
            context: Context::new(),
            coordinator: None,
            tools,
            model,
            fallback_models: Vec::new(),
            bus,
            config,
            cmd_rx,
        };

        // Spawn the agent loop on a dedicated task
        tokio::spawn(agent.run());

        AgentHandle {
            id: agent.id.clone(),
            cmd_tx,
            event_rx,
        }
    }

    /// Main event loop - SINGLE THREADED, owns all state
    /// No locks needed, state transitions are sequential
    /// Uses batching for model chunks to prevent starvation
    async fn run(mut self) -> Result<AgentResult> {
        // Emit started event
        self.emit(Event::Agent(AgentEvent::Started {
            agent_id: self.id.clone(),
        })).await?;

        loop {
            tokio::select! {
                // Handle commands from outside (high priority)
                Some(cmd) = self.cmd_rx.recv() => {
                    match cmd {
                        AgentCommand::UserMessage { content, system_context } => {
                            // Agent receives pre-built prompt with system context
                            let full_prompt = format!("{}\n\n{}", system_context, content);
                            self.handle_user_message(full_prompt).await?;
                        }
                        AgentCommand::ConfirmTool { tool_id, approved } => {
                            self.handle_tool_confirmation(tool_id, approved).await?;
                        }
                        AgentCommand::Cancel { reason } => {
                            self.transition_to(AgentState::Cancelled {
                                reason,
                                finished_at: Instant::now(),
                            }).await?;
                        }
                        AgentCommand::GetState => {
                            // Return current state info
                        }
                    }
                }

                // Batch model streaming chunks to prevent starvation
                // Process up to N chunks or until None, then yield
                biased; // Prioritize commands over streaming
                _ = self.process_model_chunks_batch(10).await => {
                    // Batched chunks processed
                }

                // Handle tool completion
                Some(result) = self.pending_tool.next() => {
                    self.handle_tool_complete(result).await?;
                }

                // Check for state-specific timeouts
                _ = self.check_timeouts() => {
                    // Handle thinking/tool timeout
                }
            }

            // Check if we reached terminal state
            match &self.state {
                AgentState::Completed { result, .. } => {
                    return Ok(result.clone());
                }
                AgentState::Failed { error, .. } => {
                    return Err(error.clone().into());
                }
                AgentState::Cancelled { reason, .. } => {
                    return Err(anyhow!("Agent cancelled: {}", reason));
                }
                _ => continue,
            }
        }
    }


    /// Batch process model chunks to prevent starvation of other branches
    /// Processes up to `max_chunks` or until stream yields None
    async fn process_model_chunks_batch(&mut self, max_chunks: usize) {
        for _ in 0..max_chunks {
            match tokio::time::timeout(
                Duration::from_millis(1),
                self.model_stream.next()
            ).await {
                Ok(Some(chunk)) => {
                    if let Err(e) = self.handle_model_chunk(chunk).await {
                        log::error!("Error handling model chunk: {}", e);
                    }
                }
                _ => break, // Timeout or None
            }
        }

        // Force yield to prevent starving other branches (tool completion, user commands, etc.)
        // Even if we're processing many chunks, let other select branches run
        tokio::task::yield_now().await;
    }

    /// State transition - only called from the event loop
    async fn transition_to(&mut self, new_state: AgentState) -> Result<()> {
        // Validate transition
        if !self.state.can_transition_to(&new_state) {
            bail!(
                "Invalid state transition: {:?} -> {:?}",
                self.state,
                new_state
            );
        }

        let old_state = std::mem::replace(&mut self.state, new_state.clone());

        // Cleanup old state if needed
        self.cleanup_state(&old_state).await?;

        // Emit state change event
        self.emit(Event::Agent(AgentEvent::StateChanged {
            agent_id: self.id.clone(),
            state: new_state,
        })).await?;

        Ok(())
    }

    async fn emit(&self, event: Event) -> Result<()> {
        self.bus.send(event).await
    }
}
```

#### State Machine Transitions

```
                         ┌──────────────┐
                         │   Completed  │◀────┐
                         └──────────────┘     │
                                               │
┌─────────┐     ┌──────────┐     ┌───────────┐ │   ┌──────────────┐
│  Idle   │────▶│ Thinking │────▶│ ToolExec  │─┴──▶│    Failed    │
└─────────┘     └──────────┘     └───────────┘     └──────────────┘
     │                │                  │
     │                ▼                  │          ┌──────────────┐
     │           ┌─────────┐             └─────────▶│  Cancelled   │
     │           │ Timeout │                          └──────────────┘
     │           └─────────┘  (SSE idle timeout)
     │
     ▼
┌─────────┐     ┌─────────────┐
│ Waiting │◀────│  Delegating │
│  Human  │     │ (Sub Agent) │
└─────────┘     └─────────────┘

All paths eventually converge to terminal states:
• Completed - Normal finish with result
• Failed - Error occurred
• Cancelled - User interrupt or timeout
```

### 3.3 Core Traits (Interface Definitions)

Core defines traits that adapters implement. This keeps core pure and testable.

#### ModelProvider Trait

```rust
// crates/core/src/provider.rs

use async_trait::async_trait;

/// LLM provider interface - core defines the trait
/// adapters (openai.rs, anthropic.rs) implement it
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    fn model_id(&self) -> &str;

    /// Non-streaming completion
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ModelResponse, ProviderError>;

    /// Streaming completion - returns a stream of chunks
    /// Core uses this, doesn't care about HTTP/SSE details
    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError>;
}

/// Provider configuration (data only, no I/O)
#[derive(Debug, Clone)]
pub struct ProviderConfig {
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
    pub timeout_secs: u64,
    pub sse_idle_timeout_secs: u64,
}
```

#### Storage Trait

```rust
// crates/core/src/storage.rs

use async_trait::async_trait;

/// Storage interface - core defines the trait
/// adapters (sqlite.rs) implement it
#[async_trait]
pub trait Storage: Send + Sync {
    /// Save a message to storage
    async fn save_message(&self, session_id: &str, message: &Message) -> Result<()>;

    /// Load session history
    async fn load_session(&self, session_id: &str) -> Result<Vec<Message>>;

    /// List sessions
    async fn list_sessions(&self, project_path: Option<&Path>) -> Result<Vec<SessionRecord>>;

    /// Fork a session
    async fn fork_session(
        &self,
        from_session_id: &str,
        new_project_path: Option<&Path>,
    ) -> Result<SessionRecord>;
}

/// Session record (data only)
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

#### Tool Trait

```rust
// crates/core/src/tool.rs

use async_trait::async_trait;

/// Tool interface - core defines the trait
/// adapters (bash_tool.rs, file_tool.rs) implement it
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value; // JSON schema
    fn permission_level(&self) -> PermissionLevel;

    /// Required permissions for this tool
    fn required_permissions(&self) -> PermissionSet;

    /// Execute the tool
    /// Core calls this, doesn't care about how it's implemented
    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError>;
}

/// Tool execution context
pub struct ToolContext {
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub sandbox: ToolSandbox,
}

/// Tool registry in core (pure logic, no I/O)
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    yolo: bool,
}

impl ToolRegistry {
    pub fn new(yolo: bool) -> Self {
        Self {
            tools: HashMap::new(),
            yolo,
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn needs_confirmation(&self, tool_name: &str) -> bool {
        if self.yolo {
            return false;
        }
        self.get(tool_name)
            .map(|t| matches!(t.permission_level(), PermissionLevel::Confirm))
            .unwrap_or(false)
    }
}
```

### 3.4 Sub-Agent (Hybrid Modes + Copy-on-Write Context)

Sub-agents support three modes with proper context isolation to prevent race conditions and prompt pollution.

```rust
// crates/core/src/agent/sub_agent.rs

pub enum SubAgentMode {
    /// Fully independent with own context (isolated)
    Detached,
    /// Lightweight task delegation with Copy-on-Write context
    /// - Reads from parent context (immutable)
    /// - Writes to local diff (isolated)
    /// - Optional merge back to parent on completion
    Task,
    /// Creates child coordinator for hierarchical tree
    Coordinator,
}

/// Copy-on-Write context for Task mode sub-agents
pub struct CowContext {
    /// Read-only reference to parent context
    parent: Arc<Context>,
    /// Local modifications (writes go here)
    local: RwLock<ContextDiff>,
}

impl CowContext {
    /// Read - check local first, then parent
    pub fn get(&self, key: &str) -> Option<Value> {
        // 1. Check local diff
        if let Some(val) = self.local.read().unwrap().get(key) {
            return Some(val.clone());
        }
        // 2. Fall back to parent
        self.parent.get(key).cloned()
    }

    /// Write - only to local diff
    pub fn set(&self, key: String, value: Value) {
        self.local.write().unwrap().insert(key, value);
    }

    /// Merge local changes back to parent context
    pub fn merge_to_parent(&self, parent: &mut Context) {
        for (k, v) in self.local.read().unwrap().iter() {
            parent.set(k.clone(), v.clone());
        }
    }
}

impl Agent {
    /// Spawn a fully independent sub-agent
    pub async fn spawn_detached(
        &self,
        task: String,
        tools: ToolSet,
        config: AgentConfig,
    ) -> Result<AgentHandle> {
        // Creates new agent with completely isolated context
        let context = Context::new();
        // ... spawn with fresh context
    }

    /// Spawn lightweight task sub-agent with COW context
    /// Prevents: race conditions, prompt pollution
    pub async fn spawn_task(
        &self,
        task: String,
        merge_back: bool,  // Whether to merge local changes back to parent
    ) -> Result<AgentHandle> {
        // Creates COW context:
        // - Read from parent's context (immutable)
        // - Write to local diff (isolated)
        let cow_context = CowContext {
            parent: Arc::clone(&self.context),
            local: RwLock::new(ContextDiff::new()),
        };
        // ... spawn with COW context
    }

    /// Spawn a child coordinator
    pub async fn spawn_coordinator(
        &self,
        sub_tasks: Vec<String>,
    ) -> Result<CoordinatorHandle> {
        // Creates hierarchical tree structure
        // Coordinator manages multiple sub-agents
    }
}

/// Handle to running sub-agent
pub struct AgentHandle {
    id: AgentId,
    progress: mpsc::Receiver<ProgressUpdate>,
    result: oneshot::Receiver<AgentResult>,
    abort: AbortHandle,
    /// Local changes from COW context (only for Task mode)
    local_diff: Option<ContextDiff>,
}

impl AgentHandle {
    /// Merge sub-agent's local changes back to parent context
    pub fn merge_changes(&self, parent: &mut Context) -> Result<()> {
        if let Some(diff) = &self.local_diff {
            for (k, v) in diff.iter() {
                parent.set(k.clone(), v.clone());
            }
        }
        Ok(())
    }
}
```

### 3.4 Model Provider & Fallback

```rust
// crates/core/src/provider/mod.rs

#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &str;
    fn model_id(&self) -> &str;

    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ModelResponse, ProviderError>;

    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError>;
}

/// Provider with SSE idle timeout handling
pub struct TimeoutProvider {
    inner: Arc<dyn ModelProvider>,
    idle_timeout: Duration,
}

impl TimeoutProvider {
    pub async fn stream_with_timeout(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError> {
        // Wraps stream with idle timeout detection
    }
}

/// Fallback chain
pub struct FallbackProvider {
    providers: Vec<Arc<dyn ModelProvider>>,
    current: AtomicUsize,
}

impl FallbackProvider {
    pub async fn complete_with_fallback(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ModelResponse, ProviderError> {
        // Try each provider in order until one succeeds
    }
}

/// Configuration (supports custom endpoint, api key, model id)
#[derive(Debug, Clone, Deserialize)]
pub struct ProviderConfig {
    pub endpoint: String,
    pub api_key: String,  // Supports $ENV_VAR syntax
    pub model: String,
    pub timeout: Duration,
    pub sse_idle_timeout: Duration,
}
```

### 3.5 Tool System (with Sandbox Context - MVP: Soft Sandbox)

Tool execution with **soft sandbox** for security isolation. **Note**: This is "soft" sandboxing - shell commands can potentially bypass restrictions using relative paths or command substitution. Production-grade hardening requires OS-level isolation.

**MVP (Current)**: Soft sandbox - path validation, cwd restriction, env control
**Future**: Hard sandbox - chroot, namespaces (Linux), seccomp

```rust
// crates/core/src/tool/mod.rs

/// Tool sandbox - SOFT sandbox for tool execution
/// WARNING: Shell commands can bypass (e.g., `cat ../../../../etc/passwd`, `$(...)`)
/// For production security, consider: chroot, Linux namespaces, seccomp
#[derive(Debug, Clone)]
pub struct ToolSandbox {
    /// Whether sandbox is enabled (can be disabled for trusted environments)
    pub enabled: bool,
    /// Working directory (tools should not escape this)
    /// NOTE: Shell commands can use relative paths to escape
    pub cwd: PathBuf,
    /// Environment variables available to tool
    pub env: HashMap<String, String>,
    /// Allowed operations
    pub permissions: PermissionSet,
    /// Read-only paths (can read, cannot write)
    pub read_only_paths: Vec<PathBuf>,
    /// Writable paths
    pub writable_paths: Vec<PathBuf>,
    /// Maximum execution time
    pub timeout: Duration,
}

/// Hard sandbox for production (future)
#[cfg(feature = "hard-sandbox")]
pub struct HardToolSandbox {
    /// Use chroot to restrict filesystem access
    pub root_dir: PathBuf,
    /// Linux namespaces
    pub use_namespaces: bool,
    /// Seccomp BPF filter
    pub seccomp_filter: Vec<AllowedSyscall>,
}

#[derive(Debug, Clone, Default)]
pub struct PermissionSet {
    pub read_fs: bool,
    pub write_fs: bool,
    pub execute_shell: bool,
    pub network: bool,
    pub environment: bool,
}

impl ToolSandbox {
    /// Create default sandbox for current project (disabled by default for MVP)
    /// Use `with_sandbox()` to enable
    pub fn for_project(project_path: &Path) -> Self {
        Self {
            enabled: false,  // Default: disabled for MVP
            cwd: project_path.to_path_buf(),
            env: std::env::vars().collect(),
            permissions: PermissionSet {
                read_fs: true,
                write_fs: true,
                execute_shell: true,
                network: false,
                environment: true,
            },
            read_only_paths: vec![],
            writable_paths: vec![project_path.to_path_buf()],
            timeout: Duration::from_secs(30),
        }
    }

    /// Enable sandbox (opt-in)
    pub fn with_sandbox(mut self) -> Self {
        self.enabled = true;
        self
    }

    /// Disable sandbox explicitly
    pub fn without_sandbox(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Validate path is within allowed boundaries
    pub fn check_path_access(&self, path: &Path, write: bool,
    ) -> Result<()> {
        // If sandbox is disabled, allow all access
        if !self.enabled {
            return Ok(());
        }

        let canonical = path.canonicalize()?;

        // Check against writable paths
        if write {
            if !self.writable_paths.iter().any(|p| canonical.starts_with(p)) {
                bail!(
                    "Path {} is outside of writable directories",
                    path.display()
                );
            }
        }

        // Check against blocked paths (e.g., ~/.ssh, /etc)
        if is_blocked_path(&canonical) {
            bail!("Access to {} is not allowed", path.display());
        }

        Ok(())
    }
}

/// Tool execution context
pub struct ToolContext {
    pub sandbox: ToolSandbox,
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub bus: SessionBus,  // For emitting progress events
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> serde_json::Value;  // JSON schema
    fn permission_level(&self) -> PermissionLevel;

    /// Check if tool can run in given sandbox
    fn required_permissions(&self) -> PermissionSet;

    async fn execute(
        &self,
        params: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError>;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PermissionLevel {
    Auto,      // Execute without confirmation
    Confirm,   // Require user confirmation
    Deny,      // Disabled
}

/// Tool registry with sandbox-aware execution
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    permissions: ToolPermissions,
    /// Global yolo mode - skip all confirmations
    yolo: bool,
}

impl ToolRegistry {
    pub fn new(yolo: bool) -> Self {
        Self {
            tools: HashMap::new(),
            permissions: ToolPermissions::default(),
            yolo,
        }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>);
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>>;
    pub fn list(&self) -> Vec<&dyn Tool>;
    pub fn to_openai_format(&self) -> Vec<ToolDefinition>;

    /// Check if tool needs user confirmation
    /// Returns false if yolo mode is enabled
    pub fn needs_confirmation(&self, tool_name: &str) -> bool {
        if self.yolo {
            return false;
        }

        if let Some(tool) = self.get(tool_name) {
            matches!(tool.permission_level(), PermissionLevel::Confirm)
        } else {
            false
        }
    }

    /// Execute tool with sandbox validation
    pub async fn execute(
        &self,
        tool_name: &str,
        params: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let tool = self.get(tool_name)
            .ok_or_else(|| ToolError::UnknownTool(tool_name.to_string()))?;

        // Check sandbox permissions (if sandbox is enabled)
        if ctx.sandbox.enabled {
            let required = tool.required_permissions();
            if !ctx.sandbox.permissions.covers(&required) {
                return Err(ToolError::InsufficientPermissions {
                    tool: tool_name.to_string(),
                    required,
                    available: ctx.sandbox.permissions.clone(),
                });
            }
        }

        // Execute with timeout
        tokio::time::timeout(ctx.sandbox.timeout, tool.execute(params, ctx))
            .await
            .map_err(|_| ToolError::Timeout)?
    }
}

/// Built-in tools use sandbox for safe execution
pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str { "bash" }

    fn required_permissions(&self) -> PermissionSet {
        PermissionSet {
            execute_shell: true,
            read_fs: true,
            write_fs: false,
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        params: Value,
        ctx: &ToolContext,
    ) -> Result<ToolOutput, ToolError> {
        let cmd = params["command"].as_str()
            .ok_or_else(|| ToolError::InvalidParams("missing command"))?;

        // Run in sandbox cwd with restricted env
        let output = tokio::process::Command::new("bash")
            .arg("-c")
            .arg(cmd)
            .current_dir(&ctx.sandbox.cwd)
            .env_clear()
            .envs(&ctx.sandbox.env)
            .output()
            .await?;

        Ok(ToolOutput::new(output.stdout, output.stderr))
    }
}
```

---

## 4. Adapters (I/O Implementations)

Adapters crate implements the traits defined in core. This is where all I/O happens.

```
crates/adapters/src/
├── lib.rs
├── openai.rs      # OpenAI provider implementation
├── anthropic.rs   # Anthropic provider implementation
├── sqlite.rs      # SQLite storage implementation
├── bash_tool.rs   # Bash tool implementation
└── file_tool.rs   # File tool implementation
```

### 4.1 Provider Implementations

```rust
// crates/adapters/src/openai.rs

use nekoclaw_core::provider::{ModelProvider, ProviderConfig, ModelResponse, StreamChunk};
use reqwest;  // HTTP client - ONLY in adapters

pub struct OpenAiProvider {
    config: ProviderConfig,
    client: reqwest::Client,
}

#[async_trait]
impl ModelProvider for OpenAiProvider {
    fn name(&self) -> &str { "openai" }
    fn model_id(&self) -> &str { &self.config.model }

    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError> {
        // HTTP/SSE implementation using reqwest
        // Core doesn't know about HTTP details
    }
}
```

### 4.2 Storage Implementation

```rust
// crates/adapters/src/sqlite.rs

use nekoclaw_core::storage::{Storage, SessionRecord};
use sqlx;  // SQLite driver - ONLY in adapters

pub struct SqliteStorage {
    pool: sqlx::SqlitePool,
    data_dir: PathBuf,
}

#[async_trait]
impl Storage for SqliteStorage {
    async fn save_message(&self, session_id: &str, message: &Message) -> Result<()> {
        // SQLite implementation
        // Core doesn't know about SQL
    }

    async fn load_session(&self, session_id: &str) -> Result<Vec<Message>> {
        // SQLite implementation
    }
}
```

### 4.3 Tool Implementations

```rust
// crates/adapters/src/bash_tool.rs

use nekoclaw_core::tool::{Tool, ToolContext, ToolOutput};
use tokio::process::Command;  // Process execution - ONLY in adapters

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str { "bash" }

    async fn execute(&self, params: Value, ctx: &ToolContext) -> Result<ToolOutput, ToolError> {
        // Process execution
        // Core doesn't know about shell execution
        let output = Command::new("bash")
            .arg("-c")
            .arg(params["command"].as_str().unwrap())
            .current_dir(&ctx.sandbox.cwd)
            .output()
            .await?;

        Ok(ToolOutput::new(output.stdout, output.stderr))
    }
}
```

---

## 5. Storage Layer (Adapter Implementation)

### 5.1 Hybrid Storage (SQLite + Filesystem)

```rust
// crates/adapters/src/sqlite.rs (implements core::storage::Storage)

use nekoclaw_core::storage::{Storage, SessionRecord, StorageError};
use sqlx;

/// SQLite storage implementation of core's Storage trait
pub struct SqliteStorage {
    pool: sqlx::SqlitePool,
    content_dir: PathBuf,
}

impl SqliteStorage {
    pub async fn new(data_dir: &Path) -> Result<Self> {
        // Create SQLite pool
        let pool = sqlx::sqlite::SqlitePoolOptions::new()
            .connect(&format!("{}/nekoclaw.db", data_dir.display()))
            .await?;

        // Run migrations
        sqlx::migrate!("./migrations").run(&pool).await?;

        Ok(Self {
            pool,
            content_dir: data_dir.join("content"),
        })
    }
}

#[async_trait]
impl Storage for SqliteStorage {
    async fn save_message(&self, session_id: &str, message: &Message) -> Result<()> {
        // Implementation details hidden from core
        sqlx::query("INSERT INTO messages ...")
            .bind(session_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn load_session(&self, session_id: &str) -> Result<Vec<Message>> {
        // Implementation
    }

    async fn fork_session(
        &self,
        from_session_id: &str,
        new_project_path: Option<&Path>,
    ) -> Result<SessionRecord> {
        // Implementation
    }
}
    /// Save message (metadata to SQLite, large content to filesystem)
    pub async fn save_message(&self, session_id: &str, message: &Message) -> Result<()>;

    /// Load session history
    pub async fn load_session(&self, session_id: &str) -> Result<Vec<Message>>;

    /// List sessions
    pub async fn list_sessions(&self, project_path: Option<&Path>) -> Result<Vec<SessionRecord>>;

    /// Fork a session (create new branch from existing)
    pub async fn fork_session(
        &self,
        from_session_id: &str,
        new_project_path: Option<&Path>,
    ) -> Result<SessionRecord> {
        // 1. Copy session metadata with new ID
        // 2. Copy all messages
        // 3. Set parent_session_id reference
        // 4. Return new session record
    }

    /// Get session tree (for displaying branch structure)
    pub async fn get_session_tree(&self, root_session_id: &str) -> Result<SessionTree>;
}
```

### 4.2 Database Schema

```sql
-- migrations/001_initial.sql

CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    project_path TEXT NOT NULL,
    message_count INTEGER DEFAULT 0,
    metadata TEXT, -- JSON
    -- For session branching/forking
    parent_session_id TEXT,
    FOREIGN KEY (parent_session_id) REFERENCES sessions(id)
);

CREATE TABLE messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    role TEXT NOT NULL, -- 'user', 'assistant', 'system', 'tool'
    -- Content storage with explicit type
    content_type TEXT NOT NULL, -- 'inline' | 'file' | 'chunked' | 'compressed'
    content_ref TEXT NOT NULL,  -- actual content (if inline) or file path
    content_size INTEGER,       -- bytes (for chunked/large content)
    checksum TEXT,              -- for integrity verification
    tool_calls TEXT,            -- JSON array of tool calls
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

CREATE TABLE snapshots (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    file_path TEXT NOT NULL,
    content_path TEXT NOT NULL, -- Path to actual content file
    content_type TEXT,          -- 'raw' | 'diff' | 'compressed'
    size INTEGER,
    checksum TEXT,              -- content integrity
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
    FOREIGN KEY (session_id) REFERENCES sessions(id)
);

-- For deduplication and integrity
CREATE TABLE content_store (
    checksum TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    size INTEGER,
    ref_count INTEGER DEFAULT 1,
    created_at DATETIME DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_messages_session ON messages(session_id, created_at);
CREATE INDEX idx_sessions_project ON sessions(project_path);
CREATE INDEX idx_messages_content_type ON messages(content_type);
CREATE INDEX idx_snapshots_checksum ON snapshots(checksum);
```

---

## 5. TUI Design

### 5.1 Architecture (Event-Driven)

TUI uses event-driven architecture instead of polling to reduce CPU usage, especially over SSH.

```rust
// crates/tui/src/app.rs

pub struct App {
    // State
    session: SessionState,
    input: InputState,
    scroll: ScrollState,

    // Components
    chat: ChatComponent,
    input_box: InputComponent,
    sidebar: SidebarComponent,
    status_bar: StatusBarComponent,

    // Theme
    theme: Theme,

    // Event handling
    event_tx: mpsc::UnboundedSender<TuiEvent>,
    event_rx: mpsc::UnboundedReceiver<TuiEvent>,

    // Render state
    dirty: bool,  // Mark when redraw needed
}

pub enum TuiEvent {
    Tick,           // For animations/spinners
    Key(KeyEvent),
    Resize(u16, u16),
    CoreEvent(Event),  // From core event bus
    RenderRequest,  // Explicit render trigger
}

impl App {
    pub async fn run(&mut self) -> Result<()> {
        // Main event loop - event-driven, not polling
        loop {
            tokio::select! {
                // Handle incoming events
                Some(event) = self.event_rx.recv() => {
                    self.handle_event(event).await?;
                    self.dirty = true;  // Mark for redraw
                }

                // Periodic tick for animations (not for constant redraw)
                _ = tokio::time::sleep(Duration::from_millis(100)) => {
                    self.update_animations();
                    if self.has_active_animations() {
                        self.dirty = true;
                    }
                }
            }

            // Redraw only when dirty
            if self.dirty {
                self.draw()?;
                self.dirty = false;
            }
        }
    }

    /// Request explicit redraw
    fn request_render(&mut self) {
        self.dirty = true;
    }
}
```

### 5.2 Theme (Dark)

```rust
// crates/tui/src/theme/mod.rs

pub struct Theme {
    // Backgrounds
    pub bg_primary: Color,      // Main background
    pub bg_secondary: Color,    // Secondary panels
    pub bg_highlight: Color,    // Selection/hover

    // Accents
    pub accent_primary: Color,   // Main accent (blue-ish)
    pub accent_success: Color,   // Success states
    pub accent_warning: Color,   // Warnings
    pub accent_error: Color,     // Errors

    // Text
    pub text_primary: Color,     // Main text
    pub text_secondary: Color,   // Dimmed text
    pub text_muted: Color,       // Very dimmed

    // Syntax highlighting for code blocks
    pub syntax: SyntaxTheme,
}

impl Default for Theme {
    fn default() -> Self {
        Self {
            bg_primary: Color::Rgb(18, 18, 18),
            bg_secondary: Color::Rgb(28, 28, 28),
            bg_highlight: Color::Rgb(40, 40, 40),

            accent_primary: Color::Rgb(100, 150, 255),    // Soft blue
            accent_success: Color::Rgb(100, 200, 100),    // Soft green
            accent_warning: Color::Rgb(200, 180, 100),    // Soft yellow
            accent_error: Color::Rgb(200, 100, 100),      // Soft red

            text_primary: Color::Rgb(220, 220, 220),
            text_secondary: Color::Rgb(150, 150, 150),
            text_muted: Color::Rgb(100, 100, 100),

            syntax: SyntaxTheme::default(),
        }
    }
}
```

### 5.3 Event Rendering Styles

| Event Type | Visual Style |
|-----------|-------------|
| **Text** | Normal text, code blocks with background highlight and syntax highlighting |
| **Think** | Italic text, muted color (secondary), collapsible with `▼`/`▶` indicator |
| **Tool** | Icon prefix (🔧/✓/✗), status color indicator, expandable details |
| **SubAgent** | Tree indentation, spinner/progress when running, checkmark when done |
| **Error** | Red background strip, error icon, stack trace in expandable section |
| **User** | Right-aligned, distinct background color |

### 5.4 Custom Markdown Renderer (Streaming Support)

Streaming markdown renderer for real-time AI output without flickering.

**Key Design**: Use `pulldown_cmark` library for parsing (don't write custom parser state machine). Buffer content and re-parse on each chunk.

```rust
// crates/tui/src/renderer/markdown.rs

/// Streaming markdown renderer - handles incomplete/partial content
/// Uses pulldown_cmark for parsing (NOT custom parser state machine)
pub struct StreamingRenderer<'a> {
    theme: &'a Theme,
    width: u16,
    buffer: String,                    // Accumulated markdown
    last_rendered_len: usize,          // How much was already rendered
    cached_lines: Vec<Line<'a>>,       // Cached rendered output
    pending_events: Vec<Event<'a>>,    // Parse events for incremental rendering
}

impl<'a> StreamingRenderer<'a> {
    pub fn new(theme: &'a Theme, width: u16) -> Self {
        Self {
            theme,
            width,
            buffer: String::new(),
            last_rendered_len: 0,
            cached_lines: Vec::new(),
            pending_events: Vec::new(),
        }
    }

    /// Push a chunk of markdown (called for each SSE chunk)
    pub fn push_chunk(&mut self, chunk: &str) {
        self.buffer.push_str(chunk);
        self.render_incremental();
    }

    /// Re-render only the changed portion
    /// Strategy: Re-parse entire buffer, but only re-render from last stable point
    fn render_incremental(&mut self) {
        // 1. Parse entire buffer with pulldown_cmark
        let parser = pulldown_cmark::Parser::new(&self.buffer);

        // 2. Find stable render point (last complete block)
        let stable_point = self.find_stable_point();

        // 3. Only re-render from stable point onwards
        if stable_point > self.last_rendered_len {
            // Truncate cached lines to stable point
            self.cached_lines.truncate(self.line_count_at(stable_point));
            self.last_rendered_len = stable_point;
        }

        // 4. Render new content from stable point
        let new_content = &self.buffer[stable_point..];
        let new_lines = self.render_content(new_content);
        self.cached_lines.extend(new_lines);
    }

    /// Find last position where we have a complete markdown element
    /// This prevents re-rendering incomplete blocks
    fn find_stable_point(&self) -> usize {
        // Look backwards for:
        // - Complete paragraph (blank line after)
        // - Complete code block (``` close)
        // - Complete list item
        // - End of buffer if we're mid-block

        // Simplistic: find last '\n\n' or '\n```'
        self.buffer
            .rmatch_indices("\n\n")
            .chain(self.buffer.rmatch_indices("\n```"))
            .map(|(i, _)| i + 1)
            .next()
            .unwrap_or(0)
    }

    /// Render content to lines
    fn render_content(&self, content: &str) -> Vec<Line<'a>> {
        let parser = pulldown_cmark::Parser::new(content);
        let mut lines = Vec::new();

        for event in parser {
            match event {
                Event::Start(Tag::CodeBlock(lang)) => {
                    // Start code block styling
                }
                Event::Text(text) => {
                    lines.push(self.style_text(&text));
                }
                Event::End(Tag::CodeBlock(_)) => {
                    // End code block styling
                }
                // ... handle other events
            }
        }

        lines
    }

    /// Finalize and get complete rendered output
    pub fn flush(mut self) -> Vec<Line<'a>> {
        // Render any remaining content
        self.render_incremental();
        self.cached_lines
    }

    /// Get current rendered output (for real-time display)
    pub fn current_render(&self) -> &[Line<'a>] {
        &self.cached_lines
    }

    /// Check if we're in an incomplete block (for cursor styling)
    pub fn in_incomplete_block(&self) -> bool {
        // Check if buffer ends mid-block
        self.buffer.ends_with("```")
            || self.buffer.ends_with("<think>")
            || self.buffer.ends_with("**")
    }
}

/// Non-streaming renderer for complete content
pub struct MarkdownRenderer<'a> {
    theme: &'a Theme,
    width: u16,
}

impl<'a> MarkdownRenderer<'a> {
    pub fn render(&self, markdown: &str) -> Vec<Line<'a>> {
        let parser = pulldown_cmark::Parser::new(markdown);
        let mut lines = Vec::new();

        for event in parser {
            match event {
                Event::Start(Tag::CodeBlock(lang)) => {
                    // Render code block with syntax highlighting
                }
                Event::Text(text) => {
                    // Render text with appropriate styling
                }
                Event::End(Tag::Strong) => {
                    // Apply bold styling
                }
                // ... other events
            }
        }

        lines
    }
}
```

---

## 6. Project-Level Configuration

### 6.1 AGENTS.md

Project-specific instructions loaded on every prompt:

```markdown
# AGENTS.md

## Project Context

This is a Rust CLI tool for AI-assisted coding.

## Coding Standards

- Use async/await for I/O operations
- Prefer thiserror for error handling
- Write tests for all public APIs

## Important Files

- `crates/core/src/agent/` - Agent state machine
- `crates/tui/src/` - Terminal UI components

## Common Tasks

### Running tests
```bash
cargo test --workspace
```

### Adding a new tool
1. Implement `Tool` trait in `crates/core/src/tool/`
2. Register in `ToolRegistry`
3. Add tests
```

### 6.2 Skills

Reusable instruction modules in `.nekoclaw/skills/`:

```
project-root/
├── AGENTS.md
└── .nekoclaw/
    ├── config.toml      # Project-local config
    └── skills/
        ├── testing.md   # Testing patterns
        ├── api-design.md # API design guidelines
        └── refactoring.md # Refactoring strategies
```

Example skill (`testing.md`):

```markdown
# Testing Guidelines

## Unit Tests

- Test one concept per test
- Use descriptive test names
- Arrange-Act-Assert pattern

## Async Tests

```rust
#[tokio::test]
async fn test_async_operation() {
    // Use tokio::time::timeout for async operations
}
```

## Integration Tests

- Create temp directories with `tempfile::TempDir`
- Clean up resources in `Drop` impl
```

Skills are automatically loaded and included in prompt building.

### 6.3 PromptBuilder

Separate component responsible for building prompts. Agent receives pre-built prompts and doesn't know about AGENTS.md or skills.

```rust
// crates/core/src/prompt/mod.rs

/// Configuration for prompt building
pub struct PromptBuilderConfig {
    /// Path to AGENTS.md (configurable)
    pub agents_md_path: Option<PathBuf>,
    /// Paths to skill directories (configurable)
    pub skill_paths: Vec<PathBuf>,
    /// Base system prompt
    pub system_prompt: String,
}

impl Default for PromptBuilderConfig {
    fn default() -> Self {
        Self {
            agents_md_path: None, // Will look for AGENTS.md in project root
            skill_paths: vec![],  // Will look for .nekoclaw/skills/ by default
            system_prompt: "You are a helpful coding assistant.".to_string(),
        }
    }
}

/// Builds prompts by combining system context with user messages
pub struct PromptBuilder {
    config: PromptBuilderConfig,
    project_path: PathBuf,
}

impl PromptBuilder {
    pub fn new(config: PromptBuilderConfig, project_path: PathBuf) -> Self {
        Self { config, project_path }
    }

    /// Build complete prompt with system context
    pub async fn build_prompt(&self, user_message: &str) -> Result<String> {
        let mut parts = Vec::new();

        // 1. Base system prompt
        parts.push(self.config.system_prompt.clone());

        // 2. AGENTS.md (if exists)
        if let Some(content) = self.load_agents_md().await? {
            parts.push(format!("\n\n# Project Instructions (AGENTS.md)\n{}", content));
        }

        // 3. Skills (from configured paths)
        for skill in self.load_skills().await? {
            parts.push(format!("\n\n# Skill: {}\n{}", skill.name, skill.content));
        }

        // 4. User message
        parts.push(format!("\n\n# User Request\n{}", user_message));

        Ok(parts.join("\n"))
    }

    /// Build system context only (for reuse across messages)
    pub async fn build_system_context(&self) -> Result<String> {
        let mut parts = Vec::new();

        parts.push(self.config.system_prompt.clone());

        if let Some(content) = self.load_agents_md().await? {
            parts.push(format!("\n\n# Project Instructions (AGENTS.md)\n{}", content));
        }

        for skill in self.load_skills().await? {
            parts.push(format!("\n\n# Skill: {}\n{}", skill.name, skill.content));
        }

        Ok(parts.join("\n"))
    }

    /// Load AGENTS.md from configured path or project root
    async fn load_agents_md(&self) -> Result<Option<String>> {
        let path = self.config.agents_md_path.clone()
            .unwrap_or_else(|| self.project_path.join("AGENTS.md"));

        if path.exists() {
            Ok(Some(tokio::fs::read_to_string(&path).await?))
        } else {
            Ok(None)
        }
    }

    /// Load skills from configured paths
    async fn load_skills(&self) -> Result<Vec<Skill>> {
        let mut skills = Vec::new();

        // Use configured paths or default
        let paths = if self.config.skill_paths.is_empty() {
            vec![self.project_path.join(".nekoclaw/skills")]
        } else {
            self.config.skill_paths.clone()
        };

        for dir in paths {
            if !dir.exists() {
                continue;
            }

            let mut entries = tokio::fs::read_dir(&dir).await?;
            while let Some(entry) = entries.next_entry().await? {
                let path = entry.path();
                if path.extension().map_or(false, |ext| ext == "md") {
                    let content = tokio::fs::read_to_string(&path).await?;
                    let name = path.file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    skills.push(Skill { name, content });
                }
            }
        }

        Ok(skills)
    }
}

/// Loaded skill content
pub struct Skill {
    pub name: String,
    pub content: String,
}
```

---

## 7. Configuration

### 7.1 Configuration Hierarchy

Priority (low to high):
1. Built-in defaults
2. `/etc/nekoclaw/config.toml` (system)
3. `~/.config/nekoclaw/config.toml` (user)
4. `./.nekoclaw/config.toml` (project-local)
5. Environment variables (`NEKOCLAW_*`)
6. CLI arguments

### 7.2 Configuration Schema

```toml
# ~/.config/nekoclaw/config.toml

[provider]
endpoint = "https://api.openai.com/v1"
api_key = "$OPENAI_API_KEY"  # Environment variable substitution
model = "gpt-4o"

[[provider.fallback]]
model = "gpt-4o-mini"

[[provider.fallback]]
endpoint = "https://api.anthropic.com/v1"
api_key = "$ANTHROPIC_API_KEY"
model = "claude-3-sonnet-20240229"

[agent]
max_iterations = 50
sse_idle_timeout = 60  # seconds
thinking_indicator = true

[permissions]
read = "auto"      # auto, confirm, deny
write = "confirm"
bash = "confirm"
git = "confirm"

# ⚠️ Security settings
# yolo = false       # Skip all confirmations (use with caution!)
# sandbox = false    # Enable tool sandbox (default: false, opt-in)

[prompt]
# Custom path to AGENTS.md (default: AGENTS.md in project root)
# agents_md_path = "/path/to/custom/AGENTS.md"

# Additional skill directories (default: .nekoclaw/skills/)
# skill_paths = ["~/.config/nekoclaw/skills", "./custom-skills"]

# Custom system prompt
# system_prompt = "You are a helpful coding assistant..."

[ui]
theme = "dark"
show_thinking = true
auto_scroll = true

[storage]
data_dir = "~/.local/share/nekoclaw"
max_history_sessions = 100
snapshot_retention_days = 30
```

---

## 8. CLI Interface

```bash
# Start interactive session
nekoclaw

# Start with specific directory
nekoclaw /path/to/project

# Resume previous session
nekoclaw --resume <session-id>

# Fork/branch a session (create new branch from existing)
nekoclaw --fork <session-id>

# YOLO mode - skip all tool confirmations (⚠️ use with caution)
nekoclaw --yolo
nekoclaw --ask "Delete all test files" --yolo

# Enable sandbox for tool execution (MVP: soft sandbox)
ekoclaw --sandbox

# List sessions (with tree view for branched sessions)
nekoclaw sessions list
nekoclaw sessions tree  # Show branch structure

# Run single command (non-interactive)
nekoclaw --ask "Explain this codebase"

# Configuration
nekoclaw config get provider.model
nekoclaw config set provider.model gpt-4o-mini

# Version
nekoclaw --version
nekoclaw --help
```

---

## 9. Testing Strategy

### 9.1 Unit Tests

- Core business logic in `core` crate
- Mock providers for deterministic testing
- State machine transition validation

### 9.2 Integration Tests

- Full agent loop with mock LLM
- Tool execution in isolated temp directories
- Storage layer with in-memory SQLite

### 9.3 TUI Tests

- Component rendering tests
- Event handling validation
- Theme application verification

---

## 10. Context Management

### 10.1 Context Compactor

When conversation exceeds token limit, compact context by summarizing older messages.

```rust
// crates/core/src/context/compactor.rs

/// Compacts conversation history to fit within token limit
pub struct ContextCompactor {
    model: Box<dyn TokenCounter>,
    max_tokens: usize,
    preserve_recent: usize, // Number of recent messages to preserve verbatim
}

impl ContextCompactor {
    pub fn compact(&self, messages: &[Message]) -> Vec<Message> {
        let total_tokens = self.model.count_tokens(messages);

        if total_tokens <= self.max_tokens {
            return messages.to_vec();
        }

        // Split into: [older messages to summarize] + [recent messages to preserve]
        let split_point = messages.len().saturating_sub(self.preserve_recent);
        let (older, recent) = messages.split_at(split_point);

        // Summarize older messages
        let summary = self.summarize(older);

        // Combine: [system] + [summary] + [recent messages]
        let mut compacted = vec![Message::system(summary)];
        compacted.extend_from_slice(recent);

        compacted
    }

    fn summarize(&self, messages: &[Message]) -> String {
        // Strategy 1: Use LLM to summarize (async, requires adapter call)
        // Strategy 2: Simple truncation with "..." indicator
        // Strategy 3: Extract key decisions/actions only

        // For MVP: simple summary of message types and count
        format!(
            "[{} previous messages summarized: {}]",
            messages.len(),
            self.extract_key_points(messages)
        )
    }
}
```

---

## 11. Rate Limiting & Retries

### 11.1 Provider Rate Limit Handler

Automatic retry with exponential backoff for 429 (rate limit) errors.

```rust
// crates/core/src/provider/retry.rs

use std::time::Duration;

/// Retry configuration for rate-limited requests
pub struct RetryConfig {
    pub max_retries: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
    pub exponential_base: f64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(60),
            exponential_base: 2.0,
        }
    }
}

/// Wrapper around provider that adds retry logic
pub struct RetryingProvider {
    inner: Arc<dyn ModelProvider>,
    config: RetryConfig,
}

#[async_trait]
impl ModelProvider for RetryingProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ModelResponse, ProviderError> {
        let mut attempt = 0;
        let mut delay = self.config.base_delay;

        loop {
            match self.inner.complete(messages, tools).await {
                Ok(response) => return Ok(response),
                Err(ProviderError::RateLimit { retry_after }) => {
                    attempt += 1;
                    if attempt > self.config.max_retries {
                        return Err(ProviderError::RateLimit { retry_after });
                    }

                    // Use provider's suggested retry time or calculate exponential backoff
                    let wait_duration = retry_after
                        .unwrap_or_else(|| {
                            let backoff = delay.mul_f64(self.config.exponential_base.powi(attempt as i32));
                            backoff.min(self.config.max_delay)
                        });

                    tokio::time::sleep(wait_duration).await;
                }
                Err(other) => return Err(other),
            }
        }
    }

    async fn stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<BoxStream<'static, Result<StreamChunk, ProviderError>>, ProviderError> {
        // Similar retry logic for streaming
        // ...
    }
}
```

---

## 12. Future Considerations

### Server Mode

The hybrid event bus architecture supports future server mode:

```rust
// Future: crates/server/src/lib.rs

pub struct Server {
    bus: GlobalBus,
    sessions: SessionManager,
    clients: ClientManager,
}

impl Server {
    pub async fn run(&self) -> Result<()> {
        // HTTP/WebSocket server
        // Multiple TUI clients can connect
        // Sessions persist on server
    }
}
```

### Plugin System

Tool registry supports dynamic loading:

```rust
pub trait Plugin: Send + Sync {
    fn name(&self) -> &str;
    fn register_tools(&self, registry: &mut ToolRegistry);
}
```

---

## 11. Dependencies Summary

| Category | Primary Crate | Version |
|----------|--------------|---------|
| Async Runtime | tokio | 1.x |
| HTTP Client | reqwest | 0.12 |
| Serialization | serde | 1.x |
| Markdown | pulldown-cmark | 0.12 |
| Database | sqlx | 0.8 |
| TUI | ratatui | 0.29 |
| Terminal | crossterm | 0.28 |
| CLI | clap | 4.x |
| Config | config | 0.15 |
| Error Handling | thiserror | 2.x |
| Logging | tracing | 0.1 |

---

## 12. Design Improvements (Post-Review)

Based on ChatGPT's detailed review, the following improvements were incorporated:

### 12.1 Event Bus - Modular Events
- **Before**: Single monolithic `Event` enum with 15+ variants
- **After**: Modular design with `Event { User(UserEvent), Agent(AgentEvent), ... }`
- **Benefit**: Prevents enum explosion as system grows, enables plugin-defined events

### 12.2 Agent State Machine - Terminal States
- **Before**: `Error { reason }` state, unclear lifecycle
- **After**: Explicit terminal states: `Completed`, `Failed`, `Cancelled`
- **Benefit**: Clear lifecycle boundaries, easier sub-agent orchestration

### 12.3 Sub-Agent - Copy-on-Write Context
- **Before**: `Task` mode shared parent context directly
- **After**: `CowContext` with read-only parent + local diff
- **Benefit**: Prevents race conditions, prompt pollution, enables controlled merge

### 12.4 Storage - Explicit Content Types
- **Before**: `content_ref TEXT` ambiguous
- **After**: `content_type` enum with checksums
- **Benefit**: Future-proof for dedup, integrity checks, chunked content

### 12.5 TUI - Event-Driven Rendering
- **Before**: 60fps polling (`sleep(16ms)`)
- **After**: Event-driven with `dirty` flag
- **Benefit**: Lower CPU usage, better SSH experience

### 12.6 Markdown Renderer - Streaming Support
- **Before**: `render(&str)` for complete content
- **After**: `push_chunk()` for incremental parsing
- **Benefit**: Real-time AI output without flickering

### 12.7 Session - Branching/Forking
- **Before**: Linear session history
- **After**: `parent_session_id` for tree structure
- **Benefit**: Claude Code-style session branching

### 12.8 Agent State - Single-Threaded Loop (Second Review)
- **Before**: `state: Arc<RwLock<AgentState>>` - risk of deadlocks, long-held locks
- **After**: `Agent::spawn()` creates single-task event loop, `AgentHandle` for external communication
- **Benefit**: No locks needed, clear ownership, no deadlock risk

### 12.9 Event Bus - Bounded Channels with Backpressure (Second Review)
- **Before**: `mpsc::UnboundedSender` - risk of memory explosion if TUI can't keep up
- **After**: Bounded channels (`mpsc::channel(capacity)`) with backpressure: coalesce `ModelChunk` events or block
- **Benefit**: Memory bounded even under heavy load

### 12.10 Markdown Renderer - Use pulldown_cmark (Second Review)
- **Before**: Custom `ParseState` enum with `InCodeBlock`, `InThinkBlock` states
- **After**: Buffer + `pulldown_cmark::Parser` for each chunk, find stable point, re-render from there
- **Benefit**: No custom parser complexity, handles broken/incomplete markdown correctly

### 12.11 Tool System - Sandbox Context (Second Review)
- **Before**: `execute(&self, params, ctx)` without isolation
- **After**: `ToolSandbox` with `cwd`, `env`, `permissions`, path validation (MVP: soft sandbox)
- **Note**: Shell commands can still bypass (e.g., `cat ../../etc/passwd`). Hard sandbox (chroot/namespaces) for production
- **Benefit**: Basic isolation, foundation for hardening

### 12.12 Agent Loop - Batching + Prompt Building (Third Review)
- **Before**: `tokio::select!` with individual chunk handling - risk of starvation
- **After**:
  - `process_model_chunks_batch(10)` - batch up to N chunks before yielding
  - `build_prompt()` - construct full prompt with system + AGENTS.md + skills + user message
- **Benefit**: Prevents UI lag, tool delays; structured prompt construction

### 12.13 Skills System (Third Review)
- **New**: Load `.md` files from `.nekoclaw/skills/` directory
- **Usage**: Include skill content in prompt building
- **Benefit**: Modular, reusable instructions per project

### 12.14 Sandbox Toggle (Fourth Review)
- **Before**: Sandbox always enabled
- **After**: `ToolSandbox::enabled` flag, `ToolSandbox::disabled()` constructor, `--no-sandbox` CLI flag
- **Benefit**: Users can disable sandbox for trusted environments (with clear security warnings)

### 12.15 YOLO Mode (Fourth Review)
- **Before**: All `Confirm` level tools require user approval
- **After**: Global `yolo` flag in `AgentConfig` and `ToolRegistry`, `--yolo` CLI flag
- **Benefit**: Power users can skip confirmations for batch operations (use with caution!)

### 12.16 PromptBuilder Separation (Fifth Review)
- **Before**: Agent had `build_prompt()`, `load_agents_md()`, `load_skills()` methods
- **After**: Separate `PromptBuilder` component with configurable paths, Agent receives pre-built `system_context`
- **Benefit**: Clean separation of concerns, Agent is pure execution engine, paths are configurable

### 12.17 Configurable Skill Paths (Fifth Review)
- **Before**: Hardcoded `.nekoclaw/skills/` path
- **After**: `PromptBuilderConfig.skill_paths: Vec<PathBuf>` with fallback to default
- **Benefit**: Users can organize skills in custom locations

### 12.18 Sandbox Default Disabled (Fifth Review)
- **Before**: `enabled: true` by default
- **After**: `enabled: false` by default, opt-in via `--sandbox` or config
- **Benefit**: MVP simplicity, users opt-in to sandbox when needed

### 12.19 UNIX Philosophy - Core/Adapters Split (Sixth Review)
- **Before**: Core contained storage, provider, tool implementations (mixed concerns)
- **After**:
  - Core defines traits (`ModelProvider`, `Storage`, `Tool`) + async agent runtime
  - Adapters crate implements all I/O (OpenAI client, SQLite, bash tool)
- **Benefit**: Core is pure interface + async orchestration, easily testable, adapters depend on core (not reverse)

### 12.20 Event Bus - Broadcast for Multi-Consumer (Seventh Review)
- **Before**: `mpsc::channel` - only one receiver, TUI/Logger/Recorder can't all subscribe
- **After**: `tokio::sync::broadcast` - true fan-out, multiple subscribers receive same event
- **Benefit**: TUI, Logger, Recorder, Debugger can all subscribe independently

### 12.21 Model Streaming - Yield to Prevent Starvation (Seventh Review)
- **Before**: `process_model_chunks_batch(10)` but still possible to starve other branches
- **After**: Added `tokio::task::yield_now().await` after each batch to force scheduler switch
- **Benefit**: Tool completion, user commands, timeouts get CPU time even during heavy streaming

### 12.22 Context Compactor (Seventh Review)
- **New**: `ContextCompactor` component that summarizes older messages when token limit exceeded
- **Strategies**: Preserve recent N messages, summarize older ones with LLM or simple truncation
- **Benefit**: Long conversations don't crash or get cut off

### 12.23 Rate Limit Retry with Exponential Backoff (Seventh Review)
- **New**: `RetryingProvider` wrapper that catches 429 errors and retries with exponential backoff
- **Config**: `max_retries`, `base_delay`, `max_delay`, `exponential_base`
- **Benefit**: Automatic recovery from rate limits without user intervention

---

## 13. Success Criteria

1. ✅ Can start a session and have a conversation with LLM
2. ✅ Tool calls work with configurable permissions
3. ✅ Sub-agents can be spawned and complete tasks
4. ✅ Model fallback works when primary fails
5. ✅ SSE idle timeout is enforced
6. ✅ TUI renders markdown with custom styles
7. ✅ Dark theme is pleasant and not too contrasty
8. ✅ Sessions persist across restarts
9. ✅ Configuration is flexible (endpoint, api key, model)
10. ✅ Code is well-structured and tested
11. ✅ Sub-agent context isolation prevents pollution
12. ✅ Markdown streams without flickering
13. ✅ Sessions can be forked/branched
14. ✅ Agent state machine has no locks (single-threaded loop)
15. ✅ Event bus has backpressure (bounded channels)
16. ✅ Tools run in soft sandbox (path restrictions)
17. ✅ Model chunks are batched (no starvation)
18. ✅ Prompt building includes AGENTS.md and skills
19. ✅ Sandbox can be disabled (--no-sandbox)
20. ✅ YOLO mode skips confirmations (--yolo)
21. ✅ PromptBuilder is separate from Agent
22. ✅ Skill paths are configurable
23. ✅ Sandbox disabled by default
24. ✅ Core defines traits (interfaces), adapters implement them
25. ✅ Core has async agent but NO concrete I/O (HTTP, DB, filesystem)
26. ✅ Each crate has single responsibility (UNIX philosophy)
27. ✅ Dependencies flow inward: adapters → core (not reverse)
28. ✅ Core is testable without real HTTP/DB (mock implementations)
29. ✅ Event bus supports multiple subscribers (broadcast, not mpsc)
30. ✅ Model streaming yields to prevent starving other branches
31. ✅ Context compactor handles long conversations
32. ✅ Rate limit retry with exponential backoff
