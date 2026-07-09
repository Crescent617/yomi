# 执行文档：多 Model 配置与 Session 级别运行时切换

> 基于设计文档 `docs/design/multi-model-config.md`，将变更拆分为可执行的步骤。按依赖顺序执行，每步完成后应能编译通过（`cargo build`）或至少能通过 `cargo check`。

---

## 阶段一：数据模型重构（无编译依赖，可独立完成）

### 1. `ModelConfig` 加 `name` + `context_window`

**文件**: `crates/kernel/src/provider/mod.rs`

**动作**:
- `ModelConfig` 增加 `pub name: String` 和 `pub context_window: u32`
- `Default` 实现中 `name = "default"`, `context_window = 131_072`
- 增加 `has_api_key()` 方法（如果尚未存在）

**编译验证**: `cargo check -p yomi-core` 应通过。

**潜在风险**: `ModelConfig` 的 `Default` 被多处使用，新增字段默认值不会破坏现有行为。

---

### 2. `Compactor` 移除 `context_window`，方法签名改参数传递

**文件**: `crates/kernel/src/compactor/mod.rs`

**动作**:
- 删除 `Compactor::context_window` 字段
- `Default::default()` 中去掉 `context_window`
- `new()` 参数去掉 `context_window`
- `threshold()` 增加 `context_window: u32` 参数
- `should_compact()` 增加 `context_window: u32` 参数
- `auto_compact()` 和 `full_compact()` 签名不变（它们已经接收 `model_config: &ModelConfig`，内部从 `model_config.context_window` 获取）
- 更新常量：`DEFAULT_CONTEXT_WINDOW` 从 `compactor/mod.rs` 移到 `provider/mod.rs`（或直接使用 `ModelConfig::default().context_window`）

**编译验证**: `cargo check -p yomi-core`（此时会报 `Agent` 中调用点错误，正常现象，阶段二修复）。

---

### 3. `Compactor` 测试文件更新

**文件**: `crates/kernel/src/compactor/tests.rs`

**动作**:
- 所有 `Compactor::new()` 调用去掉 `context_window` 参数
- `should_compact()` 调用增加 `context_window` 参数
- 新增测试：验证不同 `context_window` 下的 `threshold()` 计算正确

**编译验证**: `cargo test -p yomi-core -- compactor`（此时可能会因 Agent 调用点编译失败，先注释掉 Agent 相关测试）

---

## 阶段二：Agent 层适配（依赖阶段一）

### 4. `Agent` 适配所有 `compactor` 调用点

**文件**: `crates/kernel/src/agent/agent.rs`

**动作**:
- `maybe_compact_messages()`: `should_compact()` 调用增加 `self.shared.model_config.context_window` 参数
- `force_compact()`: `auto_compact()` 调用保持不变（因为它已经接收 `&self.shared.model_config`）
- `force_full_compact()`: `full_compact()` 调用保持不变
- `handle_streaming()` 中 `context_window` 获取：从 `self.shared.compactor.as_ref().map_or(...)` 改为 `self.shared.model_config.context_window`

**搜索关键词**: `compactor`、`should_compact`、`context_window`、`DEFAULT_CONTEXT_WINDOW`

**编译验证**: `cargo check -p yomi-core` 应通过。

---

### 5. `AgentConfig` 删除 `model` 字段，`default_model` 改为 `String`

**文件**: `crates/kernel/src/agent/types.rs`

**动作**:
- 删除 `pub model: ModelConfig` 字段
- `default_model` 从 `Option<String>` 改为 `String`
- `Default` 实现中：`default_model: "default".to_string()`，删除 `model: ModelConfig::default()`
- 删除所有依赖 `agent_config.model` 的代码（编译时会报错，逐个修复）

**编译验证**: `cargo check -p yomi-core`（会报大量编译错误，正常现象，阶段三修复）

---

### 6. `AgentShared` 增加 `model_registry`

**文件**: `crates/kernel/src/agent/types.rs`

