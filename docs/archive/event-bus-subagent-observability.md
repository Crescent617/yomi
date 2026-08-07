# Event Bus 架构重构设计：支持 Subagent 实时观测（v2）

**Status:** Draft  
**Author:** @hrli  
**Date:** 2026-07-05

---

## 1. 背景与问题

当前 subagent 工具在 TUI 中是一个**黑箱**：用户只能看到 `Agent` 工具被调用 → 等待 → 返回最终结果。subagent 内部的所有 streaming chunk、thinking、工具调用、retry 等过程完全不可见。

根本原因是事件总线存在三个结构性缺陷：

1. **Event 缺乏自描述性**。`Event` 是纯枚举，不携带 `event_id` 和 `session_id`。当事件被序列化到 IPC 或持久化时，来源信息丢失，也无法做幂等去重。
2. **Internal 事件泄露到外部 subscriber**。`InternalEvent::MessageAdded`/`MessageReplaced` 被广播给 TUI 等外部 consumer，造成噪音和无效重绘。TUI 目前靠 `_ => {}` 忽略，属于防御性编程。
3. **订阅模型是静态的**。TUI 的 `EventPump` 启动时只订阅主 session，无法动态增删对 subagent session 的监听。

---

## 2. 设计约束（来自 review）

| # | 约束 | 含义 |
|---|------|------|
| C1 | `event_id` 用现有 `define_id!` 宏 | 复用 `EVT_PREFIX` + `Ulid` 的 ID 生成机制，不引入新 ID 类型 |
| C2 | 订阅失败不重试 | 动态订阅 subagent session 失败时（如 session 不存在），直接标记失败，不进入重试循环 |
| C3 | 不拆分 Internal Bus，发送时过滤 | 不创建第二个 `PubSub` 实例。在同一个 `EventBus` 上，通过 listener 级别的 filter 实现内部/外部隔离 |
| C4 | 订阅不做上限，但支持传 filter | 并发 subagent 数量不限。每个订阅可指定事件过滤规则（如排除 model delta），减少无效带宽 |

---

## 3. 核心设计

### 3.1 Event 自描述化：从枚举到结构体

将 `Event` 从裸枚举重构为**包装结构体**，所有事件统一携带全局唯一标识、来源 session 和时间戳。

```rust
pub struct Event {
    pub event_id: EventId,       // 通过 define_id!(EventId => "evt_") 生成
    pub session_id: SessionId,   // 事件来源 session（由 PubSubHandle 自动注入）
    pub timestamp_ms: u64,       // 发送时刻（unix epoch ms，由 PubSubHandle 自动注入）
    pub payload: EventPayload,   // 原 Event 枚举的内容
}

pub enum EventPayload {
    User(UserEvent),
    Agent(AgentEvent),
    Model(ModelEvent),
    Tool(ToolEvent),
    System(SystemEvent),
    Internal(InternalEvent),     // 保留，但外部 subscriber 默认被过滤掉
}
```

**为什么保留 `Internal` 变体？**

不拆分 `InternalBus`（约束 C3）。`InternalEvent` 仍然通过同一个 `EventBus` 发送，但只在内部 listener 之间流转。外部 subscriber（TUI、ChannelHub）的默认 filter 自动排除 `InternalEvent`，从根本上解决泄露问题。

**`session_id` 的注入**

`PubSubHandle` 在发送时自动将绑定的 `key` 填充到 `event.session_id`，Agent 无需手动设置。这保证了 `session_id` 的准确性和一致性，避免 Agent 误填。

**`event_id` 的用途**

- 去重：subscriber 重连后（如 daemon 重启后的 resubscribe）可依据 `event_id` 过滤已处理事件。
- 日志追踪：跨 session 的事件链可通过 `event_id` 关联。
- 幂等：TUI 重绘时可根据 `event_id` 判断是否为重复事件。

**兼容性**

IPC 序列化使用 JSON。`Event` 结构体的序列化格式为：

```json
{
  "event_id": "evt_01ABCDEF",
  "session_id": "sess_123456",
  "timestamp_ms": 1720000000000,
  "payload": { "type": "model", "chunk": { ... } }
}
```

与旧格式（直接是 `{"type": "model", ...}`）不同。server 和 client 同时升级，不保留旧格式兼容。如果 GUI 等外部 consumer 也消费该格式，需要同步升级。

---

### 3.2 发送时过滤：PubSub 的 listener filter 机制

不创建第二个 `PubSub` 实例。在同一个 `PubSub` 的 `forwarder` 中，为每个 listener 维护一个 `filter: Fn(&T) -> bool`。

