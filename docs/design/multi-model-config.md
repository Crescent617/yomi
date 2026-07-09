# 设计文档：多 Model 配置与 Session 级别运行时切换

## 背景

当前 `Config` 只支持单 `model`（`agent.model`），`Compactor` 的 `context_window` 也是全局固定的。随着不同模型（Claude、GPT、自托管等）的接入需求，我们需要：

1. **配置层支持多 model**：`config.toml` 中定义 `models` 数组，每个 model 有 `name`、provider、endpoint、api_key、**context_window** 等。
2. **Session 级别运行时切换**：每个 session 可以独立切换当前使用的 model，**只影响内存**，不写入配置文件。切换后该 session 的后续 streaming 生效。
3. **context_window 归属 model**：`Compactor` 的 `context_window` 是模型属性，应随 model 切换而变化，而不是全局固定。

---

## 目标

- `ModelConfig` 增加 `name` 和 `context_window` 字段。
- `Config` 增加 `models: Vec<ModelConfig>` 数组，`AgentConfig` 的 `default_model` 改为 `String`（指向 `models` 中的某个 `name`）。
- 直接删除 `AgentConfig.model` 字段，不再做单模型兼容。`Config::default()` 初始化 `models` 包含一个默认模型。
- `Compactor` 移除 `context_window` 字段，运行时从 `ModelConfig` 获取。
- `Kernel` 维护 `models: Arc<BTreeMap<String, ModelConfig>>`（只读，从配置数组构建，按 `name` 排序），以及 `session_models: Arc<DashMap<SessionId, String>>`（per-session 当前 model name，与 `Conductor` 共享）。
- `Conductor` 在 `wake_agent` 时根据 `session_models` 动态创建对应的 `Provider` 和 `ModelConfig`。
- `Agent` 新增 `AgentInput::SwitchModel` 变体，只传 `model_key`，`Agent` 从 `AgentShared.model_registry` 查表创建 provider，热更新 `AgentShared`。
- `CreateSessionInput` 支持 `model_key` 参数，创建时指定初始模型（不传则用 `agent.default_model`）。
- GUI 暴露 `list_models` / `get_session_model` / `set_session_model` 命令，前端 model selector 绑定到当前 session。

---

## 范围

### 做（In Scope）

- `ModelConfig` 加 `name` + `context_window`。
- `Config` 加 `models: Vec<ModelConfig>`，`AgentConfig` 的 `default_model` 改为 `String`。
- 删除 `AgentConfig.model` 字段，所有依赖改为从 `models` 数组获取。
- `Compactor` 移除 `context_window`，方法签名增加 `context_window` 参数。
- `Agent` 中所有 `compactor` 调用点传入 `model_config.context_window`。
- `AgentShared` 增加 `model_registry: Arc<BTreeMap<String, ModelConfig>>`。
- `Kernel` 维护 `models`（只读 `BTreeMap`）和 `session_models`（共享 `DashMap`）。
- `Conductor` 在 `wake_agent` 时根据 `session_models` 动态注入 `provider` + `model_config`。
- `AgentInput` 新增 `SwitchModel` 变体，`Agent::handle_input` 支持热更新 model。
- `CreateSessionInput` 增加 `model_key: Option<String>`。
- `KernelApi` 增加 `list_models`、`get_session_model`、`set_session_model`。
- Wire protocol 增加 `ListModels`、`GetSessionModel`、`SetSessionModel`（协议版本升级）。
- GUI 后端增加 `get_models`、`get_session_model`、`set_session_model` Tauri 命令。
- GUI 前端增加 model selector 组件（绑定到当前 session）。

### 不做（Out of Scope）

- **不落库**：`session_models` 为纯内存，重启后恢复 `agent.default_model`。
- **不切换已运行 streaming 的 model**：`SwitchModel` 进入 mailbox，等当前 streaming 结束后处理，下一轮生效。
- **不自动 fallback model 切换**：`fallback_model_id` 仍只是字符串标识。
- **不修改 usage 存储的 model 关联**：usage 记录按实际 `model_id` 记录。
- **不持久化每个 session 的 model 到 session store**。

---

## 核心原则

1. **Session 级别隔离**：每个 session 独立管理 `model_key`。新 session 继承 `agent.default_model`，但后续可独立切换。
2. **运行时热更新**：`AgentInput::SwitchModel` 在 `handle_input` 中更新 `AgentShared`，下一轮 streaming 生效。
3. **最小改动**：`EventBus`、`MessageBuffer` 不做结构性修改。
4. **统一 snake_case**：TOML 配置、Wire 协议、Tauri IPC、前端 TS 接口统一使用 `snake_case`。

---

## 数据模型变更

