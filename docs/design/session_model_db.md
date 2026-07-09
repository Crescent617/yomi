## 设计文档：Session Model Key 落库 + 每轮动态解析

### 核心结论

不用 WrappedProvider。当前架构下，每次 agent turn 都从数据库读取 model_key，创建/复用 provider，是更自然的方案。

**Agent 生命周期中的 model 解析时机：**
```
agent_loop:
  1. 从 mailbox 获取下一个输入
  2. 每次迭代开始前：resolve_model() → 从 session_store 读 model_key → 查 registry → 获取 provider + model_config
  3. 用解析到的 provider/model_config 执行 stream / compact / usage_record
  4. 继续下一轮
```

这样 agent 存活期间，每次 turn 都会重新读取当前 session 的 model_key，切换立即生效。

---

### 1. 数据库层

**Migration v13**：
```sql
ALTER TABLE sessions ADD COLUMN model_key TEXT;
```

**`SessionStore` trait 变更**：
- `create` 和 `fork` 增加 `model_key: Option<&str>` 参数
- 新增 `update_model_key(&self, id: &SessionId, key: &str) -> Result<u64>`
- `SessionInfo` 加 `model_key: Option<String>` 字段

**`fork` 时继承**：
```sql
INSERT INTO sessions (id, parent_id, project_id, working_dir, auto_approve_level, model_key)
SELECT ?, ?, project_id, working_dir, auto_approve_level, model_key FROM sessions WHERE id = ?
```

---

### 2. ModelResolver（新增）

新建 `crates/kernel/src/provider/resolver.rs`：

```rust
pub struct ModelResolver {
    session_id: SessionId,
    models: Arc<BTreeMap<String, ModelConfig>>,
    session_store: Arc<dyn SessionStore>,
    default_model: String,
    cached_providers: tokio::sync::Mutex<HashMap<String, Arc<dyn Provider>>>,
}

impl ModelResolver {
    pub async fn resolve(&self) -> Result<(Arc<dyn Provider>, Arc<ModelConfig>), ModelError> {
        let key = self.session_store
            .get(&self.session_id).await?
            .and_then(|i| i.model_key)
            .unwrap_or_else(|| self.default_model.clone());
        
        let model_config = self.models
            .get(&key)
            .cloned()
            .unwrap_or_else(|| self.models.values().next().cloned().unwrap_or_default());
        
        let provider = {
            let mut cache = self.cached_providers.lock().await;
            cache.entry(key.clone()).or_insert_with(|| {
                match create_provider_for_model(&model_config) {
                    Ok(p) => p,
                    Err(e) => {
                        tracing::error!("Failed to create provider for '{}': {}", key, e);
                        Arc::new(NoKeyProvider)
                    }
                }
            }).clone()
        };
        
        Ok((provider, Arc::new(model_config)))
    }
}
```

Provider 缓存：按 `model_key` 缓存 provider 实例，避免每次创建新 HTTP client。切换 model 时自动创建新 provider。

---

### 3. AgentShared 改造

**删除字段**：
- `provider: Arc<dyn Provider>`
- `model_config: Arc<ModelConfig>`
- `model_registry: Arc<BTreeMap<...>>`（已被 `ModelResolver` 取代）

**新增字段**：
- `model_resolver: Arc<ModelResolver>`

**方法**：
```rust
impl AgentShared {
    pub async fn resolve_model(&self) -> Result<(Arc<dyn Provider>, Arc<ModelConfig>), AgentError> {
        self.model_resolver.resolve().await
            .map_err(|e| AgentError::Other(format!("Model resolution failed: {e}")))
    }
}
```

---

### 4. Agent 层改造

**`agent.rs` 中每次 turn 解析**：

在 `execute_iteration` 或 `handle_input` 的 `Message` 分支中：