```
Agent 发事件 ──► EventBus (PubSub)
                     │
                     ▼ (forwarder 分发)
            ┌────────────┬────────────┐
            │            │            │
            ▼            ▼            ▼
    TUI subscriber  Storage subscriber  ChannelHub subscriber
    (filter: 排除     (filter: 接收    (filter: 排除
     Internal)        全部)             Internal)
```

**为什么不是"接收后过滤"？**

如果 subscriber 收到全部事件再自己过滤，`mpsc` channel 的 `capacity` 会被 `InternalEvent` 占满，导致外部事件被丢弃。在 `forwarder` 发送前过滤，可以保护外部 subscriber 的 channel 带宽。

**默认 filter 策略**

- `PubSub::subscribe(key)` → 默认 `|_| true`（接收全部，用于内部 consumer）
- `EventBus::subscribe_external(key)` → 默认 `|ev| !matches!(ev.payload, EventPayload::Internal(_))`（用于 TUI 等外部 subscriber）
- `EventBus::subscribe_filtered(key, filter)` → 自定义 filter（用于约束 C4）

---

### 3.3 动态订阅：TUI EventPump 的订阅池

`EventPump` 从"单 subscriber 循环"重构为**订阅池管理器**。

**启动时**

`EventPump` 启动后，先订阅主 session（同现有行为）。

```rust
event_bus.subscribe_external(session_id)  // 主 session
```

**发现 subagent**

主 subscriber 收到 `ToolEvent::Metadata`（含 `subagent_session_id` + `parent_tool_id`）时，EventPump 启动一个新的 subscriber：

```rust
event_bus.subscribe_filtered(subagent_session_id, |ev| {
    !matches!(ev.payload, EventPayload::Internal(_))
})
```

**订阅失败处理**

如果 `coordinator.subscribe_session_events(subagent_session_id)` 返回失败（如 `session_not_found`），**不重试**（约束 C2）。EventPump 直接丢弃该 subagent 的订阅请求，由 `subagent.rs` 的 `run_subagent` 在超时后返回失败状态。

**生命周期自管理**

每个 subagent subscriber 在独立 task 中运行 `recv` 循环。当收到 `AgentEvent::Lifecycle(Stopped { .. })` 时，该 subscriber 自动退出，无需外部通知。

如果 subagent 崩溃（未发 `Stopped`），`subscriber.recv()` 会收到 `ConnectionLost` 或 channel close，同样触发退出。`PubSubSubscriber` 的 `Drop` 会自动发送 unsubscribe 命令，释放 `forwarder` 中的 listener 槽位。

**并发 subagent**

多个 async subagent 同时运行时，EventPump 同时持有多个 subscriber。数量不做上限（约束 C4），由系统资源自然限制。每个 subscriber 的 filter 可独立配置。

**Filter 在 subagent 订阅中的应用**

TUI 在创建 subagent subscriber 时，可通过 filter 控制粒度。例如：

- 排除高频 model delta（减少刷屏）：
  ```rust
  |ev| match &ev.payload {
      EventPayload::Internal(_) => false,
      EventPayload::Model(ModelEvent::ToolCallDelta { .. }) => false,
      _ => true,
  }
  ```

- 只关心工具调用和生命周期：
  ```rust
  |ev| matches!(&ev.payload,
      EventPayload::Tool(_) | EventPayload::Agent(AgentEvent::Lifecycle { .. })
  )
  ```

Filter 是 TUI 本地决定的，不需要在 `subscribe_session_events` 的 RPC 参数中传递（因为 TUI 的 subscriber 本身就是本地 filter，RPC 流仍传输全部事件，由 TUI 应用 filter）。

---

### 3.4 TUI 消息关联：parent_tool_id 追踪

TUI 需要知道 subagent 的事件属于父会话中的哪个 `Agent` tool call。

**关联机制**

`subagent.rs` 在启动时，通过 `ToolEvent::Metadata` 将 `parent_tool_id`（即 `Agent` tool 的 `tool_call_id`）和 `subagent_session_id` 一起广播到父 session。

EventPump 收到 `Metadata` 后，维护一个内存映射：

```rust
subagent_sessions: HashMap<SessionId, String>  // subagent_session_id → parent_tool_id
```

当 subagent subscriber 收到事件时，EventPump 查找 `subagent_sessions` 获取 `parent_tool_id`，将事件包装为 `TaggedEvent::Subagent { parent_tool_id, event }` 发送给 TUI。

```rust
pub enum TaggedEvent {
    Main(Event),           // 主 session 事件，直接处理
    Subagent {
        parent_tool_id: String,
        event: Event,
    },
}
```

