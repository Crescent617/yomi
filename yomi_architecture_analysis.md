Systematic reading and analysis of 25+ source files across the Yomi kernel is complete. The findings are synthesized into a structured architectural report below.

# Yomi Kernel Architecture: Comprehensive Analysis

## 1. Dependency Graph Between Core Components

The architecture is layered as **Server → Coordinator → Session → Agent → Tools**, with a cross-cutting **EventBus** and **Storage** layer.

### 1.1. The Agent Core (Agent, SimpleAgent, and their Context)

*   **`Agent`**: The heavyweight, stateful main agent. It owns a `MessageBuffer`, a `ToolRegistry`, an `AgentExecutionContext` (state machine), and runs an async `start_loop()` inside a spawned Tokio task. It is constructed from `AgentSpawnArgs` and `AgentShared`.
*   **`SimpleAgent`**: A lightweight, single-request-response agent used exclusively by `SubagentTool`. It has no persistence, no complex state machine, and uses a native `tokio_util::sync::CancellationToken`. It reuses the same `ToolRegistry` and `ToolExecCtx` patterns but does not interact with `AgentShared` stores directly.
*   **`AgentExecutionContext`**: A shared-state wrapper around a `tokio::sync::watch::Sender<AgentState>` and an `AtomicUsize` iteration counter. It enforces valid state transitions (Idle → Streaming → ExecutingTool → Idle, etc.).
*   **`Turn`**: Represents a single user-request → model-response → tool-execution cycle. It is created when the agent transitions from Idle to Streaming and tracks file modifications for checkpointing. `Agent` holds `current_turn: Option<Arc<Turn>>` and passes it to tools via `ToolExecCtx`.

### 1.2. The Handle and Spawn Arguments

*   **`AgentHandle`**: The external control surface for a running `Agent`. It is returned by `Agent::spawn()` and contains:
    *   `input_tx: mpsc::Sender<AgentInput>` — to send `User`, `Shutdown`, `Compact`, `Rewind`, `Clear`, `Continue`, and `TaskResult` inputs.
    *   `state_rx: watch::Receiver<AgentState>` — to observe state changes.
    *   `cancel_token: CancelToken` — to trigger cancellation (which increments `input_stale_since`).
    *   `permission_responder` / `ask_user_responder` — to respond to permission/ask_user requests without using the main input channel.
    *   `steer_tx: mpsc::Sender<Vec<ContentBlock>>` — to inject steer messages before the next streaming turn.
*   **`AgentSpawnArgs`**: A builder-style configuration struct passed to `Agent::spawn()`. It contains `base_prompt`, `skills`, `history`, `session_id`, `max_iterations`, `tool_blocklist`, `file_state_store`, `cancel_token`, and `working_dir`. It is consumed at spawn time to build the `Agent`.
*   **`AgentShared`**: A cloneable struct containing **all session-scoped shared resources** that outlive any individual agent. It is passed as an `Arc<AgentShared>` to `Agent::spawn()` and to `SubagentTool`. Key fields:
    *   `provider`, `model_config` — LLM backend.
    *   `session_store`, `message_store`, `usage_store`, `todo_storage` — storage backends.
    *   `permission_state` — shared permission checker state.
    *   `file_state_store`, `checkpoint_store` — file tracking and rewind state.
    *   `compactor` — context compaction logic.
    *   `hook_registry`, `goal_store`, `channel_hub`, `event_bus` — optional extensions.
    *   `data_dir`, `skill_folders` — filesystem paths.

### 1.3. Tools and Execution Context