### 1. `ModelConfig`（`crates/kernel/src/provider/mod.rs`）

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// 模型的唯一标识名（如 "claude_sonnet"、"gpt4o"）
    pub name: String,
    pub provider: crate::config::ModelProvider,
    pub model_id: String,
    pub endpoint: String,
    pub api_key: String,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub fallback_model_id: Option<String>,
    pub sse_timeout_secs: u64,
    pub thinking: ThinkingConfig,
    #[serde(skip_serializing_if = "HashMap::is_empty", default)]
    pub headers: HashMap<String, String>,
    /// 该模型对应的上下文窗口大小
    pub context_window: u32,
}

impl Default for ModelConfig {
    fn default() -> Self {
        Self {
            name: "default".to_string(),
            provider: crate::config::ModelProvider::default(),
            model_id: String::new(),
            endpoint: String::new(),
            api_key: String::new(),
            max_tokens: None,
            temperature: None,
            fallback_model_id: None,
            sse_timeout_secs: 30,
            thinking: ThinkingConfig::default(),
            headers: HashMap::new(),
            context_window: 131_072, // 128k
        }
    }
}

impl ModelConfig {
    #[inline]
    pub const fn has_api_key(&self) -> bool {
        !self.api_key.is_empty()
    }
}
```

### 2. `Compactor`（`crates/kernel/src/compactor/mod.rs`）

移除 `context_window` 字段，所有方法改为接收 `context_window` 参数。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Compactor {
    pub threshold_ratio: f32,
    pub keep_recent: usize,
    pub summary_max_tokens: u32,
}

impl Default for Compactor {
    fn default() -> Self {
        Self {
            threshold_ratio: DEFAULT_THRESHOLD_RATIO,
            keep_recent: KEEP_RECENT_MESSAGES,
            summary_max_tokens: SUMMARY_MAX_TOKENS,
        }
    }
}

impl Compactor {
    pub const fn new(
        threshold_ratio: f32,
        keep_recent: usize,
        summary_max_tokens: u32,
    ) -> Self {
        Self { threshold_ratio, keep_recent, summary_max_tokens }
    }

    #[allow(clippy::cast_precision_loss)]
    pub fn threshold(&self, context_window: u32) -> u32 {
        (context_window as f32 * self.threshold_ratio) as u32
    }

    pub fn should_compact(&self, messages: &[Arc<Message>], context_window: u32) -> bool {
        Self::calculate_tokens(messages) >= self.threshold(context_window)
    }

    pub fn micro_compact(&self, messages: &[Arc<Message>]) -> Option<Vec<Arc<Message>>> { ... }

    pub async fn full_compact(...) -> Result<CompactionResult, CompactionError> { ... }

    pub async fn auto_compact(
        &self,
        messages: &[Arc<Message>],
        provider: Arc<dyn Provider>,
        model_config: &ModelConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Result<Option<CompactionResult>, CompactionError> {
        if !self.should_compact(messages, model_config.context_window) {
            return Ok(None);
        }
        // ... 其余逻辑不变
    }
}
```

### 3. `AgentConfig`（`crates/kernel/src/agent/types.rs`）