```rust
async fn handle_one_turn(&mut self) -> Result<(), AgentError> {
    // 1. 每次 turn 开始前动态解析 model
    let (provider, model_config) = self.shared.resolve_model().await?;
    
    // 2. 可能 compact
    if self.maybe_compact_messages(&model_config).await {
        tracing::info!("compacted before streaming");
    }
    
    // 3. Stream（用解析到的 provider 和 model_config）
    let stream = provider.stream(..., &model_config).await?;
    ...
}
```

所有原来使用 `self.shared.provider` / `self.shared.model_config` 的地方改为局部变量 `provider` / `model_config`：
- `stream()` 调用
- `compactor.auto_compact()` 调用
- `compactor.should_compact()` 调用（传 `model_config.context_window`）
- `record_compactor_token_usage()` 中记录 model_id / provider
- `TokenUsage` 事件中 emit `context_window`

**`AgentInput` 删除**：`SwitchModel` 变体（不再需要，切换 model 只更新数据库，下次 turn 自动生效）

---

### 5. Conductor 层改造

**删除**：
- `session_models: DashMap<SessionId, String>`（不再用内存 map）
- `models: Arc<BTreeMap<...>>`（移入 `ModelResolver`，由 `AgentShared` 持有）

**`wake_agent` 简化**：
- 不再需要在 `wake_agent` 中读取 model、创建 provider、注入 `AgentShared`
- `AgentShared` 已经包含 `model_resolver`，agent 每次 turn 自己 resolve
- `wake_agent` 只负责：读取历史、创建 mailbox、spawn agent

---

### 6. Kernel 层改造

**删除**：
- `session_models: DashMap<SessionId, String>`

**`create_session`**：
- 接收 `model_key: Option<String>` 参数
- 默认 `model_key.unwrap_or(agent_config.default_model)`
- 调用 `session_store.create(..., model_key.as_deref())`

**`set_session_model`**：
- 直接 `session_store.update_model_key(session_id, key)`
- 不再发 `SwitchModel` input
- 如果 agent 正在运行，下次 turn 自动生效

**`get_session_model`**：
- 从 `session_store.get(session_id)` 读取 `model_key`
- 未设置则返回 `default_model`

**`fork_session`**：
- 从父 session 读取 `model_key`，复制到子 session（`session_store.fork` 已支持）

---

### 7. 前端改造

**`create_session` API**：
- 增加 `model_key?: string` 参数
- 首页 ModelSelector 选中的 model 在创建时传递

**`ModelSelector`**：
- 切换时调用 `setSessionModel(session_id, key)`
- 不需要额外通知，InfoBar 在下次 render 时读取

**`InfoBar`**：
- 从 `session_info` 获取 `model_key`（如果 `get_session` 返回的话）
- 或直接用 `get_session_model` API

---

### 8. 数据流

```
创建 session:
  Frontend → create_session(model_key="gpt4o")
    → Kernel::create_session
      → session_store.create(model_key="gpt4o")

切换 model:
  Frontend → set_session_model("claude3.5")
    → Kernel::set_session_model
      → session_store.update_model_key("claude3.5")

LLM 请求:
  Frontend → send_message
    → Conductor::handle_input → wake_agent
      → Agent::start_loop → handle_one_turn
        → self.shared.resolve_model()
          → session_store.get(sid) → model_key
            → model_registry.get(key) → ModelConfig
              → create_provider_for_model → provider.stream()

Subagent:
  Parent session model_key = "gpt4o"
  → fork_session
    → session_store.fork 复制 model_key
  → Subagent wake_agent
    → Subagent::resolve_model() → 读取到 "gpt4o"
```

---

### 执行步骤

1. Migration + `SessionStore` trait + `sqlite.rs` 实现
2. `SessionInfo` 加 `model_key`，`fork` 继承
3. 新建 `ModelResolver`
4. `AgentShared` 改造（删除 provider/model_config/model_registry，加 model_resolver）
5. `agent.rs` 改造（每次 turn resolve_model，删除 SwitchModel）
6. `Conductor` 改造（删除 session_models/models，wake_agent 简化）
7. `Kernel` 改造（删除 session_models，create_session/set_session_model/get_session_model 走数据库）
8. 前端 API + 组件适配