*   **`Tool` trait**: Core abstraction. `exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput>`.
*   **`ToolExecCtx`**: Passed to every tool execution. Contains `tool_call_id`, `parent_messages`, `cancel_token`, `working_dir`, `session_id`, `message_id`, `turn` (for checkpointing), `skills`, and `max_tool_output_length`.
*   **`ToolRegistry`**: A `HashMap<String, Arc<dyn Tool>>` with lazy-cached `Vec<Arc<ToolDefinition>>`. Each `Agent` and `SimpleAgent` owns its own registry (subagents get a fresh one without subagent tools by default).
*   **`ToolRegistryFactory` / `ToolRegistryConfig`**: Decouples registry creation from `Agent`. Used by both `Agent::spawn()` and `SubagentTool::create_tool_registry()` to register standard tools (shell, read, edit, write, glob, grep, webfetch, websearch, todo, sleep, ask_user, subagent, update_goal, etc.).
*   **`execute_tools_parallel`**: A standalone function in `tools::executor` that takes `ToolExecParams` (including `tool_registry`, `tool_calls`, `turn`, `skills`, etc.) and runs each tool call in a `tokio::task::JoinSet`, supporting cancellation via `CancellationToken`.

### 1.4. Event Bus and Hooks

*   **`EventBus`**: Global singleton per kernel process. Created by `Coordinator` and stored in `AgentShared`. It has an internal `run_forwarder` task that routes `(SessionId, Event)` tuples to session-specific or global listeners.
*   **`EventBusHandle`**: A lightweight cloneable producer handle bound to a specific `SessionId`. Methods: `send()`, `try_send()`.
*   **`EventBusSubscriber`**: A consumer guard that auto-unsubscribes on `Drop`. Returns `(SessionId, Event)` via `recv()`.
*   **`HookRegistry`**: Owned by `Agent`, built at spawn time from `AgentShared.hook_registry` (user config) plus skill-level hooks. Groups `Arc<dyn HookHandler>` by `HookEvent`.
*   **`agent/hooks.rs`**: Bridge between `HookRegistry` and `Agent`. `run_pre_tool_hooks()` filters/modifies `ToolCall`s before execution. `run_post_tool_hooks()` transforms `ToolExecutionResult`s and can inject context messages or stop the session.

### 1.5. Storage Layer

All storage is trait-based with concrete implementations:
*   **`MessageStore`** (`jsonl::JsonlMessageStore`): Append-only JSON Lines file per session (`{base_dir}/{session_id}.jsonl`). Supports `append`, `get`, `replace` (atomic rename). Inline images are extracted to the data directory before writing.
*   **`SessionStore`** (`sqlite::SqliteSessionStore`): SQLite-backed metadata (title, created_at, working_dir, project_id, auto_approve_level, message_count). Supports `create`, `fork`, `get`, `list`, `update_title`, `update_auto_approve_level`, `cleanup`.
*   **`UsageStore`** (`sqlite::SqliteUsageStore`): SQLite table `token_usage` with per-request records (prompt/completion/cached tokens, model, provider, usage_type). Supports `record`, `summarize`, `daily_summary`.
*   **`TodoStore`** (`json::JsonTodoStore`): Simple JSON file per session (`sessions/todos/{session_id}.json`). `save`, `load`, `clear`.
*   **`FileStateStore`** (`jsonl::JsonlFileStateStore`): Tracks `(path, mtime)` for read-before-write validation. Auto-vacuum deduplicates by path (last wins).
*   **`CheckpointStore`**: Referenced but not fully read; used by `Turn` for file backups and rewind.

### 1.6. High-Level Dependency Flow

```
KernelServer → Coordinator (owns Arc<AgentShared> + sessions DashMap)
     ↓
Coordinator::create_session() / restore_session()
     ↓
Session::init() → Session (owns AgentHandle + PermissionState + event_bus handle)
     ↓
Agent::spawn(id, &AgentShared, AgentSpawnArgs) → AgentHandle
     ↓
Agent (inside Tokio task) owns:
   - MessageBuffer
   - ToolRegistry (from ToolRegistryFactory)
   - HookRegistry (from AgentShared + skills)
   - EventBusHandle (from AgentShared.event_bus)
   - PermissionChecker (from AgentShared.permission_state)
   - current_turn: Option<Arc<Turn>>
   - input_rx / steer_rx
     ↓
ToolExecCtx (passed to tools) references parent_messages, turn, skills, etc.
     ↓
SubagentTool → creates SimpleAgent + new ToolRegistry (for_subagent) + session_store.create()
```

## 2. Event Flow (Which Struct Sends What Event to Whom)

