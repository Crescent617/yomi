# Event Bus 重构执行计划：Subagent 实时观测

> **对应设计文档**: `docs/design/event-bus-subagent-observability.md` (v2)  
> **总工期预估**: 2.5 周（含 review 与联调）  
> **负责人**: @hrli  
> **执行日期**: 2026-07-05

---

## 执行原则

1. **每 Phase 必须独立可编译、可测试**。不可出现跨 Phase 的无法编译的中间状态。
2. **先改 kernel，再改 TUI**。kernel 侧的改动对其他 consumer（gui、channel_hub）有 breaking change，需同步通知。
3. **Phase 1 的 Event 结构体是最大 breaking change**。该 Phase 必须一次性完成全项目的 `Event` → `EventPayload` 迁移，避免半吊子状态。
4. **回退策略**：每个 Phase 完成后打 `git tag`，若后续 Phase 阻塞可独立回退。

---

## Phase 1: PubSub 过滤 + Event 结构体

**目标**: 完成 kernel 侧基础设施重构，使 Event 具备自描述能力，Internal 事件对外部 subscriber 不可见。  
**工期**: 3-4 天  
**阻塞后续**: 是（Phase 2/3 依赖此 Phase）

### 1.1 PubSub 增加 listener filter 机制

**文件范围**:
- `crates/kernel/src/comms/bus.rs`
- `crates/kernel/src/comms/mod.rs`（如有 re-export 调整）

**任务清单**:
1. `Listener` 结构体增加 `filter: Arc<dyn Fn(&T) -> bool + Send + Sync>` 字段。
2. `Command::SubscribeSession` 和 `Command::SubscribeGlobal` 增加 `filter` 参数。
3. `run_forwarder` 中的 `try_send_to_listeners` 在发送前应用 `filter`：若 `!filter(ev)` 则跳过该 listener。
4. `PubSub::subscribe` 增加默认参数：`|_| true`（接收全部）。
5. 新增 `PubSub::subscribe_filtered(key, filter)` 方法，支持自定义过滤。

**验收标准**:
- `cargo build` 通过。
- 新增单元测试：`test_filter_excludes_events` 验证 filter 生效后 subscriber 收不到被排除的事件。
- 新增单元测试：`test_filter_does_not_affect_other_listeners` 验证同一事件下不同 filter 的 listener 行为独立。
- 现有 `PubSub` 测试（如有）全部通过。

**风险回退**:
- 若 `Arc<dyn Fn>` 引入显著性能损耗（如 `forwarder` 每帧分配），回退为 `fn(&T) -> bool` 函数指针（限制闭包捕获），但会牺牲灵活性。预计性能影响可忽略（filter 是纯比较逻辑）。

---

### 1.2 Event 重构为结构体 + EventPayload

**文件范围**:
- `crates/kernel/src/types.rs`（新增 `EventId` 类型）
- `crates/kernel/src/event.rs`（完全重写）
- `crates/kernel/src/lib.rs`（如有类型 re-export）
- `crates/kernel/src/wire.rs`（序列化格式调整）
- `crates/kernel/src/server/mod.rs`（序列化调整）
- `crates/kernel/src/client/mod.rs`（序列化调整）
- `crates/tui/src/msg.rs`（如有事件类型引用）
- `crates/gui/src`（如有 IPC 消费事件）

**任务清单**:
1. `types.rs` 中新增 `define_id!(EventId => "evt_");`。
2. `event.rs` 中：
   - 将现有 `Event` 枚举重命名为 `EventPayload`。
   - 新建 `Event` 结构体：`{ event_id: EventId, session_id: SessionId, timestamp_ms: u64, payload: EventPayload }`。
   - 为 `Event` 实现 `Default` 或 builder，方便测试构造。
3. `wire.rs` 中调整 `Event` 的序列化/反序列化：
   - 发送端：`Event` 结构体序列化为 JSON（含 `event_id`/`session_id`/`timestamp_ms`/`payload`）。
   - 接收端：反序列化为 `Event` 结构体。
4. 同步检查 `server/mod.rs` 和 `client/mod.rs` 中的 IPC 处理代码，确保使用新结构体。

