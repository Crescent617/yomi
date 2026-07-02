# Multi-Model Configuration Design

> **Breaking change.** This design is intentionally **not backward-compatible** with the old single-model `[agent.model]` config format. The old `Config.agent.model` field is removed entirely.

## 1. Background & Motivation

Currently `yomi` only supports a **single model** at runtime. The `Config` struct holds one `ModelConfig` inside `AgentConfig`, and `Coordinator` creates all sessions with the same fixed `Provider` + `ModelConfig` pair baked into `AgentShared`.

Users want to:
1. **Define multiple models** in the config file (e.g. Claude for deep reasoning, GPT-4o for fast chat, local model for privacy).
2. **Switch models at runtime** — either when creating a new session or mid-session while the agent is idle.
3. **Each model has its own context window** — because different models have different token limits (e.g. Claude 3.5 Sonnet 200k, GPT-4o 128k, local Llama 8k). The compactor's behavior should adapt to the currently active model.

## 2. Design Goals

| Goal | Description |
|------|-------------|
| **Multi-profile config** | A config file can declare N named model profiles. |
| **Per-model context window** | Each profile carries its own `context_window`. The compactor reads it from the active model at decision time. |
| **Runtime switch** | Switch the active model for a session without restart. |
| **Per-session isolation** | Each session can use a different model. Changing one session does not affect others. |
| **Lazy provider init** | Provider instances (HTTP clients) are created once per `ModelProvider` enum variant and cached. |
| **Clean break from old config** | Remove `agent.model` from `Config` and `AgentConfig`. All model config lives in the new `models` table. |
| **No breaking changes to Message/Agent loop** | The agent streaming loop stays the same; it just reads `provider` and `model_config` from a different source. |

## 3. Core Concepts

### 3.1 ModelProfile

A **profile** is a *named* model configuration. It bundles everything needed to talk to one model endpoint, including its **context window size**.

```rust
struct ModelProfile {
    name: String,               // e.g. "claude-sonnet", "gpt-4o", "local-llama"
    provider: ModelProvider,    // OpenAI, Anthropic, ...
    config: ModelConfig,        // model_id, endpoint, api_key, temperature, ...
    context_window: u32,        // e.g. 200_000 for Claude, 128_000 for GPT-4o
}
```

> **Why `context_window` on the profile?**
> Context window is a *model property*, not a behavior strategy. A 128k model and an 8k local model cannot share the same compactor threshold. By moving `context_window` from `Compactor` to `ModelProfile`, the compaction threshold is computed dynamically based on the active model at each turn.

> **Why a separate name?**
> The same `ModelProvider` variant (e.g. `OpenAI`) can be used with different endpoints (OpenAI official, Azure, local vLLM, Gemini via OpenAI-compatible API). The profile name is the user-facing handle.

### 3.2 ModelRegistry

A **global, shared registry** that holds all profiles and resolves a profile name to the concrete `(Provider, ModelConfig)` pair needed by the agent.

```rust
struct ModelRegistry {
    profiles: HashMap<String, ModelProfile>,
    default: String,                        // default profile name
    providers: HashMap<ModelProvider, Arc<dyn Provider>>,
}

impl ModelRegistry {
    fn resolve(&self, name: &str) -> Option<(Arc<dyn Provider>, Arc<ModelConfig>)>;
    fn resolve_with_context(&self, name: &str) -> Option<ResolvedModel>;
    fn list_profiles(&self) -> Vec<ProfileInfo>;  // name + provider + model_id + context_window
    fn default_profile(&self) -> &str;
}

struct ResolvedModel {
    provider: Arc<dyn Provider>,
    model_config: Arc<ModelConfig>,
    context_window: u32,
}
```

Provider instances are **cached by `ModelProvider` variant** because:
- `AnthropicProvider` and `OpenAIProvider` are protocol implementations, not bound to a specific endpoint.
- They can be reused across profiles that share the same provider enum variant (e.g. two OpenAI-compatible endpoints).
- This avoids creating redundant HTTP clients (`reqwest::Client`).

### 3.3 ProfileSelector (per-session)

`AgentShared` currently stores `provider: Arc<dyn Provider>` and `model_config: Arc<ModelConfig>`. These will be replaced by:

```rust
struct AgentShared {
    // --- NEW ---
    model_registry: Arc<ModelRegistry>,
    current_profile: String,   // active profile name for this session

    // --- REMOVED (old fields) ---
    // provider: Arc<dyn Provider>,
    // model_config: Arc<ModelConfig>,
}
```

`AgentShared` is `Clone` (each session gets its own copy). Changing `current_profile` on one session's `AgentShared` does **not** affect other sessions.

Whenever the agent loop needs to stream, it reads the current profile from `AgentShared` and resolves it via the registry:

```rust
let resolved = shared.model_registry.resolve_with_context(&shared.current_profile)?;
let provider = resolved.provider;
let model_config = resolved.model_config;
let context_window = resolved.context_window;  // used by compactor
```

Because the agent clones `provider` and `model_config` at the start of each iteration, a switch only takes effect on the **next** turn. This is safe because the switch is only allowed when the agent is in `Idle` state.

## 4. Configuration Format

### 4.1 TOML format (multi-profile, required)

The `models` table is **mandatory**. There is no `agent.model` fallback.

```toml
# ~/.yomi/config.toml

[models.default]
provider = "anthropic"
model_id = "claude-3-5-sonnet-20241022"
endpoint = "https://api.anthropic.com"
api_key = "sk-ant-..."
max_tokens = 8192
context_window = 200_000

[models.fast]
provider = "openai"
model_id = "gpt-4o"
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
temperature = 0.5
context_window = 128_000

[models.local]
provider = "openai"
model_id = "llama-3.1-70b"
endpoint = "http://localhost:8000/v1"
api_key = "not-needed"
context_window = 8_000

[agent]
max_iterations = 100
system_prompt = "..."

[agent.compactor]
threshold_ratio = 0.8
keep_recent = 6
summary_max_tokens = 8192
# ❌ context_window is NO LONGER here
```

### 4.2 Removed fields

These fields from the old `Config` / `Compactor` are **deleted**:

```toml
# ❌ GONE — no longer supported
[agent.model]
provider = "..."
model_id = "..."

# ❌ GONE — moved to ModelProfile
[agent.compactor]
context_window = 131072
```

`AgentConfig` no longer has a `model: ModelConfig` field. `Compactor` no longer has a `context_window` field. All model-specific data lives in `ModelProfile` under the top-level `models` table.

## 5. Architecture Changes

### 5.1 Compactor Changes

`Compactor` is now a **pure behavior strategy** without model-specific data:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Compactor {
    pub threshold_ratio: f32,
    // ❌ REMOVED: pub context_window: u32,
    pub keep_recent: usize,
    pub summary_max_tokens: u32,
}
```

`threshold()` and `should_compact()` now take `context_window` as a parameter:

```rust
impl Compactor {
    pub fn threshold(&self, context_window: u32) -> u32 {
        (context_window as f32 * self.threshold_ratio) as u32
    }

    pub fn should_compact(&self, messages: &[Arc<Message>], context_window: u32) -> bool {
        let threshold = self.threshold(context_window);
        let tokens = Self::calculate_tokens(messages);
        tokens >= threshold
    }
}
```

The agent loop resolves `context_window` from the active model profile before calling `should_compact()` or `compact()`.

### 5.2 Kernel (`crates/kernel`)

#### Config (`config.rs`)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    // --- REMOVED ---
    // pub agent: AgentConfig,  // old AgentConfig had `model: ModelConfig`

    // --- NEW ---
    pub models: HashMap<String, ModelProfile>,
    pub default_model: String,                // default profile name
    pub agent: AgentConfig,                   // AgentConfig no longer has `model`
    // ... other fields unchanged
}
```

- `AgentConfig` removes `pub model: ModelConfig`.
- `Config` adds `pub models: HashMap<String, ModelProfile>` and `pub default_model: String`.
- On deserialization, if `models` is empty, `ModelRegistry::new()` fails with a clear error: `"No model profiles configured. Define at least one [models.<name>] table."`.
- `Config::model()` accessor is **deleted**. Callers use `model_registry.resolve(name)` instead.