删除 `model` 字段，`default_model` 改为 `String`（不再 `Option`）。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    /// 默认激活的模型名（指向 `Config.models` 中的某一项 `name`）
    pub default_model: String,
    pub max_iterations: usize,
    pub enable_subagent: bool,
    pub system_prompt: String,
    #[serde(skip)]
    pub skills: Vec<Arc<Skill>>,
    pub tool_blocklist: Vec<String>,
    pub compactor: Compactor,
    pub allow_command_hooks: bool,
    pub max_tool_output_length: usize,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            default_model: "default".to_string(),
            max_iterations: 100,
            enable_subagent: true,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            skills: Vec::new(),
            tool_blocklist: Vec::new(),
            compactor: Compactor::default(),
            allow_command_hooks: false,
            max_tool_output_length: 40_000,
        }
    }
}
```

### 4. `Config`（`crates/kernel/src/config/mod.rs`）

删除 `agent.model` 的 fallback 逻辑，`models` 数组必须有至少一个元素，`default_model` 指向 `models` 中的某个 `name`。

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub agent: AgentConfig,
    pub auto_approve: Level,
    pub data_dir: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_dir: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill_folders: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub hooks: Vec<crate::hooks::HookEntry>,
    pub features: FeaturesConfig,
    pub max_checkpoints: usize,
    #[serde(default)]
    pub channels: Vec<crate::channels::ChannelConfig>,
    /// 多模型配置数组（TOML: `[[models]]`），至少一个元素
    #[serde(default)]
    pub models: Vec<ModelConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let data_dir = expand_tilde(DEFAULT_DATA_DIR);
        Self {
            agent: AgentConfig::default(),
            auto_approve: Level::default(),
            data_dir,
            log_dir: None,
            skill_folders: None,
            hooks: Vec::new(),
            features: FeaturesConfig::default(),
            max_checkpoints: 5,
            channels: Vec::new(),
            models: vec![ModelConfig::default()],
        }
    }
}

impl Config {
    /// 获取默认 model 配置（finalize() 后 models 保证非空且 default_model 有效）
    #[inline]
    pub fn model(&self) -> &ModelConfig {
        self.models.iter()
            .find(|m| m.name == self.agent.default_model)
            .expect("default_model must exist in models")
    }

    /// 初始化必要的默认值
    pub fn finalize(&mut self) {
        // ... 现有逻辑（展开 ~、log_dir、skill_folders） ...

        // 如果 models 为空，用默认模型兜底（这不应该发生，除非 config.toml 未加载）
        if self.models.is_empty() {
            self.models.push(ModelConfig::default());
        }
        // 如果 default_model 指向不存在的模型，fallback 到 models[0].name
        if !self.models.iter().any(|m| m.name == self.agent.default_model) {
            self.agent.default_model = self.models[0].name.clone();
        }
    }

    /// 环境变量覆盖逻辑（改造后）
    fn load_from_env(&mut self) {
        // 单模型 env 变量作用于 models[0]（默认模型）
        let default_model = &mut self.models[0];

        // Provider selection
        if let Some(provider) = env_var(env_names::PROVIDER) {
            if let Ok(p) = provider.parse() {
                default_model.provider = p;
            }
        }
        let provider = default_model.provider;

        // API Key / Model / Endpoint
        if let Some(key) = env_first(&[env_names::API_KEY, provider.standard_api_key_env()]) {
            default_model.api_key = key;
        }
        if let Some(model) = env_first(&[env_names::MODEL, provider.standard_model_env()]) {
            default_model.model_id = model;
        }
        if let Some(endpoint) = env_first(&[env_names::API_BASE, provider.standard_api_base_env()]) {
            default_model.endpoint = endpoint;
        }

        // Numeric settings
        if let Some(max_tokens) = env_var(env_names::MAX_TOKENS) {
            if let Some(tokens) = parse_number_with_unit(&max_tokens) {
                default_model.max_tokens = Some(tokens);
            }
        }
        if let Some(temp) = env_parse::<f32>(env_names::TEMPERATURE) {
            default_model.temperature = Some(temp);
        }
        if let Some(budget) = env_parse::<u32>(env_names::THINKING_BUDGET) {
            default_model.thinking.budget_tokens = budget;
        }

        // Boolean settings
        if let Some(enabled) = env_bool_opt(env_names::THINKING) {
            default_model.thinking.enabled = enabled;
        }
        if let Some(effort) = env_var(env_names::THINKING_EFFORT) {
            default_model.thinking.effort = Some(effort);
        }

        // Context window（从 model 获取，作用于默认模型）
        if let Some(context_window) = env_var(env_names::CONTEXT_WINDOW) {
            if let Some(tokens) = parse_number_with_unit(&context_window) {
                default_model.context_window = tokens;
            }
        }

        // 新增：YOMI_DEFAULT_MODEL 覆盖全局默认模型名
        if let Some(key) = env_var(env_names::DEFAULT_MODEL) {
            self.agent.default_model = key;
        }

        // 其余与 model 无关的 env 变量保持不变
        if let Some(iters) = env_parse::<usize>(env_names::MAX_ITERATIONS) {
            self.agent.max_iterations = iters;
        }
        if let Some(val) = env_var(env_names::ENABLE_SUB_AGENTS) {
            self.agent.enable_subagent = val != "false";
        }
        if let Some(dir) = env_var(env_names::DATA_DIR) {
            self.data_dir = expand_tilde(dir);
        }
        if let Some(dir) = env_var(env_names::LOG_DIR) {
            self.log_dir = Some(expand_tilde(dir));
        }
        if let Some(folders) = env_var(env_names::SKILL_FOLDERS) {
            self.skill_folders = Some(folders.split(',').map(String::from).collect());
        }
        if let Some(level) = env_var(env_names::AUTO_APPROVE) {
            if let Ok(l) = Level::from_str(&level) {
                self.auto_approve = l;
            }
        }
        if let Some(ratio) = env_parse::<f32>(env_names::COMPACTOR_RATIO) {
            self.agent.compactor.threshold_ratio = ratio.clamp(0.0, 1.0);
        }
        if let Some(max) = env_parse::<usize>(env_names::MAX_CHECKPOINTS) {
            self.max_checkpoints = max;
        }
        if let Some(list) = env_var(env_names::TOOL_BLOCKLIST) {
            self.agent.tool_blocklist = list
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        if let Some(val) = env_bool_opt(env_names::ALLOW_COMMAND_HOOKS) {
            self.features.allow_command_hooks = val;
        }
        if let Some(max_len) = env_parse::<usize>(env_names::MAX_TOOL_OUTPUT_LENGTH) {
            self.agent.max_tool_output_length = max_len;
        }
    }
}
```