**验收标准**:
- `cargo build` 通过。
- `cargo test -p kernel` 通过（所有与事件相关的测试）。
- 手动验证：启动 daemon + TUI，发送一条消息，TUI 正常显示（验证 IPC 序列化未断裂）。
- 序列化后的 JSON 示例符合设计文档格式（顶层有 `event_id`/`session_id`/`timestamp_ms`/`payload`）。

**风险回退**:
- 若 GUI 等外部 consumer 无法同步升级，可临时在 `wire.rs` 中增加兼容层：发送时同时附旧格式（`payload` 的扁平化版本），标记 `#[deprecated]` 并在 Phase 4 移除。但建议强制同步升级，避免技术债。

---

### 1.3 PubSubHandle 自动注入元数据

**文件范围**:
- `crates/kernel/src/comms/bus.rs`
- `crates/kernel/src/event.rs`（可能需要 `Event` 的 `new` 构造函数）

**任务清单**:
1. `PubSubHandle::send` 和 `try_send` 内部修改：
   - 如果 `T` 是 `Event` 类型，自动填充 `event_id = EventId::new()`、`session_id = self.key.clone()`、`timestamp_ms = current_time()`。
   - 如果传入的 `Event` 已经包含这些字段（非零值），则保留原值（允许调用方显式覆盖）。
2. 确保 `PubSubHandle` 的 `key` 类型（`K`）与 `Event::session_id` 类型匹配，或者增加 `K: Into<SessionId>` 约束。

**验收标准**:
- `cargo build` 通过。
- 单元测试：`test_auto_inject_metadata` 验证发送后 `event_id` 和 `session_id` 被正确填充。
- 单元测试：`test_preserve_explicit_metadata` 验证调用方显式设置时不会被覆盖。
- 手动验证：TUI 启动后，事件日志中的 `event_id` 和 `timestamp_ms` 非空。

**风险回退**:
- 若 `PubSubHandle` 的泛型约束导致编译错误（如 `K` 不总是 `SessionId`），可将自动注入逻辑下沉到 `EventBusHandle` 类型别名层，而非泛型 `PubSubHandle`。

---

### 1.4 全项目迁移 `event_bus.send(Event::...)` → `event_bus.send(EventPayload::...)`

**文件范围**:
- `crates/kernel/src/agent/agent.rs`
- `crates/kernel/src/agent/turn.rs`
- `crates/kernel/src/tools/*.rs`（所有发事件的 tool）
- `crates/kernel/src/app/conductor.rs`
- `crates/kernel/src/channels/hub.rs`
- `crates/kernel/src/permissions/checker.rs`
- `crates/kernel/src/cron/worker.rs`
- 其他包含 `event_bus.send` 或 `event_bus.try_send` 的文件

**任务清单**:
1. 全局搜索 `event_bus\.((try_)?send)` 调用点。
2. 将 `Event::User(...)` → `EventPayload::User(...)`，其余变体同理。
3. 将 `Event::Internal(...)` → `EventPayload::Internal(...)`（这些调用点只存在于内部 consumer）。
4. 检查 `Conductor` 的 subscriber 逻辑：从 `subscribe_all` 改为 `subscribe`（接收全部，含 Internal），或者保持 `subscribe_all` 但确认其 filter 行为。

**验收标准**:
- `cargo build` 通过。
- `cargo clippy --all-targets --all-features` 通过（无废弃 `Event` 变体使用）。
- `cargo test -p kernel` 全部通过。
- 手动验证：daemon + TUI 端到端运行正常，聊天、工具调用、streaming 无异常。

**风险回退**:
- 若迁移过程中漏改某些调用点（如 `event_bus.send(Event::System(...))` 漏成 `Event::System`），编译器会报错（`Event` 不再是枚举，无法直接构造）。编译器即是防线，无静默失败风险。

---

### 1.5 Conductor 订阅方式调整

**文件范围**:
- `crates/kernel/src/app/conductor.rs`