#### ModelRegistry (`new file: model_registry.rs`)
- New module inside `kernel`.
- Constructed from `Config` during `Coordinator` startup.
- Exposes `resolve(name)` → `(Arc<dyn Provider>, Arc<ModelConfig>)`.
- Exposes `resolve_with_context(name)` → `ResolvedModel` (includes `context_window`).
- Exposes `list_profiles()` for UI consumption.
- Requires at least one profile on construction, otherwise fails.

#### AgentConfig (`agent/types.rs`)
```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    // --- REMOVED ---
    // pub model: ModelConfig,

    // --- UNCHANGED ---
    pub max_iterations: usize,
    pub enable_subagent: bool,
    pub system_prompt: String,
    pub skills: Vec<Arc<Skill>>,
    pub tool_blocklist: Vec<String>,
    pub compactor: Compactor,  // no longer contains context_window
    pub allow_command_hooks: bool,
}
```

#### AgentShared (`agent/types.rs`)
```rust
pub struct AgentShared {
    pub model_registry: Arc<ModelRegistry>,
    pub current_profile: String,
    // ... all other fields remain unchanged
}
```

Add convenience methods:
```rust
impl AgentShared {
    pub fn provider(&self) -> Arc<dyn Provider>;
    pub fn model_config(&self) -> Arc<ModelConfig>;
    pub fn context_window(&self) -> u32;  // reads from active profile
}
```

#### Agent (`agent/agent.rs`)
- Replace all `self.shared.provider.clone()` and `self.shared.model_config.clone()` with calls that resolve via `self.shared.current_profile`.
- The resolution happens at the **start of each streaming iteration**, so the switch is naturally safe.
- Compaction calls pass `context_window` from the resolved model profile.

#### Coordinator (`app/coordinator.rs`)
- `Coordinator::new()` receives `Arc<ModelRegistry>` instead of `provider + model_config`.
- It constructs `AgentShared` with `model_registry` and `current_profile = config.default_model`.
- New API: `list_models()` → `Vec<ProfileInfo>` (for UI dropdowns).
- New API: `switch_session_model(session_id, profile_name)` → validates `profile_name` exists, then updates the session's `AgentShared.current_profile`. Only allowed if the agent is `Idle`.

#### Session (`app/session.rs`)
- New method: `set_model_profile(name: &str)` that updates the local `AgentShared.current_profile`.
- Only callable when `agent_state() == Some(Idle)`.

### 5.3 CLI (`crates/cli`)

- `yomi --model fast` CLI flag: select profile at session creation time.
- `yomi models list` subcommand: list available profiles.
- `yomi models switch <profile>` command: send a signal to the running session to switch profile (if idle).

### 5.4 GUI / TUI (`crates/gui`, `crates/tui`)

- Top toolbar or status bar shows current model profile name and context window (e.g. `fast: gpt-4o (128k)`).
- Dropdown / picker to select from `list_models()`.
- Switch is sent via IPC/command to the kernel. The UI waits for confirmation that the agent is idle before applying the switch.

## 6. Runtime Model Switch

### 6.1 机制概述

每个 Session 的 `AgentShared` 持有 `current_profile: String`。切换模型就是**修改这个字符串** — 简单、原子、不需要重建 Session 或 Agent。

关键设计点：
- **惰性生效**：`current_profile` 修改后，**当前正在进行的对话不受影响**。新模型在**下一个 streaming iteration** 开始时通过 `resolve()` 生效。
- **Idle 强制检查**：切换只能发生在 `AgentState::Idle`，防止 provider / model_config 在流中途被替换导致不一致。
- **自动级联**：子 Agent（sub-agent）与父 Agent 共享同一个 `AgentShared` 实例，所以父 Agent 切换模型后，子 Agent 也会自动跟随切换。

### 6.2 切换流程