### 5. `AgentShared`（`crates/kernel/src/agent/types.rs`）

```rust
pub struct AgentShared {
    pub provider: Arc<dyn crate::provider::Provider>,
    pub model_config: Arc<ModelConfig>,
    /// 模型注册表，用于 SwitchModel 时查表创建 provider
    pub model_registry: Arc<std::collections::BTreeMap<String, ModelConfig>>,
    // ... 其余字段不变 ...
}

impl AgentShared {
    // ...

    #[must_use]
    pub fn with_model_registry(
        mut self,
        registry: Arc<std::collections::BTreeMap<String, ModelConfig>>,
    ) -> Self {
        self.model_registry = registry;
        self
    }
}
```

### 6. `AgentInput`（`crates/kernel/src/agent/agent.rs`）

```rust
pub enum AgentInput {
    User { content: Vec<ContentBlock> },
    Continue,
    Cancel,
    Steer(Vec<ContentBlock>),
    PermissionResponse { req_id: String, approved: bool, remember: bool },
    Shutdown,
    Compact,
    Rewind { ... },
    Clear,
    AskUserResponse { req_id: String, response: crate::tools::AskUserResponse },
    /// 运行时切换模型（不下库，仅内存生效）。
    /// 只传 model_key，Agent 从 model_registry 查表创建 provider。
    SwitchModel { model_key: String },
}
```

### 7. `CreateSessionInput`（`crates/kernel/src/kernel/mod.rs`）

```rust
pub struct CreateSessionInput {
    pub project_id: Option<ProjectId>,
    pub working_dir: Option<std::path::PathBuf>,
    pub auto_approve_level: Level,
    pub tool_blocklist: Vec<String>,
    /// 指定初始模型名（不传则用 agent.default_model）
    pub model_key: Option<String>,
}
```

---

## 配置格式

```toml
[agent]
max_iterations = 100
enable_sub_agents = true
system_prompt = "..."
default_model = "claude_sonnet"

[agent.compactor]
threshold_ratio = 0.8
keep_recent = 6
summary_max_tokens = 8192

[[models]]
name = "claude_sonnet"
provider = "anthropic"
model_id = "claude-3-5-sonnet-20241022"
endpoint = "https://api.anthropic.com"
api_key = "sk-ant-..."
context_window = 200000
max_tokens = 4096

[[models]]
name = "gpt4o"
provider = "openai"
model_id = "gpt-4o"
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
context_window = 128000

[[models]]
name = "gpt4o_mini"
provider = "openai"
model_id = "gpt-4o-mini"
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
context_window = 128000
max_tokens = 16384
```

> `models` 数组至少有一个元素。`agent.default_model` 指向其中一个 `name`。

---

## 各层改动

### 1. Provider 工厂（`crates/kernel/src/lib.rs`）

```rust
pub fn create_provider_for_model(model: &ModelConfig) -> Result<Arc<dyn Provider>> {
    if !model.has_api_key() {
        tracing::warn!("No API key for model '{}' — using NoKeyProvider", model.model_id);
        return Ok(Arc::new(NoKeyProvider));
    }
    match model.provider {
        ModelProvider::OpenAI => Ok(Arc::new(OpenAIProvider::new()?)),
        ModelProvider::Anthropic => Ok(Arc::new(AnthropicProvider::new()?)),
    }
}

pub fn create_provider(config: &Config) -> Result<Arc<dyn Provider>> {
    create_provider_for_model(config.model())
}
```

### 2. `Kernel` 改造（`crates/kernel/src/kernel/mod.rs`）