**任务清单**:
1. 确认 `Conductor` 作为内部 consumer，需要接收全部事件（含 `Internal`）。
2. 如果 `Conductor` 之前使用 `subscribe_all`，检查其是否仍工作（`subscribe_all` 返回全部事件，filter 不影响）。
3. 如果 `Conductor` 需要创建 `EventBusHandle` 向外部发事件，确保发送的是 `EventPayload`。

**验收标准**:
- `cargo build` 通过。
- `cargo test -p kernel` 通过。
- 手动验证：session 恢复、消息持久化、compaction 等功能正常。

---

### 1.6 单元测试

**文件范围**:
- `crates/kernel/src/comms/bus.rs`（新增测试）
- `crates/kernel/src/event.rs`（新增测试）
- `crates/kernel/src/comms/tests.rs`（如有）

**任务清单**:
1. `test_external_subscriber_does_not_receive_internal_events`：外部 subscriber（filter 排除 Internal）确认收不到 `EventPayload::Internal`。
2. `test_internal_subscriber_receives_all_events`：内部 subscriber（filter 全部接收）确认能收到 `Internal`。
3. `test_event_metadata_auto_populated`：验证 `PubSubHandle::send` 自动注入 `event_id`/`session_id`/`timestamp_ms`。
4. `test_event_serde_roundtrip`：验证 `Event` 结构体的 JSON 序列化和反序列化正确。

**验收标准**:
- `cargo test -p kernel` 全部通过，新增测试覆盖率 >= 80%。

---

## Phase 2: Subagent 事件关联

**目标**: 在 subagent 启动时注入 `parent_tool_id`，使 TUI 能建立 subagent session → parent tool 的关联。  
**工期**: 半天  
**依赖**: Phase 1 完成（`Event` 结构体已稳定）  
**阻塞后续**: 是（Phase 3 依赖此 Phase）

### 2.1 subagent.rs 增加 `parent_tool_id`

**文件范围**:
- `crates/kernel/src/tools/subagent.rs`

**任务清单**:
1. 在 `run_subagent` 方法参数中增加 `parent_tool_id: &str`（或从 `_ctx.tool_call_id` 提取）。
2. 在 `ToolEvent::Metadata` 的 `metadata` HashMap 中增加 `"parent_tool_id"` 键，值为 `_ctx.tool_call_id`。
3. 确保 `parent_tool_id` 在 `exec` 方法中可获取（`ToolExecCtx` 已包含 `tool_call_id`）。

**验收标准**:
- `cargo build` 通过。
- 手动验证：触发一次 subagent 调用，检查 TUI 日志中 `ToolEvent::Metadata` 的 `parent_tool_id` 字段非空且与 `Agent` 工具的 `tool_call_id` 一致。
- `cargo test -p kernel` 通过（`subagent.rs` 的现有测试不受影响）。

**风险回退**:
- 无显著风险。若 `parent_tool_id` 注入错误，TUI 在 Phase 3 中无法关联 subagent 事件，但不会影响 kernel 功能。

---

## Phase 3: TUI 动态订阅与渲染

**目标**: TUI 能动态订阅 subagent session，实时显示其内部进度。  
**工期**: 1.5-2 周（含联调）  
**依赖**: Phase 1 + Phase 2 完成

### 3.1 EventPump 重构为订阅池

**文件范围**:
- `crates/tui/src/app/event_pump.rs`
- `crates/tui/src/app/events.rs`（subscriber 映射定义）
- `crates/tui/src/app/model.rs`（`EventPump` 创建参数）

**任务清单**:
1. `EventPump` 内部增加：
   - `subagent_sessions: Arc<Mutex<HashMap<SessionId, String>>>`（subagent session → parent_tool_id）。
   - `subscribers: Arc<Mutex<HashMap<SessionId, CancelToken>>>`（跟踪活跃 subscriber）。
2. 主 subscriber 循环中，解析 `ToolEvent::Metadata`：
   - 提取 `subagent_session_id` 和 `parent_tool_id`。
   - 存入 `subagent_sessions`。
   - 调用 `coordinator.subscribe_session_events(subagent_session_id)` 创建新 subscriber。
   - 新 subscriber 在独立 task 中运行，使用 `CancelToken` 控制生命周期。