```
User 在 GUI 选择 "fast" 模型
        │
        ▼
┌──────────────────────────────────────────────┐
│ GUI 发送命令: switch_model(session_id, "fast")│
└──────────────────────────────────────────────┘
        │
        ▼
┌──────────────────────────────────────────────┐
│ Coordinator::switch_session_model()          │
│  1. 检查 profile "fast" 在 ModelRegistry 中存在 │
│     ├─ 不存在 → 返回 ModelError::NotFound     │
│     └─ 存在   → 继续                          │
│  2. 获取 Session 的 RwLock                   │
│  3. 检查 session.agent_state() == Idle       │
│     ├─ 不是 Idle → 返回 BusyError            │
│     └─ 是 Idle   → 继续                      │
│  4. 调用 session.set_model_profile("fast")   │
│     (直接修改 AgentShared.current_profile)   │
│  5. 可选: 发送 SwitchModelEvent 到 event bus │
└──────────────────────────────────────────────┘
        │
        ▼
┌──────────────────────────────────────────────┐
│ GUI 收到确认                                  │
│ 更新状态栏显示: "fast: gpt-4o (128k)"          │
└──────────────────────────────────────────────┘
```

### 6.3 下一个对话周期的生效过程

当用户发送新消息时：

```rust
// agent/agent.rs 中每次 streaming iteration 开始
let resolved = shared.model_registry.resolve_with_context(&shared.current_profile)?;
// 此时 resolved 已经是 "fast" 对应的 GPT-4o provider + 128k context_window

let provider = resolved.provider;           // OpenAIProvider
let model_config = resolved.model_config;   // ModelConfig { model_id: "gpt-4o", ... }
let context_window = resolved.context_window; // 128_000

// 1. 先做 compaction 检查（用新的 context_window）
if compactor.should_compact(&messages, context_window) { ... }

// 2. 再发起 streaming 请求
let stream = provider.stream(&messages, &tools, &model_config).await?;
```

### 6.4 状态变更通知

切换成功后，内核通过 event bus 发送事件：

```rust
#[derive(Debug, Clone)]
pub enum ModelEvent {
    ProfileSwitched {
        session_id: SessionId,
        old_profile: String,
        new_profile: String,
        model_id: String,
        context_window: u32,
    },
}
```

UI 层监听此事件，实时更新：
- 状态栏当前模型显示
- 可用工具列表（未来若支持 per-model tool）
- 会话标题中的模型标识（可选）

### 6.5 切换失败场景

| 场景 | 原因 | 返回错误 |
|------|------|----------|
| Agent 正在 Streaming | 流中途不能换 provider | `SessionError::AgentBusy` |
| Agent 正在 ExecutingTool | 工具执行中 | `SessionError::AgentBusy` |
| Profile 不存在 | 用户输入了未配置的 profile | `ModelError::ProfileNotFound("fast")` |
| Profile 的 API key 未配置 | 虽然 profile 存在但 key 为空 | `ModelError::ApiKeyMissing`（启动时检查）|
| Session 已关闭 | 切换时 session 被 pruner 清理 | `SessionError::NotFound` |

### 6.6 与 Session 恢复的关系

恢复 Session 时，`current_profile` 需要持久化：

```rust
// 方案 A: 存到 session store 数据库
ALTER TABLE sessions ADD COLUMN current_profile TEXT NOT NULL DEFAULT 'default';

// 方案 B: 不存，恢复时总是 fallback 到 default_model
// 简单但丢失用户上次切换的选择
```

建议**方案 A**，在 `session_store.create()` 和 `session_store.update()` 时读写 `current_profile`。

## 7. Edge Cases & Constraints

| Case | Handling |
|------|----------|
| **Agent is streaming** | Switch rejected. The UI shows a warning. The user must wait for the current turn to finish (Idle state). |
| **Profile name does not exist** | `switch_session_model` returns `ModelError::ProfileNotFound`. |
| **Session is restored from storage** | Restored sessions use the same `current_profile` they had when saved. This requires storing the profile name in the `Session` or `SessionConfig` persistence layer. |
| **All profiles deleted at runtime** | At minimum, one default profile must always exist. The registry ensures this invariant. |
| **Provider cache invalidation** | Provider instances are cached by `ModelProvider` enum and never invalidated. If a user edits a profile's provider variant, a new provider instance is created lazily. |
| **Sub-agents** | Sub-agents inherit the same `AgentShared` (and therefore `current_profile`). They will use the same model and context window as the parent. This is intentional for consistency. |
| **Session 恢复后模型丢失** | 恢复时 fallback 到 default。建议 session 表存 `current_profile` 字段。 |
| **UI 在切换时显示旧模型** | 订阅 `ModelEvent::ProfileSwitched`，收到事件立即刷新状态栏。 |
| **快速连续切换** | 第二次切换在第一次完成后才能进行（Idle 状态下）。 |
| **切换时配置文件被外部修改** | ModelRegistry 只读启动后数据。运行时改配置文件需重启生效。 |