TUI 的 `process_kernel_event` 处理 `TaggedEvent::Subagent` 时，根据 `parent_tool_id` 更新 `ChatView` 中对应 `HistoryMessage::Tool` 的 `SubagentState`。

**映射清理**

当 subagent subscriber 退出（收到 `Stopped` 或 `ConnectionLost`）时，EventPump 从 `subagent_sessions` 中移除对应条目，避免内存泄漏。

---

### 3.5 TUI 渲染模型：SubagentState 内嵌视图

`ChatView` 的 `HistoryMessage::Tool` 增加 `subagent` 字段：

```rust
pub struct SubagentState {
    pub session_id: SessionId,
    pub description: String,
    pub status: SubagentStatus,   // Running / Completed / Failed / Cancelled
    pub events: Vec<Event>,       // 累积的 subagent 事件
    pub folded: bool,             // 默认折叠
}

pub enum HistoryMessage {
    // ...
    Tool {
        tool_name: String,
        tool_id: String,
        status: ToolStatus,
        // ...
        subagent: Option<SubagentState>,  // 新增
    },
}
```

**默认状态（折叠）**

只显示单行实时摘要：

```
 Agent · Audit dependencies  async
 ⎿ 󰔟 Running… (grep · read · 2 tools done)
```

摘要算法：取最近 3 个非 streaming 事件，显示工具名，超出用 `…`。

**展开状态**

在原 tool 输出区域下方渲染一个**带缩进引导线的内嵌面板**，复用主会话的渲染逻辑（streaming markdown、tool icon、thinking 折叠），整体缩进 2 格：

```
 Agent · Audit dependencies  async  12.3s
 ┊ ▼ sub_abc123 · Audit dependencies
 ┊  Thinking (42 tokens) · 1.2s
 ┊  grep Cargo.toml
 ┊  Read src/lib.rs
 ┊ 󰔟 exploring architecture...
 ┊  Agent completed
 ┊ 
 ┊ Result: Found 3 outdated dependencies
```

subagent 内部的事件颜色比主会话**降一级**（thinking 用更灰的灰，tool 用更暗的绿/红），通过视觉层级区分。

**交互**

| 交互 | 行为 |
|------|------|
| 默认 | subagent 折叠，只显示单行实时摘要 |
| 点击 `Agent` tool header / 按 `e` | 展开/折叠 subagent 内嵌视图 |
| `Ctrl-O` (expand all) | 同时展开所有 subagent |

---

## 4. 数据流时序

### 4.1 正常 Subagent 执行（sync 模式）

```
Timeline ──────────────────────────────────────────────────────►

Parent Agent
  │  1. ToolEvent::Start { tool_id: "call_01", tool_name: "agent" }
  │  2. ToolEvent::Metadata { subagent_session_id: "sub_abc", parent_tool_id: "call_01" }
  │        │
  │        ▼
  │     EventBus ──► TUI EventPump (main subscriber)
  │                      │ 解析 Metadata，提取 subagent_session_id + parent_tool_id
  │                      │ 维护映射: sub_abc → call_01
  │                      │ coordinator.subscribe_session_events("sub_abc")
  │                      ▼
  │                   [New subscriber for sub_abc]
  │                      │
  │                      ▼
  │                   TUI 收到 TaggedEvent::Subagent { parent_tool_id: "call_01", event }
  │                      │ 在 ChatView 初始化 HistoryMessage::Tool { tool_id: "call_01", subagent: Some(...) }
  │                      │
  │  3. Subagent 执行
  │        │
  │        ▼
  │     subagent.rs ──► EventBus (session: sub_abc)
  │        │  ModelEvent::Chunk
  │        │  ToolEvent::Start { read }
  │        │  ToolEvent::End { read }
  │        │  AgentEvent::Lifecycle(Stopped)
  │        │
  │        ▼
  │     EventBus ──► TUI Subagent Subscriber
  │                      │
  │                      ▼
  │                   TUI 实时更新 call_01.subagent
  │                      │ 追加 chunk、更新 tool 状态
  │                      │
  │  4. Subagent 结束
  │        │
  │        ▼
  │     EventBus ──► TUI Subagent Subscriber
  │                      │ 收到 Lifecycle(Stopped)
  │                      │ 标记 SubagentState 为 Completed
  │                      │ subscriber 退出，Drop 触发 unsubscribe
  │                      │ 从 subagent_sessions 映射中移除 sub_abc
  │                      ▼
  │  5. subagent.rs 返回
  │        │
  │        ▼
  │     ToolEvent::End { tool_id: "call_01", output: "..." }
  │        │
  │        ▼
  │     TUI 收到 ToolEvent::End，关闭 call_01 的 Tool 卡片
  │     （SubagentState 的实时视图保留，但不再更新）
```