3. 新 subscriber 的 `recv` 循环：
   - 收到事件 → 包装为 `TaggedEvent::Subagent { parent_tool_id, event }` → 通过 `mpsc::Sender` 发给 TUI。
   - 收到 `AgentEvent::Lifecycle(Stopped { .. })` 或 `ConnectionLost` → 退出循环，从 `subscribers` 中移除，从 `subagent_sessions` 中移除。
4. 将 `mpsc::Receiver<Event>` 改为 `mpsc::Receiver<TaggedEvent>`。
5. 定义 `TaggedEvent` 枚举：
   ```rust
   pub enum TaggedEvent {
       Main(Event),
       Subagent { parent_tool_id: String, event: Event },
   }
   ```

**验收标准**:
- `cargo build` 通过。
- 手动验证：启动 subagent 后，TUI 日志中出现 `Subagent` subscriber 的创建和销毁记录。
- 并发测试：同时启动 2-3 个 async subagent，验证 EventPump 同时持有多个 subscriber，且各自独立退出。

**风险回退**:
- 若 `coordinator.subscribe_session_events` 在 subagent session 不存在时返回错误，EventPump 直接丢弃（约束 C2）。如果大量 subagent 同时失败导致日志刷屏，可增加日志采样。

---

### 3.2 `process_kernel_event` 处理 `TaggedEvent`

**文件范围**:
- `crates/tui/src/app/events.rs`

**任务清单**:
1. 将 `process_kernel_event` 的输入从 `Event` 改为 `TaggedEvent`。
2. 主循环中匹配 `TaggedEvent`：
   - `TaggedEvent::Main(event)` → 现有处理逻辑不变。
   - `TaggedEvent::Subagent { parent_tool_id, event }` → 新增分支：
     - 匹配 `event.payload` 的各种变体（`ModelEvent::Chunk`、`ToolEvent::Start`/`End`、`AgentEvent::Lifecycle` 等）。
     - 将事件转发给 `ChatView` 的 `update_subagent` 方法。
3. 删除现有 `_ => {}` 中对 `Event::Internal` 的忽略逻辑（Phase 1 后已无需）。

**验收标准**:
- `cargo build` 通过。
- 手动验证：subagent 执行时，TUI 日志中显示 `Subagent` 事件被正确分类和路由。
- `cargo clippy` 通过。

**风险回退**:
- 若 `TaggedEvent::Subagent` 的事件处理逻辑有 bug（如漏处理 `ModelEvent::Error`），subagent 会静默失败。需确保所有 `ModelEvent`/`AgentEvent`/`ToolEvent` 变体都有对应的处理分支。

---

### 3.3 `ChatView` 增加 `SubagentState`

**文件范围**:
- `crates/tui/src/components/chat_view/core.rs`
- `crates/tui/src/components/chat_view/mod.rs`（如有 re-export）

**任务清单**:
1. 定义 `SubagentState` 结构体：
   ```rust
   pub struct SubagentState {
       pub session_id: SessionId,
       pub description: String,
       pub status: SubagentStatus,
       pub events: Vec<Event>,
       pub folded: bool,
   }
   ```
2. `HistoryMessage::Tool` 增加 `subagent: Option<SubagentState>` 字段。
3. 在 `ChatView` 中新增方法：
   - `update_subagent(&mut self, parent_tool_id: &str, event: Event)`：根据事件更新对应 `HistoryMessage::Tool` 的 `subagent` 状态。
   - `init_subagent(&mut self, parent_tool_id: &str, session_id: SessionId, description: String)`：在 `ToolEvent::Metadata` 时初始化 `SubagentState`。
   - `finalize_subagent(&mut self, parent_tool_id: &str, status: SubagentStatus)`：在 `Lifecycle(Stopped)` 时标记最终状态。
4. 修改 `HistoryMessage::Tool` 的 `Debug` 输出（如需）。