```rust
pub struct Kernel {
    agent_shared_template: Arc<AgentShared>,
    input_bus: Arc<InputBus>,
    conductor: Arc<Conductor>,
    agent_config: AgentConfig,
    /// 只读模型注册表（从 Config.models 构建，按 name 排序）
    models: Arc<std::collections::BTreeMap<String, ModelConfig>>,
    /// 每个 session 当前使用的 model name（与 Conductor 共享）
    session_models: Arc<DashMap<SessionId, String>>,
    project_store: Arc<dyn ProjectStore>,
    pinned_session_store: Arc<dyn PinnedSessionStore>,
    cron_store: Option<Arc<dyn CronStore>>,
    channel_manager: Option<Arc<ChannelHub>>,
    notification_bus: Arc<NotificationBus>,
    shutdown: CancellationToken,
}

impl Kernel {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        storage: &StorageSet,
        provider: Arc<dyn Provider>,
        agent_config: AgentConfig,
        task_store: Option<Arc<TaskStore>>,
        compactor: Option<Compactor>,
        skill_folders: Vec<PathBuf>,
        hook_registry: Option<HookRegistry>,
        enable_cron: bool,
        channel_store: Option<Arc<dyn ChannelStore>>,
        models: Vec<ModelConfig>,
    ) -> Arc<Self> {
        let models_map: Arc<std::collections::BTreeMap<String, ModelConfig>> = Arc::new(
            models.into_iter().map(|m| (m.name.clone(), m)).collect()
        );
        let session_models: Arc<DashMap<SessionId, String>> = Arc::new(DashMap::new());

        let agent_shared = AgentShared::with_data_dir(
            provider,
            Arc::new(agent_config.model.clone()),
            task_store,
            Some(todo_storage),
            compactor,
            Some(session_store),
            Some(message_store),
            Some(storage.usage_store()),
            None,
            skill_folders,
            None,
            Some(checkpoint_store),
            data_dir,
        )
        .with_model_registry(Arc::clone(&models_map));
        let agent_shared = Arc::new(agent_shared);

        let conductor = Arc::new(Conductor::new(
            agent_shared.clone(),
            agent_config.clone(),
            rx,
            event_bus,
            input_bus.clone(),
            base_prompt,
            data_dir_for_conductor,
            notification_bus.clone(),
            Arc::clone(&models_map),
            Arc::clone(&session_models),
        ));

        Arc::new(Self {
            agent_shared_template: agent_shared,
            input_bus,
            conductor,
            agent_config,
            models: models_map,
            session_models,
            // ...
        })
    }

    // ── Model API ──────────────────────────────────────────────────────

    pub async fn list_models(&self) -> Result<Vec<ModelInfo>> {
        Ok(self.models.values().map(|m| ModelInfo {
            name: m.name.clone(),
            model_id: m.model_id.clone(),
            provider: m.provider.to_string(),
            context_window: m.context_window,
        }).collect())
    }

    pub async fn get_session_model(&self, session_id: &SessionId) -> String {
        self.session_models
            .get(session_id)
            .map(|e| e.clone())
            .unwrap_or_else(|| self.agent_config.default_model.clone())
    }

    pub async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()> {
        if !self.models.contains_key(key) {
            return Err(SessionError::Other(format!("Model '{}' not found", key)).into());
        }
        self.session_models.insert(session_id.clone(), key.to_string());

        if self.conductor.get_state(session_id).is_some() {
            let input = AgentInput::SwitchModel { model_key: key.to_string() };
            self.input_bus.send(session_id.clone(), input).await;
        }
        Ok(())
    }

    // ── Session API ────────────────────────────────────────────────────

    pub async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        // ...
        let id = SessionId::new();
        let model_key = input.model_key
            .unwrap_or_else(|| self.agent_config.default_model.clone());
        self.session_models.insert(id.clone(), model_key);
        // ...
        Ok(id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub model_id: String,
    pub provider: String,
    pub context_window: u32,
}
```

### 3. `Conductor` 改造（`crates/kernel/src/kernel/conductor.rs`）

```rust
pub struct Conductor {
    agent_shared_template: Arc<AgentShared>,
    agent_config: AgentConfig,
    active: DashMap<SessionId, ActiveAgent>,
    mailboxes: DashMap<SessionId, Arc<Mailbox>>,
    rx: std::sync::Mutex<Option<InputBusSubscriber>>,
    event_bus: Arc<EventBus>,
    input_bus: Arc<InputBus>,
    base_prompt: String,
    data_dir: PathBuf,
    spawn_locks: DashMap<SessionId, Arc<tokio::sync::Mutex<()>>>,
    notification_bus: Arc<NotificationBus>,
    /// 只读模型注册表（与 Kernel 共享）
    models: Arc<std::collections::BTreeMap<String, ModelConfig>>,
    /// 每个 session 当前使用的 model name（与 Kernel 共享）
    session_models: Arc<DashMap<SessionId, String>>,
}

impl Conductor {
    pub fn new(
        agent_shared_template: Arc<AgentShared>,
        agent_config: AgentConfig,
        rx: InputBusSubscriber,
        event_bus: Arc<EventBus>,
        input_bus: Arc<InputBus>,
        base_prompt: String,
        data_dir: PathBuf,
        notification_bus: Arc<NotificationBus>,
        models: Arc<std::collections::BTreeMap<String, ModelConfig>>,
        session_models: Arc<DashMap<SessionId, String>>,
    ) -> Self {
        Self {
            agent_shared_template,
            agent_config,
            active: DashMap::new(),
            mailboxes: DashMap::new(),
            rx: std::sync::Mutex::new(Some(rx)),
            event_bus,
            input_bus,
            base_prompt,
            data_dir,
            spawn_locks: DashMap::new(),
            notification_bus,
            models,
            session_models,
        }
    }

    async fn wake_agent(&self, sid: &SessionId, mailbox: Arc<Mailbox>) {
        // ... 现有锁和检查逻辑 ...

        let model_key = self.session_models
            .get(sid)
            .map(|e| e.clone())
            .unwrap_or_else(|| self.agent_config.default_model.clone());

        let model_config = self.models.get(&model_key)
            .cloned()
            .expect("session model must exist in models");

        let provider = match create_provider_for_model(&model_config) {
            Ok(p) => p,
            Err(e) => {
                tracing::error!("Failed to create provider for '{}': {}", model_key, e);
                Arc::new(NoKeyProvider)
            }
        };

        let mut base_clone: AgentShared = (*self.agent_shared_template).clone();
        base_clone.provider = provider;
        base_clone.model_config = Arc::new(model_config);

        // ... 其余逻辑不变 ...
    }
}
```