---

**快速切换两次模型示例**：

```
T0: Session Idle, current = "default"
T1: User 切到 "fast" → OK, current = "fast"
T2: User 再切到 "local" → OK, current = "local"（因为 T1 后仍是 Idle）
T3: User 发送消息 → 使用 "local" 的 provider 和 context_window
```

**切换时 Agent 正忙示例**：

```
T0: Session Streaming, current = "default"
T1: User 切到 "fast" → 返回 BusyError
T2: Agent 完成当前 turn → State 变为 Idle
T3: User 再次切到 "fast" → OK, current = "fast"
T4: User 发送消息 → 使用 "fast" 的 provider
```

## 8. Data Flow Diagram

```
┌─────────────────────────────────────────────────────────────┐
│                         Config File                         │
│  [models.default]  [models.fast]  [models.local]          │
│  context_window = 200000 | 128000 | 8000                  │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                      Config::from_file()                    │
│  (deserializes into HashMap<String, ModelProfile>)          │
│  ⚠️  Fails if models table is empty                           │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                     ModelRegistry::new()                    │
│  • validates profiles (at least one required)               │
│  • creates provider instances per variant                   │
│  • stores default profile name                              │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
        ┌─────────────────────┴─────────────────────┐
        │                                             │
        ▼                                             ▼
┌──────────────┐                          ┌──────────────┐
│ Session A    │                          │ Session B    │
│ current =    │                          │ current =    │
│ "default"    │                          │ "fast"       │
└──────────────┘                          └──────────────┘
        │                                             │
        ▼ (on next turn)                              ▼ (on next turn)
┌──────────────┐                          ┌──────────────┐
│ resolve() →  │                          │ resolve() →  │
│ Anthropic    │                          │ OpenAI       │
│ Provider     │                          │ Provider     │
│ ctx = 200k   │                          │ ctx = 128k   │
└──────────────┘                          └──────────────┘
        │                                             │
        ▼                                             ▼
┌──────────────┐                          ┌──────────────┐
│ compactor.   │                          │ compactor.   │
│ should_compact│                         │ should_compact│
│ (ctx=200k)   │                          │ (ctx=128k)   │
└──────────────┘                          └──────────────┘
```

## 9. Implementation Phases

### Phase 1: Config + Registry + Compactor refactor (no runtime switch yet)
1. Remove `model: ModelConfig` from `AgentConfig`.
2. Remove `context_window` from `Compactor`; add it to `ModelProfile`.
3. Update `Compactor::should_compact()` and `threshold()` to accept `context_window` as a parameter.
4. Add `ModelProfile` and `ModelRegistry` types.
5. Modify `Config` to require `models` table, remove `agent.model` support.
6. Modify `Coordinator::new()` to build `ModelRegistry`.
7. Modify `AgentShared` to hold `model_registry + current_profile`.
8. Update `Agent` loop to resolve via registry and pass `context_window` to compactor.
9. Update all tests and example configs to use new `models` table.

### Phase 2: Per-session creation selection
1. Add `model_profile` field to `CreateSessionInput`.
2. Pass selected profile through `SessionConfig` → `AgentShared`.
3. Add `Coordinator::list_models()` API.
4. Wire up GUI/TUI dropdown at session creation.

### Phase 3: Runtime switch
1. **内核层**
   - 新增 `ModelEvent::ProfileSwitched` 事件类型
   - 实现 `Session::set_model_profile(name)`：
     - 检查 `agent_state() == Some(Idle)`
     - 修改 `AgentShared.current_profile`
     - 可选：发送 `ProfileSwitched` 事件
   - 实现 `Coordinator::switch_session_model(session_id, profile_name)`：
     - 校验 profile 存在
     - 获取 session 锁
     - 校验 Idle 状态
     - 调用 `session.set_model_profile()`
   - 新增 `ModelError` 类型：`ProfileNotFound`, `AgentBusy`, `ApiKeyMissing`