**动作**:
- `AgentShared` 增加 `pub model_registry: Arc<BTreeMap<String, ModelConfig>>`
- `with_model_registry()` 方法增加到 `AgentShared`

**编译验证**: 此时编译会因 `AgentShared` 缺少字段的构造调用而报错，阶段三修复。

---

### 7. `AgentInput` 新增 `SwitchModel` 变体

**文件**: `crates/kernel/src/agent/agent.rs`

**动作**:
- `AgentInput` 枚举增加 `SwitchModel { model_key: String }` 变体
- `handle_input()` 中增加 `AgentInput::SwitchModel` 处理分支：
  - 从 `self.shared.model_registry` 查表获取 `model_config`
  - 调用 `create_provider_for_model()` 创建 provider
  - `Arc::make_mut(&mut self.shared)` 更新 `provider` 和 `model_config`

**编译验证**: `cargo check -p yomi-core`

---

## 阶段三：Config 层重构（依赖阶段二）

### 8. `Config` 删除 `agent.model` 兼容逻辑，增加 `models` 数组

**文件**: `crates/kernel/src/config/mod.rs`

**动作**:
- `Config` 结构体增加 `pub models: Vec<ModelConfig>`（有 `#[serde(default)]`）
- `Default` 实现中：`models: vec![ModelConfig::default()]`
- 删除 `Config::model()` 中的 backward compat 逻辑（不再需要 fallback 到 `agent.model`）
- `model()` 方法改为直接查 `models` 数组：
  ```rust
  pub fn model(&self) -> &ModelConfig {
      self.models.iter()
          .find(|m| m.name == self.agent.default_model)
          .expect("default_model must exist in models")
  }
  ```
- `finalize()` 简化：只保留 `models` 为空时的兜底（`push ModelConfig::default()`），以及 `default_model` 无效时的 fallback
- `load_from_env()` 改造：单模型 env 变量作用于 `self.models[0]`，新增 `YOMI_DEFAULT_MODEL` 覆盖 `agent.default_model`
- `env_names` 中删除 `CONTEXT_WINDOW`（已移到 `ModelConfig`），新增 `DEFAULT_MODEL`

**搜索关键词**: `agent.model`、`self.agent.model`、`context_window`（在 config 中的使用）

**编译验证**: `cargo check -p yomi-core`（会报 `config_test.rs` 和 `build_kernel` 等错误，阶段四修复）

---

### 9. `Config` 测试文件更新

**文件**: `crates/kernel/src/config/config_test.rs`

**动作**:
- 删除所有测试 `agent.model` 的断言
- 新增测试：`models` 数组序列化/反序列化
- 新增测试：`default_model` 指向不存在时 `finalize()` 的 fallback
- 新增测试：`load_from_env()` 中 `YOMI_DEFAULT_MODEL` 的覆盖
- 更新 `test_config_serialization_roundtrip`：验证 `models` 字段而非 `agent.model`

**编译验证**: `cargo test -p yomi-core -- config::`

---

## 阶段四：Kernel 构建层（依赖阶段三）

### 10. `build_kernel` 和 `build_agent_config` 适配

**文件**: `crates/kernel/src/lib.rs`

**动作**:
- `build_agent_config()`：删除从 `config.agent.model` 获取模型的逻辑，改为从 `config.models` 获取。`skill_folders` 等不变。
- `build_kernel()`：从 `config.model()` 获取默认模型创建 provider。`config.models` 整体传给 `Kernel::new()`。
- `create_provider_for_model()` 已在设计文档中定义，需要实际实现（或确保已存在）。
- `AgentConfig::model` 字段删除后，确保 `build_agent_config` 不再引用它。

**编译验证**: `cargo check -p yomi-core`

---

### 11. `Kernel` 改造

**文件**: `crates/kernel/src/kernel/mod.rs`

**动作**:
- `Kernel` 结构体增加：
  - `models: Arc<BTreeMap<String, ModelConfig>>`
  - `session_models: Arc<DashMap<SessionId, String>>`