### 4. `Agent` 中 `SwitchModel` 处理（`crates/kernel/src/agent/agent.rs`）

```rust
async fn handle_input(&mut self, input: AgentInput) -> Result<(), AgentError> {
    match input {
        // ... 现有分支 ...
        AgentInput::SwitchModel { model_key } => {
            tracing::info!("session {} switching model to '{}'", self.session_id.0, model_key);
            let model_config = self.shared.model_registry.get(&model_key)
                .ok_or_else(|| AgentError::Serialization(format!("Model '{}' not found", model_key)))?
                .clone();
            let provider = create_provider_for_model(&model_config)
                .map_err(AgentError::Provider)?;

            let shared = Arc::make_mut(&mut self.shared);
            shared.provider = provider;
            shared.model_config = Arc::new(model_config);
            Ok(())
        }
        // ... 其余分支 ...
    }
}
```

### 5. `Agent` 中 Compactor 调用改造（`crates/kernel/src/agent/agent.rs`）

```rust
impl Agent {
    fn maybe_compact_messages(&mut self) -> bool {
        let Some(compactor) = &self.shared.compactor else { return false };
        if !compactor.should_compact(self.message_buffer.messages(), self.shared.model_config.context_window) {
            return false;
        }
        // ... 触发 compaction ...
    }

    pub async fn force_compact(&mut self) -> Result<String, String> {
        let compactor = self.shared.compactor.as_ref().ok_or("No compactor")?;
        let result = compactor.auto_compact(
            self.message_buffer.messages(),
            Arc::clone(&self.shared.provider),
            &self.shared.model_config,
            Some(self.cancel_token.runtime_token()),
        ).await;
        // ...
    }
}
```

### 6. `build_kernel` 适配（`crates/kernel/src/lib.rs`）

```rust
pub async fn build_kernel(config: &Config, enable_cron: bool) -> Result<Arc<Kernel>> {
    // ... 现有逻辑 ...

    let default_model = config.model().clone();
    let provider = create_provider_for_model(&default_model)
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to create provider: {e}, using NoKeyProvider");
            Arc::new(NoKeyProvider)
        });

    let kernel = Kernel::new(
        &storage,
        provider,
        agent_config,
        Some(task_store),
        Some(config.agent.compactor.clone()),
        skill_folders,
        hooks,
        enable_cron,
        if config.channels.is_empty() { None } else { Some(storage.channel_store()) },
        config.models.clone(), // Vec<ModelConfig>
    );

    Ok(kernel)
}
```

### 7. `KernelApi` 扩展（`crates/kernel/src/client/mod.rs`）

```rust
#[async_trait]
pub trait KernelApi: Send + Sync {
    // ... 现有方法 ...
    async fn list_models(&self) -> Result<Vec<kernel::ModelInfo>>;
    async fn get_session_model(&self, session_id: &SessionId) -> Result<String>;
    async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()>;
}

#[async_trait]
impl KernelApi for Kernel {
    async fn list_models(&self) -> Result<Vec<kernel::ModelInfo>> {
        Self::list_models(self).await
    }
    async fn get_session_model(&self, session_id: &SessionId) -> Result<String> {
        Ok(Self::get_session_model(self, session_id).await)
    }
    async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()> {
        Self::set_session_model(self, session_id, key).await
    }
}
```

### 8. Wire Protocol 扩展（`crates/kernel/src/wire/mod.rs`）

```rust
pub enum ReqMethod {
    // ... 现有方法 ...
    ListModels,
    GetSessionModel { session_id: String },
    SetSessionModel { session_id: String, key: String },
}
```

`WIRE_PROTOCOL_VERSION` 升级（例如 `7 -> 8`）。

`RemoteKernel` 实现：