2. **持久化**
   - 在 session 数据库表增加 `current_profile` 字段
   - `session_store.create()` 写入 default profile
   - `session_store.get()` 读取并恢复
   - `restore_session()` 用持久化的 profile 初始化 `AgentShared`

3. **CLI 层**
   - `yomi models switch <profile>`：向运行中的 session 发送切换命令
   - `yomi models list`：列出所有可用 profile

4. **GUI / TUI 层**
   - 状态栏显示当前 profile 名 + model_id + context_window
   - 下拉框选择 profile，触发 `switch_model` 命令
   - 监听 `ProfileSwitched` 事件，自动刷新 UI
   - 切换失败时显示 toast / 提示（如 "Agent is busy"）

5. **测试**
   - 单元测试：Idle 时切换成功，Streaming 时切换被拒绝
   - 集成测试：切换后下一条消息使用新模型
   - 回归测试：恢复 session 时保持上次切换的模型

## 10. API Sketch (for reference)

```rust
// ModelProfile
pub struct ModelProfile {
    pub name: String,
    pub provider: ModelProvider,
    pub config: ModelConfig,
    pub context_window: u32,
}

// ResolvedModel — 运行时 resolve 的完整结果
pub struct ResolvedModel {
    pub provider: Arc<dyn Provider>,
    pub model_config: Arc<ModelConfig>,
    pub context_window: u32,
}

// ModelRegistry
impl ModelRegistry {
    pub fn from_config(config: &Config) -> Result<Self, ModelRegistryError>;
    pub fn resolve(&self, name: &str) -> Option<(Arc<dyn Provider>, Arc<ModelConfig>)>;
    pub fn resolve_with_context(&self, name: &str) -> Option<ResolvedModel>;
    pub fn list_profiles(&self) -> Vec<ProfileInfo>;
    pub fn default_profile(&self) -> &str;
}

// Coordinator
impl Coordinator {
    pub fn list_models(&self) -> Vec<ProfileInfo>;
    pub async fn switch_session_model(
        &self,
        session_id: &SessionId,
        profile_name: &str,
    ) -> Result<ModelSwitchResult, ModelError>;
}

// Session
impl Session {
    pub fn set_model_profile(&mut self, name: &str) -> Result<(), ModelError>;
    pub fn current_model_profile(&self) -> &str;
}

// Compactor
impl Compactor {
    pub fn threshold(&self, context_window: u32) -> u32;
    pub fn should_compact(&self, messages: &[Arc<Message>], context_window: u32) -> bool;
    pub fn compact(&self, messages: &[Arc<Message>], context_window: u32) -> CompactionResult;
}

// Events (for UI notification)
pub enum ModelEvent {
    ProfileSwitched {
        session_id: SessionId,
        old_profile: String,
        new_profile: String,
        model_id: String,
        context_window: u32,
    },
}

// Errors
pub enum ModelError {
    ProfileNotFound(String),
    AgentBusy { current_state: AgentState },
    ApiKeyMissing { profile: String },
}

// Result type
pub enum ModelSwitchResult {
    Switched { old: String, new: String },
    NoChange, // new profile == old profile
}
```

## 11. Open Questions

1. **Should we store `current_profile` in the session database?** If yes, add a column to the sessions table. If no, restored sessions always fall back to the default profile.
2. **Should profiles support inheritance / templates?** E.g. a base profile with `endpoint` and children overriding `model_id`. This is a nice-to-have; the v1 design uses flat profiles.
3. **Should we allow runtime profile creation?** The registry is `Arc`-shared; adding profiles at runtime would require `RwLock` or a channel. For v1, profiles are read-only after startup.
4. **What about tool definitions per model?** Some models have different tool capabilities (e.g. reasoning). For now, assume all profiles support the same tool set; the agent loop handles model-specific behavior via `model_config` already.

---

*Document version: 3.0 (breaking + per-model context_window + runtime switch)*
*Target: yomi kernel v2 multi-model support*