Events are emitted as `Event` enum variants and sent via `EventBusHandle::try_send()`. The `EventBus` forwarder then dispatches to all subscribers of that `SessionId` plus all global subscribers.

### 2.1. Agent → EventBus

The `Agent` struct emits the majority of events during its lifecycle:

| Sender | Event Variant | Trigger | Recipients |
|--------|--------------|---------|------------|
| `Agent` | `Event::Agent(Lifecycle { Running })` | Entering `Streaming` state | TUI (session subscribers) |
| `Agent` | `Event::Agent(Lifecycle { Stopped { reason } })` | Max iterations reached, cancelled, failed, or completed | TUI |
| `Agent` | `Event::Agent(Retrying { ... })` | Streaming retry | TUI |
| `Agent` | `Event::Agent(Error { phase, error, is_recoverable })` | Any error in Idle/Streaming/ToolExecution/Compaction | TUI |
| `Agent` | `Event::Agent(PermissionRequest { ... })` | Permission check requires user approval | TUI |
| `Agent` | `Event::Agent(AskUserQuestion { ... })` | `ask_user` tool invoked | TUI |
| `Agent` | `Event::Model(Request { ... })` | Before model streaming starts | TUI |
| `Agent` | `Event::Model(Chunk { ... })` | Each streaming chunk from provider | TUI |
| `Agent` | `Event::Model(ToolCallDelta { ... })` | Incremental tool call args | TUI |
| `Agent` | `Event::Model(Completed { ... })` | Model stream finished | TUI |
| `Agent` | `Event::Model(End { ... })` | Final assembled message content | TUI |
| `Agent` | `Event::Model(TokenUsage { ... })` | Token usage received from provider | TUI |
| `Agent` | `Event::Model(Compacting { active })` | Start/end of compaction | TUI |
| `Agent` | `Event::Model(Fallback { ... })` | Provider fallback triggered | TUI |
| `Agent` | `Event::Tool(Start { ... })` | Before each tool call execution | TUI |
| `Agent` | `Event::Tool(End { ... })` | After each tool call execution | TUI |
| `Agent` | `Event::User(Message { ... })` | User message injected (including steer) | TUI |
| `Agent` | `Event::System(Shutdown { ... })` | Agent loop ends (explicit shutdown or error) | TUI |
| `Agent` | `Event::System(Rewound { ... })` | After rewind completes | TUI |

### 2.2. SubagentTool → EventBus

`SubagentTool` (a `Tool` implementation) emits `Progress` events during `SimpleAgent` execution. It forwards `ModelEvent::TokenUsage` and `ModelEvent::Chunk` from the `SimpleAgent` callback as `ToolEvent::Progress` messages to the parent's event bus. It does **not** send `ToolEvent::Start`/`End` for subagent calls (those are managed by the parent `Agent` as part of the subagent tool call itself).

### 2.3. Session → EventBus

`Session` emits `SystemEvent::TitleUpdated` when the user sends a message and the title is inferred from the first text block. It also forwards permission responses via `AgentHandle`.

### 2.4. Coordinator → EventBus

`Coordinator` emits `SystemEvent::GoalUpdated` and `SystemEvent::GoalStopped` when goal state changes. It also triggers `EventBus` subscriptions when TUI clients request `Subscribe`.

### 2.5. Server / TUI Bridge

`KernelServer::handle_connection()` spawns a task that reads from `EventBusSubscriber::recv()` and forwards events as `WireMsg::Event` over the transport (IPC/TCP) to the TUI. Each client connection holds its own `EventBusSubscriber`.

### 2.6. Key Observations

*   **All events are session-scoped**: The `EventBusHandle` is bound to `SessionId`, so events from different sessions never mix.
*   **No backpressure guarantee**: `try_send()` is used almost everywhere; if a subscriber is slow, events may be dropped silently (`TrySendError::Full` is ignored in `try_send_to_listeners`).
*   **Agent lifecycle is observable entirely through events**: A TUI can reconstruct the agent state from `Lifecycle`, `Model`, `Tool`, and `Error` events without querying internal state.

