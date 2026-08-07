# 设计文档：TUI `/models` 模型选择器

## 背景

内核已完成多模型支持（见 `multi-model-config.md`、`session_model_db.md`）：

- `KernelApi` 已暴露 `list_models()` / `get_session_model()` / `set_session_model()`，本地嵌入模式与 daemon RPC 模式均已实现（`crates/kernel/src/client/mod.rs`）。
- Model key 持久化在 `sessions.model_key` 列（migration v13），agent 每轮 turn 动态解析，切换后**下一轮 streaming 立即生效**，无需重启 agent。
- GUI 已有 model selector；TUI 目前只能在启动时读取，运行中无法切换。

TUI 已有成熟的 `FuzzyPicker` 复用模式（`/sessions`、`/rewind`、Ctrl-R 历史搜索），本功能完全复用该模式，不引入新组件类型。

## 目标

- 输入 `/models` 弹出模糊搜索选择器，列出 config 中所有模型，标记当前模型。
- 选中后调用 `set_session_model` 持久化，即刻更新状态栏/banner 的模型名与 context window 显示。
- 支持 `/models <key>` 直接切换（免弹窗）。

## 非目标

- 不改内核任何接口（已齐备）。
- 不支持切换全局默认模型（那是 `yomi config set` 的职责）。
- 不中断当前正在进行的 stream（内核语义是下一轮生效，与 GUI 一致）。

## 方案

### 1. 消息定义（`crates/tui/src/msg.rs`）

```rust
pub enum Msg {
    // ...
    CommandModels(Option<String>), // /models 或 /models <key>
    ModelList(Vec<PickerItem>),    // 异步取回的模型列表（已含当前标记）
    ModelSelected(String),         // 用户选中 model key
    CloseModelPicker,
    // set_session_model 成功后的 UI 更新载荷
    ModelSwitched {
        key: String,
        model_id: String,
        context_window: u32,
    },
}
```

说明：与 `SessionList` 同样，在异步任务中把 `ModelInfo` 转成 `PickerItem`，避免 `Msg` 对 kernel 类型的 `PartialEq/Eq` 约束扩散。

### 2. 组件挂载

- `id.rs`：新增 `Id::ModelPicker`。
- `view.rs`：加入 `OVERLAY_COMPONENTS`。
- `init.rs`：仿照 session picker 挂载：

```rust
let model_picker = FuzzyPickerComponent::new(
    PickerConfig::new("Switch Model").with_placeholder("Search models..."),
)
.with_callbacks(Msg::ModelSelected, || Msg::CloseModelPicker);
```

### 3. 命令入口（`components/input/`）

- `mod.rs::SLASH_COMMANDS` 增加 `("/models", "Switch model for this session")`（自动进入补全列表与 /help）。
- `handlers.rs::parse_command`：

```rust
"/models" | "/model" => {
    let key = parts.get(1).map(|s| (*s).to_string());
    Some(Msg::CommandModels(key))
}
```

### 4. update 逻辑（`app/update.rs`）

**`Msg::CommandModels(key)`**：

- `key = Some(k)`：跳过弹窗，直接走切换路径（等价于 `ModelSelected(k)`）。
- `key = None`：spawn 异步任务，并发调用 `kernel.list_models()` 与 `kernel.get_session_model(&sid)`，构造 `PickerItem`：
  - `id` = model key（`ModelInfo.name`）
  - `label` = `"● {name}"`（当前模型）或 `"  {name}"`
  - `meta` = `"{provider} · {model_id} · {context_window/1000}k ctx"`
  - 当前模型排在首位，其余保持 `list_models` 的 BTreeMap 有序。
  - 完成后 `tx.send(Msg::ModelList(items))`。

**`Msg::ModelList(items)`**：与 `SessionList` 完全一致 —— 设置 `PICKER_ITEMS`、`DIALOG_SHOW`、`set_focus(&Id::ModelPicker)`。若列表为空（配置异常），改发 warn Notification。

**`Msg::ModelSelected(key)`**：

1. 隐藏 picker（`DIALOG_HIDE`），焦点还给 `InputBox`。
2. 若 key 等于当前模型 → 只发 info Notification，直接返回。
3. spawn：`kernel.set_session_model(&sid, &key)`：
   - 成功 → 从本地 `crate::config().models` 查出 `model_id` / `context_window`，发送 `Msg::ModelSwitched { .. }`。若本地 config 查不到（远程 daemon 配置不同），`model_id` 退化为 key、`context_window` 为 0（沿用 `run.rs` 已有的容错思路）。
   - 失败 → 发 error Notification（如 `Model 'x' not found in config`）。

**`Msg::ModelSwitched { key, model_id, context_window }`**：

1. `self.model_name = model_id.clone()`；`context_window > 0` 时同步 `self.context_window`（`events.rs` 中 `TokenUsage` 事件本就会持续校准，这里是即时修正，消除注释中提到的 "model_name may go stale until restart" 问题）。
2. 更新 UI attr：`Id::StatusBar` 的 `SET_MODEL_NAME`、`Id::Banner` 的 `MODEL_NAME`。
3. 发送 success Notification：`Switched to {key}, takes effect next turn`。

**`Msg::CloseModelPicker`**：同 `CloseSessionPicker`。

### 5. 边界情况

| 场景 | 行为 |
|---|---|
| stream 进行中切换 | 允许；内核下一轮 turn 解析新 key，Notification 已说明 "next turn" |
| `/models unknown-key` | `set_session_model` 返回 Err，error Notification，不改 UI 状态 |
| daemon 模式且远端配置与本地不同 | 列表以远端 `list_models` 为准；本地查不到 context_window 时显示退化 |
| 只配置了一个模型 | 正常弹窗（仅一项），不做特判 |

## 测试

- `handlers` 单测（`input_edit_test.rs` 同级新增或扩展现有 parse 测试）：`/models`、`/models k2`、`/model k2` 的解析。
- picker item 构造纯函数抽出（`fn model_picker_items(models: &[ModelInfo], current: &str) -> Vec<PickerItem>`）单测：当前置顶、标记、meta 格式。
- 手工验证：切换后状态栏立即更新；发消息后 kernel 日志确认使用新 model_id；重启 TUI 后模型保持（落库验证）。

## 工作量

纯 TUI 层改动，约 6 个文件（msg / id / view / init / input / update），无内核改动。