**验收标准**:
- `cargo build` 通过。
- 手动验证：触发 subagent 后，`ChatView` 的 `messages` 中对应 `Tool` 的 `subagent` 字段为 `Some(...)`。
- subagent 结束时，`subagent.status` 正确变为 `Completed`/`Failed`/`Cancelled`。

**风险回退**:
- 若 `parent_tool_id` 找不到对应的 `HistoryMessage::Tool`（如事件顺序错乱），事件被丢弃并记录警告。这种情况不应发生，但需防御性编程。

---

### 3.4 渲染逻辑

**文件范围**:
- `crates/tui/src/components/chat_view/message_renderer.rs`
- `crates/tui/src/components/chat_view/core.rs`

**任务清单**:
1. `render_tool` 中，当 `subagent` 为 `Some` 时：
   - 折叠状态：在 tool header 下方增加一行摘要（如 `󰔟 Running… (grep · read · 2 tools done)`）。
   - 展开状态：在 tool 输出下方渲染内嵌面板，使用 `┊` 或 `│` 作为左侧引导线。
2. 复用现有的渲染逻辑：
   - subagent 的 `ModelEvent::Chunk::Text` 用 `StreamingMarkdownRenderer` 渲染。
   - subagent 的 `ToolEvent::Start`/`End` 复用 `render_tool` 逻辑，但宽度减 4，前缀加引导线。
   - subagent 的 `AgentEvent::Lifecycle` 显示状态图标。
3. 颜色降级：subagent 内部事件的颜色比主会话降一级（thinking 用更灰的灰，tool 用更暗的绿/红）。
4. 在 `ChatView` 的 `attr` 处理中增加 `SUBAGENT_EVENT` 命令，接收序列化的 `TaggedEvent::Subagent` 并更新状态。

**验收标准**:
- `cargo build` 通过。
- 手动验证：subagent 执行时，TUI 中 `Agent` tool 卡片下方显示实时进度（工具调用列表、streaming 文本）。
- 展开/折叠交互正常（点击 header 或按 `e`）。
- `Ctrl-O` (expand all) 同时展开所有 subagent 视图。
- 多 subagent 并发时，各自视图独立更新，不互相干扰。

**风险回退**:
- 若 subagent 的事件量过大（如高频 streaming）导致 TUI 卡顿，可在 `EventPump` 的 filter 中增加 `throttle` 或 `sample` 机制（如每 100ms 最多一个 `ModelEvent::Chunk`）。这是后续优化，不在当前 Phase 中实现。

---

### 3.5 联调测试

**任务清单**:
1. **单 subagent (sync 模式)**：触发一个 subagent，验证从启动到完成的完整流程：
   - Metadata → 订阅创建 → 事件流 → 实时渲染 → Stopped → 订阅销毁 → ToolEvent::End。
2. **多 subagent (async 模式)**：同时触发 2-3 个 async subagent，验证并发渲染和独立生命周期。
3. **subagent 失败场景**：构造一个会失败的 subagent（如 invalid prompt），验证 `Failed` 状态正确显示。
4. **daemon 重启场景**：subagent 执行中重启 daemon，验证 TUI 重连后不再尝试恢复已消失的 subagent subscriber（约束 C2）。
5. **长运行 subagent**：运行一个耗时 > 30s 的 subagent，验证 TUI 不卡顿、内存不泄漏。

**验收标准**:
- 上述 5 个场景全部通过。
- `cargo test -p tui` 通过（如有 TUI 测试）。
- 内存检查：使用 `valgrind` 或 `heaptrack` 检查 subscriber 退出后无内存泄漏。

---

## Phase 4: 清理

**目标**: 移除临时兼容代码和防御性分支，更新文档。  
**工期**: 半天  
**依赖**: Phase 3 完成并稳定运行

### 4.1 删除防御性 Internal 分支

**文件范围**:
- `crates/tui/src/app/events.rs`
- `crates/gui/src`（如有对 `Event::Internal` 的忽略逻辑）
- `crates/kernel/src/channels/hub.rs`（如有）
- 其他 consumer 代码