## 3. How Messages Are Stored and Recovered

### 3.1. In-Memory Buffer: `MessageBuffer`

`Agent` holds a `MessageBuffer` (a `Vec<Arc<Message>>` wrapper). All messages in the current conversation live here. New messages are pushed as `Arc<Message>` to minimize cloning.

### 3.2. Persistence: `MessageStore` (JsonlMessageStore)

*   **Append**: Every time `Agent` pushes a message to its buffer, it also calls `persist_message(&self, message)` which calls `MessageStore::append(session_id, &[message])`. This is append-only I/O, so it's fast and safe.
*   **Replace**: During compaction or rewind, `MessageStore::replace(session_id, &messages)` atomically rewrites the JSONL file using a temp file + rename.
*   **Format**: Each line is a JSON-serialized `Message`. Inline images are extracted to `asset://` files in the data directory before serialization to keep line sizes manageable.
*   **Recovery**: When `Session::spawn_main_agent()` is called, it reads historical messages via `agent_shared.message_store.get(&session_id)`. These are loaded into `AgentSpawnArgs.history`, which the `Agent` uses to initialize its `MessageBuffer` (after prepending the system prompt).

### 3.3. Session-Level Recovery Flow

1.  **Coordinator::restore_session()**: Looks up `SessionInfo` from `SessionStore`. If the session is not live in memory, it creates a `SessionConfig` from stored metadata and calls `Session::init()`.
2.  **Session::init()**: Creates a `FileStateStore` from disk (`JsonlFileStateStore`), loads workspace skills, and calls `spawn_main_agent()`.
3.  **spawn_main_agent()**: Loads message history from `MessageStore`, merges skills, creates `AgentSpawnArgs`, clones `AgentShared`, and calls `Agent::spawn()`.
4.  **Agent::spawn()**: Builds `MessageBuffer::from_arc_messages(&messages)` where `messages` starts with the system prompt and appends the recovered history (excluding duplicate system messages).

### 3.4. Forking

`Coordinator::fork_session()` copies:
*   `SessionStore` metadata (via `fork()` SQL query).
*   `MessageStore` history (via `replace()` with parent messages).
*   `GoalStore` state.
*   `TodoStore` JSON.
*   `FileStateStore` JSONL file (via `fs::copy`).
*   `CheckpointStore` checkpoints.

Then it initializes a new `Session` with the copied data.

### 3.5. Clear / Reset

`AgentInput::Clear` triggers `Agent::handle_clear()`, which:
1.  Truncates the `MessageBuffer` to just the system prompt.
2.  Clears `FileStateStore`.
3.  Clears `TodoStore`.
4.  Calls `MessageStore::replace()` with the truncated messages.

## 4. Exact Relationship Between Agent, AgentHandle, AgentSpawnArgs, and AgentShared

### 4.1. AgentShared: The "Resource Context"

`AgentShared` is **not** a trait; it is a concrete, cloneable struct that acts as a **resource bundle** for everything an agent needs that is not agent-specific. It is created once per `Coordinator` (or derived per session via `with_per_session()`), wrapped in `Arc`, and shared across all agents in a session (main agent + subagents).

Key relationships:
*   **`Agent::spawn()`** takes `&Arc<AgentShared>` and clones it.
*   **`SubagentTool`** takes `Arc<AgentShared>` and passes it to `SimpleAgent::new()`.
*   **`ToolRegistryFactory::create()`** takes `&Arc<AgentShared>` to inject stores into tools.
*   **`Checker::new()`** (permission) takes a clone of `AgentShared.permission_state`.

### 4.2. AgentSpawnArgs: The "Configuration Blueprint"

`AgentSpawnArgs` is a **builder-style value struct** consumed exactly once by `Agent::spawn()`. It contains the *initial conditions* for the agent:
*   `base_prompt` → becomes the system message.
*   `history` → recovered messages from `MessageStore`.
*   `skills` → available to tools and the system prompt builder.
*   `session_id`, `working_dir`, `max_iterations`, `tool_blocklist`, `allow_command_hooks`, `max_tool_output_length`.
*   `cancel_token` → optional shared cancellation with parent.
*   `file_state_store` → optional inherited file state.