### 4.2 并发 Subagent（async 模式）

```
Parent Agent
  │  spawn subagent_1 (task A) ──► Metadata { sub_1, parent: call_01 }
  │  spawn subagent_2 (task B) ──► Metadata { sub_2, parent: call_02 }
  │        │
  │        ▼
  │     EventPump 同时持有:
  │        - main_session (filter: 排除 Internal)
  │        - sub_1 (filter: 排除 Internal + 可选排除 delta)
  │        - sub_2 (filter: 排除 Internal + 可选排除 delta)
  │        │
  │        ▼
  │     TUI ChatView 中同时显示 call_01 和 call_02 的 SubagentState 实时进度
  │        │
  │  sub_1 先完成 ──► subscriber 退出，unsubscribe，释放映射
  │  sub_2 后完成 ──► subscriber 退出，unsubscribe，释放映射
  │
  │  最终只剩下 main_session subscriber
```

---

## 5. 接口变化

### 5.1 Kernel（`crates/kernel/`）

| 组件 | 当前 | 变更后 | 影响 |
|------|------|--------|------|
| `types.rs` | `EVT_PREFIX` 已存在 | `define_id!(EventId => "evt_")` | 新增类型 |
| `event.rs` | `Event` 枚举 | 重构为 `Event` 结构体 + `EventPayload` 枚举 | 全项目类型引用需迁移 |
| `comms/bus.rs` | `PubSub` 无 filter | `Listener` 增加 `filter` 字段，`subscribe` 支持 `Fn(&T) -> bool` | `PubSub` 内部 forwarder 逻辑修改 |
| `comms/bus.rs` | `PubSubHandle` | 发送时自动注入 `event_id`、`session_id`、`timestamp_ms` | `PubSubHandle::send` 内部行为变化 |
| `agent/agent.rs` | `event_bus.send(Event::Model(...))` | `event_bus.send(EventPayload::Model(...))`（`session_id` 自动注入） | 事件发送点简化 |
| `tools/subagent.rs` | `Metadata` 含 `subagent_session_id` | 增加 `parent_tool_id` | 字段增加 |
| `app/conductor.rs` | `subscribe_all` 处理全部事件 | 改为 `subscribe`（接收全部，含 Internal）或 `subscribe_external` | 订阅方式调整 |

### 5.2 TUI（`crates/tui/`）

| 组件 | 当前 | 变更后 | 影响 |
|------|------|--------|------|
| `app/event_pump.rs` | 单 subscriber 循环 | 订阅池 + `subagent_sessions` 映射 + `TaggedEvent` | 架构重写 |
| `app/events.rs` | `process_kernel_event` 处理 `Event` | 处理 `TaggedEvent`，区分 `Main`/`Subagent` | 新增 `Subagent` 分支 |
| `components/chat_view/core.rs` | `HistoryMessage::Tool` 无 subagent | 增加 `subagent: Option<SubagentState>` | 渲染和状态管理扩展 |
| `msg.rs` | `Msg::AppEvent(Event)` | 可能改为 `Msg::AppEvent(Event)` + 新字段标识来源 | 消息类型调整 |

---

## 6. 影响范围

### 6.1 必须同步修改的模块

- `EventBus` 的 `PubSub` 过滤机制（`comms/bus.rs`）
- 所有调用 `event_bus.send(Event::...)` 的代码（改为 `EventPayload::...`）
- `EventPump` 的完整重写（订阅池、映射管理、TaggedEvent）
- `ChatView` 的消息模型和渲染逻辑

### 6.2 无需修改或仅需适配的模块

- `ChannelHub`（外部 consumer）：从 `Event` 枚举迁移到 `Event` 结构体，业务逻辑不变。
- `storage` 层（内部 consumer）：继续使用 `subscribe` 接收全部事件（含 Internal），`filter` 不影响。
- `providers` 和 `tools`（除 `subagent.rs`）：事件来源和语义不变。
- `gui` crate：如果通过 IPC 消费 JSON，需同步升级序列化格式。`EventPayload` 的变体名与旧 `Event` 相同，主要变化是顶层多了 `event_id`/`session_id`/`timestamp_ms` 字段。

---

## 7. 迁移计划

### Phase 1: PubSub 过滤 + Event 结构体（独立可合并）