```rust
#[async_trait]
impl KernelApi for RemoteKernel {
    async fn list_models(&self) -> Result<Vec<kernel::ModelInfo>> {
        let result = self.call(ReqMethod::ListModels).await?;
        Ok(serde_json::from_value(result)?)
    }
    async fn get_session_model(&self, session_id: &SessionId) -> Result<String> {
        let result = self.call(ReqMethod::GetSessionModel {
            session_id: session_id.0.clone(),
        }).await?;
        Ok(serde_json::from_value(result)?)
    }
    async fn set_session_model(&self, session_id: &SessionId, key: &str) -> Result<()> {
        self.call(ReqMethod::SetSessionModel {
            session_id: session_id.0.clone(),
            key: key.to_string(),
        }).await?;
        Ok(())
    }
}
```

### 9. GUI 后端命令（`crates/gui/src/commands/system.rs`）

```rust
#[tauri::command(rename_all = "snake_case")]
pub async fn get_models(state: State<'_, AppState>) -> Result<serde_json::Value, GuiError> {
    let models = state.kernel.list_models().await.map_err(GuiError::kernel)?;
    Ok(serde_json::json!({ "models": models }))
}

#[tauri::command(rename_all = "snake_case")]
pub async fn get_session_model(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<String, GuiError> {
    let sid = kernel::SessionId::from(session_id);
    state.kernel.get_session_model(&sid).await.map_err(GuiError::kernel)
}

#[tauri::command(rename_all = "snake_case")]
pub async fn set_session_model(
    state: State<'_, AppState>,
    session_id: String,
    key: String,
) -> Result<(), GuiError> {
    let sid = kernel::SessionId::from(session_id);
    state.kernel.set_session_model(&sid, &key).await.map_err(GuiError::kernel)
}
```

### 10. GUI 前端

```typescript
// api.ts
export async function getModels() {
  return invoke<{ models: ModelInfo[] }>("get_models");
}
export async function getSessionModel(session_id: string) {
  return invoke<string>("get_session_model", { session_id });
}
export async function setSessionModel(session_id: string, key: string) {
  return invoke<void>("set_session_model", { session_id, key });
}
```

---

## 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| `context_window` 归属 | **`ModelConfig`** | 窗口大小是模型属性，随 model 切换而变。`Compactor` 只保留策略参数。 |
| 切换生效范围 | **Session 级别** | 每个 session 独立 `model_key`。`session_models` 用 `DashMap` 在 `Kernel` 和 `Conductor` 间共享。 |
| 切换是否落库 | **否** | 纯内存 `DashMap`，重启后恢复 `agent.default_model`。 |
| 切换时机 | **下一轮生效** | `SwitchModel` 进入 mailbox，agent 在 idle 时处理，下一次 streaming 用新 model。 |
| 新 session 默认值 | **`AgentConfig.default_model`** | 配置文件中 `[agent]` 下的 `default_model`，指向 `models` 数组中的 `name`。 |
| `models` 配置格式 | **`Vec<ModelConfig>`（TOML `[[models]]`）** | 每个 item 自带 `name` 字段。 |
| 运行时查找结构 | **`BTreeMap<String, ModelConfig>`** | 从 `Vec` 构建，按 `name` 排序，保证前端列表稳定，查找 O(log n)。 |
| `models` 是否需要锁 | **不需要** | 运行时只读，启动后不再修改。`Arc<BTreeMap>` 即可。 |
| `SwitchModel` 参数 | **只传 `model_key: String`** | `provider` 和 `model_config` 都是 `model_key` 的派生属性。`AgentShared` 挂 `model_registry`，`Agent` 自己查表。 |
| `AgentShared` 热更新 | **`Arc::make_mut`** | `Agent` 的 `shared` 是 `Arc<AgentShared>`，idle 时引用计数通常为 1，`make_mut` 几乎无开销。 |
| 旧配置兼容 | **不做** | 删除 `AgentConfig.model`，`Config::default()` 直接初始化 `models: vec![ModelConfig::default()]`。无需 backward compat 逻辑。 |
| Wire Protocol | **版本升级** | 新增 `ListModels` / `GetSessionModel` / `SetSessionModel`，`WIRE_PROTOCOL_VERSION` 递增。 |
| Env 变量覆盖 | **`YOMI_DEFAULT_MODEL` + 单模型变量作用于 `models[0]`** | `YOMI_PROVIDER`/`YOMI_MODEL`/`YOMI_API_KEY`/`YOMI_CONTEXT_WINDOW` 等覆盖 `models[0]`（默认模型）；`YOMI_DEFAULT_MODEL` 覆盖 `agent.default_model`。 |

---

## 数据流：Session 切换 Model