- `Kernel::new()` 参数增加 `models: Vec<ModelConfig>`
- `new()` 中：
  - 构建 `models_map: Arc<BTreeMap<...>>`
  - 构建 `session_models: Arc<DashMap<...>>`
  - 构建 `AgentShared` 时注入 `model_registry`
  - 构建 `Conductor` 时传入 `models_map` 和 `session_models`
- 实现 `list_models()`、`get_session_model()`、`set_session_model()`、`create_session()` 改造
- 新增 `ModelInfo` 结构体

**编译验证**: `cargo check -p yomi-core`

---

### 12. `Conductor` 改造

**文件**: `crates/kernel/src/kernel/conductor.rs`

**动作**:
- `Conductor` 结构体增加：
  - `models: Arc<BTreeMap<String, ModelConfig>>`
  - `session_models: Arc<DashMap<SessionId, String>>`
- `Conductor::new()` 参数增加上述两个字段
- `wake_agent()` 中：
  - 从 `session_models` 获取 `model_key`
  - 从 `models` 查表获取 `model_config`
  - 创建 provider，注入到 `AgentShared` 克隆中
- 删除所有引用 `agent_config.model` 的代码（编译器会报错）

**编译验证**: `cargo check -p yomi-core`（这是 kernel 核心部分，编译通过意味着主体逻辑正确）

---

## 阶段五：API 与协议层（依赖阶段四）

### 13. `KernelApi` 扩展

**文件**: `crates/kernel/src/client/mod.rs`

**动作**:
- `KernelApi` trait 增加 `list_models()`、`get_session_model()`、`set_session_model()`
- `LocalKernel` 实现（`Kernel` 的 `impl KernelApi`）直接代理到 `Kernel` 方法
- `RemoteKernel` 实现：
  - `list_models()` -> `ReqMethod::ListModels`
  - `get_session_model()` -> `ReqMethod::GetSessionModel`
  - `set_session_model()` -> `ReqMethod::SetSessionModel`

**编译验证**: `cargo check -p yomi-core`

---

### 14. Wire Protocol 扩展

**文件**: `crates/kernel/src/wire/mod.rs`

**动作**:
- `ReqMethod` 增加：`ListModels`、`GetSessionModel { session_id: String }`、`SetSessionModel { session_id: String, key: String }`
- `WIRE_PROTOCOL_VERSION` 升级（`7 -> 8`）

**编译验证**: `cargo check -p yomi-core`

---

### 15. Server dispatch 处理

**文件**: `crates/kernel/src/server/dispatcher.rs`

**动作**:
- `dispatch_request()` 的 `match method` 中增加新 `ReqMethod` 的处理：
  - `ListModels` -> 调用 `kernel.list_models()`，序列化为 JSON
  - `GetSessionModel` -> `kernel.get_session_model()`
  - `SetSessionModel` -> `kernel.set_session_model()`

**编译验证**: `cargo check -p yomi-core`

---

### 16. `CreateSessionInput` 增加 `model_key`

**文件**: `crates/kernel/src/kernel/mod.rs`（已在阶段四处理，但需确保 `KernelApi` 的 `create_session` 调用链也支持）

**动作**:
- `CreateSessionInput` 增加 `pub model_key: Option<String>`
- 检查 `RemoteKernel::create_session` 的 `CreateSessionInput` 序列化/反序列化是否受影响（`model_key` 是 `Option`，默认 `None`，不影响已有调用）
- `CLI` 和 `TUI` 的 `create_session` 调用点可能需要更新（如果是显式构造 `CreateSessionInput`）

**编译验证**: `cargo check --workspace`（全工作区）

---

## 阶段六：GUI 层（依赖阶段五）

### 17. GUI 后端命令

**文件**: `crates/gui/src/commands/system.rs`

**动作**:
- 新增 `get_models()` Tauri 命令
- 新增 `get_session_model()` Tauri 命令
- 新增 `set_session_model()` Tauri 命令
- `commands/mod.rs` 中注册新命令