1. `PubSub` 增加 `listener.filter` 字段，`subscribe` 支持传入 `Fn(&T) -> bool`。
2. `forwarder` 在 `try_send_to_listeners` 前应用 filter。
3. `EventBus` 增加 `subscribe_external`（默认排除 `Internal`）和 `subscribe_filtered`。
4. `Event` 重构为结构体 + `EventPayload` 枚举，`PubSubHandle` 自动注入 `event_id`/`session_id`/`timestamp_ms`。
5. 全项目替换 `event_bus.send(Event::...)` → `event_bus.send(EventPayload::...)`。
6. `Conductor` 的 subscriber 从 `subscribe_all` 改为 `subscribe`（接收全部，含 Internal）。
7. 单元测试：验证外部 subscriber 收不到 `InternalEvent`，内部 subscriber 能收到。

### Phase 2: Subagent 事件关联（独立可合并）

1. `subagent.rs` 在 `ToolEvent::Metadata` 中增加 `parent_tool_id`。
2. 验证：TUI 能解析 `Metadata` 并提取 `subagent_session_id` + `parent_tool_id`。

### Phase 3: TUI 动态订阅与渲染（依赖 Phase 1+2）

1. 重写 `EventPump` 为订阅池模型。
2. 在 `process_kernel_event` 中处理 `TaggedEvent::Subagent`。
3. 在 `ChatView` 中增加 `SubagentState` 和渲染逻辑。
4. 交互：点击/按键展开/折叠 subagent 内嵌视图。

### Phase 4: 清理（最终）

1. 删除 TUI 中所有对 `Event::Internal` 的防御性忽略分支（`_ => {}` 中针对 `Internal` 的处理）。
2. 文档更新。

---

## 8. 风险与未决问题

| # | 风险/问题 | 说明 | 缓解措施 |
|---|----------|------|---------|
| R1 | IPC 序列化 breaking change | `Event` 从枚举变为结构体，JSON 格式变化 | server/client 同步升级，不保留兼容。GUI 等外部 consumer 需同步适配 |
| R2 | `PubSubHandle` 自动注入字段的性能 | 每次发送都生成 `EventId`（ULID）和 `timestamp_ms` | ULID 生成是轻量的（基于原子计数器），`timestamp_ms` 是 `SystemTime::now()`，影响可忽略 |
| R3 | subagent subscriber 未收到 `Stopped` | subagent panic 或强制 kill 导致未发生命周期事件 | `subscriber.recv()` 会返回 `None`（channel close），触发 Drop 自动清理。超时机制由 `subagent.rs` 的调用方保证 |
| R4 | `subagent_sessions` 映射内存泄漏 | EventPump 崩溃或未收到 `Stopped` 时映射未清理 | 映射只在 EventPump 内存中，EventPump 与 TUI 生命周期绑定。subscriber 的 `Drop` 保证 unsubscribe。极端情况下可加入 TTL 机制（非 MVP） |
| R5 | Filter 闭包不能跨 IPC 序列化 | TUI 的 filter 是本地闭包，RPC 流仍传输全部事件，由 TUI 本地过滤 | 如果带宽敏感，后续可在 `subscribe_session_events` 的 RPC 参数中增加 `filter_mask` 让 server 预过滤。MVP 阶段本地过滤足够 |

---

## 9. 附录

### 9.1 术语表

| 术语 | 含义 |
|------|------|
| `Event` | 自描述事件结构体，包含 `event_id`、`session_id`、`timestamp_ms`、`payload` |
| `EventPayload` | 原 `Event` 枚举的内容，包含 `User`/`Agent`/`Model`/`Tool`/`System`/`Internal` 变体 |
| `EventId` | 通过 `define_id!` 生成的 ULID 类型，前缀 `evt_` |
| `PubSub` | 泛型发布-订阅通道，基于 `mpsc` 的 `forwarder` 分发 |
| `Listener.filter` | 每个 subscriber 的事件过滤闭包，`forwarder` 在发送前应用 |
| `subscribe_external` | `EventBus` 的便捷方法，默认 filter 排除 `InternalEvent` |
| `subscribe_filtered` | `EventBus` 的自定义 filter 订阅方法，支持约束 C4 |
| `TaggedEvent` | TUI 内部使用的枚举，区分主 session 事件和 subagent 事件 |
| `SubagentState` | TUI `ChatView` 中用于存储单个 subagent 实时状态的结构体 |
| `subagent_sessions` | EventPump 内部的 `HashMap<SessionId, String>`，映射 subagent session → parent tool_id |
| `parent_tool_id` | 父会话中 `Agent` 工具的 `tool_call_id`，用于 TUI 关联 subagent 事件 |