**任务清单**:
1. 全局搜索 `EventPayload::Internal` 或 `Event::Internal` 的使用点。
2. 在 TUI 中删除 `_ => {}` 里针对 `Internal` 的注释和忽略逻辑。
3. 在 ChannelHub 等外部 consumer 中确认 `Internal` 事件已不会被收到（filter 已生效），删除防御性代码。
4. 检查 `gui` crate 的 IPC 消费代码，确认 `Event` 结构体解析正确，无遗留的扁平化格式假设。

**验收标准**:
- `cargo build` 通过（所有 crate）。
- `cargo clippy` 通过（无 `Internal` 相关的 dead_code 或 unused 警告）。
- 手动验证：TUI 运行 10 分钟，无 `Internal` 事件泄露到日志。

---

### 4.2 文档更新

**文件范围**:
- `docs/design/event-bus-subagent-observability.md`（更新为 Final 状态）
- `README.md` 或 `CHANGELOG.md`（如有，记录 breaking change）
- 代码注释更新（`PubSub`、`EventBus` 等关键模块的 rustdoc）

**任务清单**:
1. 设计文档中标记所有决策为 Final，补充实际实现中的偏差（如有）。
2. 记录 `Event` 结构体的 JSON 序列化格式作为 API 契约。
3. 在 `EventBus` 和 `PubSub` 的 rustdoc 中说明 filter 机制和默认行为。
4. 如有 GUI 等外部 consumer，同步通知其升级事件解析逻辑。

**验收标准**:
- 文档与代码一致，无矛盾。
- `cargo doc` 生成无警告。

---

## 全局风险与应对

| 风险 | 影响 | 概率 | 应对措施 |
|------|------|------|---------|
| IPC 序列化 breaking change 导致 GUI 不可用 | 高 | 中 | Phase 1 完成后立即通知 GUI 维护者同步升级。若无法同步，临时保留兼容层（发送双格式） |
| `EventPump` 多 subscriber 导致 TUI 内存泄漏 | 中 | 低 | 每个 subscriber 绑定 `CancelToken` 和 `Drop` 自动 unsubscribe。Phase 3 联调中做内存检查 |
| subagent 高频事件导致 TUI 卡顿 | 中 | 中 | Phase 3 的 filter 机制可排除 `ToolCallDelta` 等噪音。后续可增加 throttle 优化 |
| `PubSub` filter 引入性能瓶颈 | 低 | 低 | filter 是纯比较逻辑，无 IO。若实测发现瓶颈，可优化为 bitmap 或 enum flag 过滤（替代闭包） |
| 跨 crate 的 `Event` 类型引用导致编译循环依赖 | 低 | 低 | `Event` 定义在 `kernel` crate，`tui` 和 `gui` 通过依赖引用，无循环风险 |

---

## 里程碑与 Checklist

- [ ] **Phase 1.1**: PubSub filter 机制 + 单元测试通过
- [ ] **Phase 1.2**: Event 结构体 + 序列化调整 + 全项目编译通过
- [ ] **Phase 1.3**: PubSubHandle 自动注入元数据 + 单元测试通过
- [ ] **Phase 1.4**: 全项目 `Event` → `EventPayload` 迁移完成
- [ ] **Phase 1.5**: Conductor 订阅调整 + 端到端测试通过
- [ ] **Phase 1.6**: 新增单元测试全部通过，覆盖率达标
- [ ] **Phase 2.1**: `parent_tool_id` 注入 + 手动验证
- [ ] **Phase 3.1**: EventPump 订阅池重构 + 并发测试通过
- [ ] **Phase 3.2**: `process_kernel_event` 处理 `TaggedEvent` + 编译通过
- [ ] **Phase 3.3**: `ChatView` `SubagentState` 数据结构 + 状态更新逻辑
- [ ] **Phase 3.4**: 渲染逻辑 + 交互（展开/折叠）
- [ ] **Phase 3.5**: 5 个联调场景全部通过
- [ ] **Phase 4.1**: 删除 `Internal` 防御性分支 + 全项目编译通过
- [ ] **Phase 4.2**: 文档更新 + rustdoc 无警告
- [ ] **最终**: 合并到主分支，打 tag `subagent-observability-v1`