**编译验证**: `cargo check -p yomi-gui`

---

### 18. GUI 前端 model selector

**文件**: `crates/gui/frontend/src/lib/api.ts`（或现有 API 文件）

**动作**:
- 增加 `getModels()`、`getSessionModel()`、`setSessionModel()` 前端 API 调用
- 在 `state.svelte.ts` 中增加 `currentModel` 或相关状态管理（可选，取决于前端架构）
- 在 ChatHeader 或 Toolbar 中增加 `ModelSelector.svelte` 组件：
  - 从 `getModels()` 加载模型列表
  - 显示当前 session 的 model（从 `getSessionModel()` 获取）
  - 切换时调用 `setSessionModel()`
  - 绑定到 `currentSession` 变化时重新加载

**编译验证**: 前端构建（`npm run build` 或 `npm run check`）

---

## 阶段七：集成与测试

### 19. 全工作区编译

```bash
cargo build --workspace
cargo test --workspace
```

**预期**: 编译通过，测试通过（可能需要修复一些因 API 变更导致的测试失败）。

---

### 20. 端到端验证

按设计文档中的验证步骤执行：

1. 启动 daemon，加载含 `[[models]]` 的 `config.toml`
2. 验证 `list_models` 返回正确列表
3. 创建 session，验证默认 model 正确
4. GUI 中切换 model，验证内存生效（不写入 config.toml）
5. 验证不同 model 的 `context_window` 影响 compactor 阈值
6. 验证 `RemoteKernel` 通过 IPC 正确工作

---

## 关键决策点

| 步骤 | 决策 | 建议 |
|------|------|------|
| 阶段一第1步 | `ModelConfig::Default` 的 `name` 是否可能冲突？ | 不会，`Default` 只在 `Config::default()` 中使用一次，后续 `finalize()` 会确保唯一。 |
| 阶段二第5步 | 删除 `AgentConfig.model` 后，编译错误可能非常多 | 用 `cargo check` 逐个修复，优先修复 `lib.rs` 和 `kernel/mod.rs`，再修复 `conductor.rs`。 |
| 阶段三第8步 | `load_from_env` 中 `CONTEXT_WINDOW` 的 env 变量从 `compactor` 移到 `model` | 需要同时更新 `env_names` 中的注释，避免开发者困惑。 |
| 阶段四第11步 | `Kernel::new` 参数增加 `models` | 可能破坏 `cli` 或 `tui` 的 `Kernel` 直接调用，需检查。 |
| 阶段六第18步 | 前端 `ModelSelector` 绑定到哪个 session？ | 绑定到当前活跃 session（`currentSession`），切换 session 时重新加载 `getSessionModel()`。 |

---

## 回滚策略

- 每个阶段独立提交，阶段之间保持可编译状态（或至少 `cargo check` 通过）。
- 如果某阶段改动过大，可在阶段内再拆小提交（例如先改结构体，再改方法，最后改测试）。
- 设计文档中所有改动都在一个 feature 分支上，可随时回滚到任何提交点。

---

## 时间预估

| 阶段 | 预估时间 | 说明 |
|------|---------|------|
| 阶段一 | 1-2 小时 | 纯数据模型变更，相对机械 |
| 阶段二 | 2-3 小时 | Agent 层适配，需要理解 compactor 调用链 |
| 阶段三 | 2-3 小时 | Config 层重构，env 变量逻辑较复杂 |
| 阶段四 | 3-4 小时 | Kernel + Conductor 改造，是核心逻辑 |
| 阶段五 | 2-3 小时 | API + Wire Protocol，需要更新 server dispatch |
| 阶段六 | 2-3 小时 | GUI 前后端，前端 Svelte 组件需调试 |
| 阶段七 | 2-3 小时 | 集成测试和修复 |
| **总计** | **14-21 小时** | 视编译错误和测试失败数量而定 |

---

## 下一步

确认此执行文档后，开始按阶段推进。建议先完成阶段一（数据模型），提交后进入阶段二。