It is **not** stored or referenced after spawn; its data is moved into the `Agent` struct fields.

### 4.3. Agent: The "Runtime Engine"

`Agent` is the **owner of the runtime state**. It is never exposed directly to callers; it runs inside a `tokio::spawn` task. Its lifetime is tied to the task. Key owned fields:
*   `id: AgentId` — unique per agent instance.
*   `shared: Arc<AgentShared>` — shared resources.
*   `message_buffer: MessageBuffer` — in-memory conversation history.
*   `input_rx: mpsc::Receiver<AgentInput>` — receives commands from `AgentHandle`.
*   `steer_rx: mpsc::Receiver<Vec<ContentBlock>>` — receives steer messages.
*   `tool_registry: ToolRegistry` — owned per agent (main vs subagent registries differ).
*   `hook_registry: HookRegistry` — owned per agent (built from `AgentShared` + skills).
*   `current_turn: Option<Arc<Turn>>` — checkpoint/file tracking for the current cycle.
*   `context: AgentExecutionContext` — state machine + iteration counter.
*   `cancel_token: CancelToken` — custom cancel token with reset support.

### 4.4. AgentHandle: The "Remote Control"

`AgentHandle` is the **only external interface** to a running `Agent`. It is returned by `Agent::spawn()` and held by `Session`. It is `Clone` and `Send`.

Its methods map directly to `AgentInput` variants sent over `input_tx`:
*   `send_message()` → `AgentInput::User`
*   `cancel()` → increments `input_stale_since` + triggers `CancelToken`
*   `close()` → `AgentInput::Shutdown`
*   `force_compact()` → `AgentInput::Compact`
*   `rewind()` → `AgentInput::Rewind`
*   `clear()` → `AgentInput::Clear`
*   `send_continue()` → `AgentInput::Continue`
*   `send_steer()` → `steer_tx.try_send()` (bypasses `input_tx`)

It also holds `permission_responder` and `ask_user_responder` for out-of-band responses.

### 4.5. Interaction Diagram

```
Coordinator (singleton)
   └─ Arc<AgentShared> ── shared across all sessions
        ├─ provider, model_config
        ├─ session_store, message_store, usage_store, todo_storage
        ├─ event_bus: Arc<EventBus>
        └─ ...

Session (per active session)
   └─ main_agent: AgentHandle
        ├─ input_tx  ────────┐
        ├─ steer_tx  ────────┤
        └─ state_rx  <───────┘

Agent (spawned task, owns runtime)
   ├─ input_rx  <── from AgentHandle.input_tx
   ├─ steer_rx  <── from AgentHandle.steer_tx
   ├─ shared: Arc<AgentShared>  (cloned from Coordinator)
   ├─ spawn_args consumed at creation:
   │   ├─ base_prompt → system message
   │   ├─ history → MessageBuffer initial state
   │   ├─ skills → passed to SystemPromptBuilder & ToolRegistry
   │   └─ working_dir, max_iterations, etc.
   └─ tool_registry, hook_registry built at spawn time
```

### 4.6. Key Design Decisions

*   **Separation of concerns**: `AgentShared` holds *persistent/reusable* resources; `AgentSpawnArgs` holds *initial configuration*; `Agent` holds *mutable runtime state*; `AgentHandle` provides *external control*.
*   **No direct parent-child agent reference**: `AgentHandle` does not know about `Agent`. They communicate only via MPSC channels. This makes the agent task independently cancellable and replaceable.
*   **Subagent reuses `AgentShared` but not `AgentSpawnArgs`**: `SubagentTool` constructs its own `SimpleAgent` with a fresh `AgentSpawnArgs`-like configuration derived from the parent context, but it shares the same `AgentShared` (provider, model config, event bus, etc.).
*   **Cancellation is generational**: `AgentHandle.cancel()` increments `input_stale_since`. Any `AgentInput::User` sent before the increment is discarded by the `Agent` task. This prevents race conditions where a user sends a message and immediately cancels.