```
GUI 前端 ──set_session_model(session_id, "gpt4")──► KernelApi
                                                    │
                                                    ▼
                                              ┌──────────┐
                                              │ Kernel   │  1. 检查 models.contains_key("gpt4")
                                              │          │  2. session_models[sid] = "gpt4"
                                              │          │  3. 若 session 活跃：input_bus.send(SwitchModel)
                                              └────┬─────┘
                                                   │
                      ┌────────────────────────────┼────────────────────────────┐
                      │                            │                            │
                      ▼                            ▼                            ▼
                ┌──────────┐                ┌──────────┐                  ┌──────────┐
                │ session  │                │ session  │                  │ session  │
                │ idle     │                │streaming │                  │executing │
                │          │                │          │                  │          │
                └────┬─────┘                └────┬─────┘                  └────┬─────┘
                     │                           │                            │
                     │ SwitchModel               │ 在 mailbox 等待            │ 在 mailbox 等待
                     │ 立即处理                  │ streaming 结束             │ tool 完成
                     │                           │                            │
                     ▼                           ▼                            ▼
                ┌────────────────────────────────────────────────────────────────────┐
                │ Agent::handle_input(SwitchModel)                                   │
                │  1. model_registry.get("gpt4") -> ModelConfig                      │
                │  2. create_provider_for_model(&model_config) -> Provider            │
                │  3. Arc::make_mut(&mut self.shared)                                │
                │  4. 更新 shared.provider 和 shared.model_config                     │
                └────────────────────────────────────────────────────────────────────┘
                                                    │
                                                    ▼
                                              ┌──────────┐
                                              │ 下次消息  │
                                              │ streaming │
                                              │ 用新 model│
                                              └──────────┘
```

---

## 验证步骤

1. **配置加载**：加载含 `[[models]]` 的 `config.toml`，验证 `finalize()` 后 `models` 非空，`agent.default_model` 有效。
2. **Compactor 行为**：`Compactor::default()` 不再包含 `context_window`，验证 `should_compact` 需要传入 `context_window`。
3. **新 session 默认 model**：创建 session A，不传 `model_key`，验证使用 `agent.default_model`。
4. **创建时指定 model**：创建 session B，传 `model_key: Some("gpt4o")`，验证使用 `gpt4o`。
5. **Session 切换 model**：session A 运行中调用 `set_session_model(session_a, "gpt4o")`，验证：
   - idle 时立即处理；streaming 时在 mailbox 等待；
   - 切换后发送新消息，agent 使用 `gpt4o`（从日志确认）。
6. **Session 隔离**：session A 切换为 `gpt4o`，session B 仍为 `claude_sonnet`，验证两者独立。
7. **Context Window 生效**：`claude_sonnet` 配置 `context_window = 200000`，`gpt4o` 配置 `128000`，验证 `should_compact` 阈值不同。
8. **RemoteKernel**：连接 IPC daemon，验证 `list_models` / `set_session_model` 通过 wire protocol 正确工作。
9. **Env 覆盖（default_model）**：设置 `YOMI_DEFAULT_MODEL=gpt4o`，验证启动后新 session 默认使用 `gpt4o`。
10. **Env 覆盖（单模型变量）**：设置 `YOMI_MODEL=gpt-4o`，验证 `models[0].model_id` 被覆盖。
11. **NoKey 处理**：某个 model 未配置 `api_key`，验证 `create_provider_for_model` 返回 `NoKeyProvider`，发送消息时优雅报错。
12. **GUI 切换**：在 model selector 中切换当前 session 的 model，验证不触发 `save_config_toml`，且后续 chat 使用新模型。

---

## 实施顺序

1. `ModelConfig` 加 `name` + `context_window`（`provider/mod.rs`）。
2. `Compactor` 移除 `context_window`（`compactor/mod.rs` + 测试）。
3. `Agent` 适配 Compactor 调用（`agent/agent.rs`）。
4. `AgentConfig` 删除 `model`，`default_model` 改为 `String`（`agent/types.rs`）。
5. `Config` 删除 `agent.model` 兼容逻辑，`Default` 初始化 `models`（`config/mod.rs`）。
6. `AgentShared` 加 `model_registry`（`agent/types.rs`）。
7. `AgentInput` 新增 `SwitchModel`（`agent/agent.rs`）。
8. `CreateSessionInput` 加 `model_key`（`kernel/mod.rs`）。
9. `Kernel` 改造（`kernel/mod.rs`）。
10. `Conductor` 改造（`kernel/conductor.rs`）。
11. `build_kernel` 适配（`lib.rs`）。
12. `KernelApi` 扩展（`client/mod.rs`）。
13. Wire Protocol（`wire/mod.rs`）。
14. Server dispatch（`server/dispatcher.rs`）。
15. GUI 后端命令（`gui/commands/system.rs`）。
16. GUI 前端（`ModelSelector.svelte`）。
17. 端到端测试。
