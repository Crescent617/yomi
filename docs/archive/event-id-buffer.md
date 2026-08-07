# 设计文档：Wire Event 增加 Event ID 与 Session 事件缓冲区（修订版）

## 背景

当前 `WireMsg::Event` 没有唯一标识，客户端订阅后只能被动接收实时事件。如果客户端因网络抖动或重启而错过事件，无法通过重传恢复。我们需要：

1. 为每个事件分配唯一 ID（`EventId`）；
2. 在 `Subscribe` 请求中增加 `after_event_id` 参数，支持断点续传；
3. 在 Server 层维护 per-session 事件缓冲区；
4. 当 `InternalEvent::MessageAdded` 到达时清空缓冲区（标志新一轮对话开始）；
5. 当 Agent 真正 shutdown 时删除该 session 的缓冲区 entry。

## 核心原则：最小改动

- **`EventBus` 不做任何修改**：仍然只传播裸 `Event`。
- **`Conductor` 不做任何修改**：不感知 `EventId`。
- **`Agent` 不做任何修改**：事件生产逻辑完全不变。
- **所有 `EventId` 生成和缓冲逻辑集中在 `KernelServer` 层**，在 RPC 出口处统一包装。

## 事件缓冲区（EventBuffer）

### 位置

`KernelServer` 持有 `Arc<EventBuffer>`，内部结构：

```rust
struct EventBuffer {
    max_size: usize, // 10000
    buffers: DashMap<SessionId, Vec<BufferedEvent>>,
}

struct BufferedEvent {
    event_id: EventId,
    event: Event,
}
```

### 为什么放在 Server 层？

`EventId` 是 wire/transport 层的概念，不是 kernel 内部业务概念。`EventBus` 和 `Conductor` 继续用裸 `Event` 工作。`KernelServer` 在把事件转发给客户端之前，才分配 `EventId` 并存入缓冲区。这样内部组件完全解耦。

### 事件流入

`KernelServer::start` 启动一个全局后台任务，通过 `coordinator.event_bus().subscribe_all()` 监听所有事件：

1. 收到 `(sid, event)` 后，生成 `EventId::new()`（`evt_` + `Ulid`）。
2. 追加到 `buffers[sid]` 尾部。
3. 如果该 buffer 超过 `max_size`（10000），淘汰头部最旧的事件。
4. 同时把 `(event_id, event)` 转发给该 session 的所有活跃 wire 订阅者（见下方实时转发）。
5. 检查事件类型：
   - 如果是 `Event::Internal(InternalEvent::MessageAdded { .. })` → **清空该 session 的 buffer**（`buffers.remove(&sid)`）。
   - 如果是 `Event::Agent(AgentEvent::Lifecycle { state: Stopped { .. } })` → **删除该 session 的 buffer entry**（`buffers.remove(&sid)`）。

### 事件流出（Subscribe 处理）

`handle_connection` 处理 `RequestMethod::Subscribe` 时：

1. **回放历史**：从 `EventBuffer` 中二分查找 `after_event_id` 之后的事件，通过 `send_tx` 发送给客户端。
   - 如果 `after_event_id` 为 `None` → 发送整个 buffer。
   - 如果 `after_event_id` 不在 buffer 中 → 发送整个 buffer（客户端自己处理不连续）。
2. **注册实时订阅**：向 `KernelServer` 注册一个 `mpsc::Sender<(EventId, Event)>`，接收该 session 的后续实时事件。
3. **实时转发**： spawned task 从 `mpsc::Receiver` 接收事件，包装成 `WireMsg::Event` 发送。
4. **unsubscribe 时**：从 `KernelServer` 注销该 sender，清理对应的 spawned task。

## EventId 的排序与查找

`EventId` 基于 `Ulid`，单线程生成时字典序单调递增。`KernelServer` 的全局任务只有一个，因此生成的 `EventId` 是严格单调的。

为支持二分查找，需要给 `EventId` 增加 `Ord/PartialOrd` 派生。由于 `EventId` 是 `define_id!` 宏生成的，修改宏统一为所有 ID 类型增加 `Ord/PartialOrd`（安全，不影响现有行为）。

查找实现（线性扫描也可，但二分查找更优雅）：
```rust
fn get_after(&self, sid: &SessionId, after: Option<&EventId>) -> Vec<(EventId, Event)> {
    let buf = match self.buffers.get(sid) {
        Some(b) => b,
        None => return Vec::new(),
    };
    let start = match after {
        Some(id) => match buf.binary_search_by(|be| be.event_id.cmp(id)) {
            Ok(idx) => idx + 1,      // exclusive
            Err(idx) => idx,         // 不在 buffer 中，从 idx 开始发
        },
        None => 0,
    };
    buf[start..].iter().map(|be| (be.event_id.clone(), be.event.clone())).collect()
}
```

## 缓冲区上限与截断

每个 session 最多保留 10000 条事件。超过时淘汰头部（最旧）。如果某个 turn 特别长，buffer 被截断是可接受的——客户端重连后只会看到该 turn 的尾部事件。

## 生命周期

| 触发条件 | 动作 |
|---|---|
| `MessageAdded` | 清空 buffer（标志新一轮对话开始，旧事件不再有意义） |
| `AgentEvent::Lifecycle { Stopped }` | 删除 buffer entry（agent 真正结束，session 无活跃推理） |
| `DeleteSession`（可选） | 如果调用 `delete_session`，Server 侧也应同步删除对应 buffer entry（可在 `Server` 的 `dispatch_request` 中处理） |

## Wire Protocol 变更

- `WireMsg::Event` 增加 `event_id: EventId` 字段。
- `RequestMethod::Subscribe` 增加 `after_event_id: Option<EventId>` 字段。
- `WIRE_PROTOCOL_VERSION` 从 `6` 升级到 `7`。

## 客户端变更

- `RemoteKernel::subscribe_session_events` 签名增加 `after_event_id: Option<EventId>`，透传给 `RequestMethod::Subscribe`。
- 旧客户端不传 `after_event_id`（`None`）时，服务器发送整个 buffer 后切实时，行为兼容。

## 无 Race 的订阅机制

`Server` 侧处理 `Subscribe` 的时序：
1. 先向 `KernelServer` 注册实时订阅（让 sender 开始排队）。
2. 再获取 `EventBuffer` 历史并发送。
3. 实时 sender 中积压的事件（步骤 1~2 之间到达的）会在历史发送完毕后自然发出，不会重复（因为 buffer 中的历史不包含这些事件——它们是在 `MessageAdded` 之后到达的，而 `MessageAdded` 会清空 buffer，或者它们在 buffer 尾部但尚未被截断）。

实际上，由于 `EventBuffer` 的写入和 `session_subscribers` 的写入都在 `KernelServer` 的同一个全局任务中串行执行，因此不存在 race：全局任务先写 buffer、再转发给 subscriber。`Subscribe` 处理时，先注册 subscriber（开始接收新事件），再读 buffer（读的是注册时刻之前的快照）。 subscriber 在注册之后收到的事件都是 buffer 中不存在的，因此不会重复。

## 待确认（如无异议则按此实现）

1. **EventId 的 `Ord/PartialOrd`**：给 `define_id!` 宏统一增加 `Ord, PartialOrd`，使所有 ID 类型支持排序。是否接受？
2. **Agent shutdown 的信号**：使用 `AgentEvent::Lifecycle { Stopped { .. } }` 作为删除 buffer entry 的触发器。是否准确？
3. **Subscribe 时如果 buffer 为空**：直接开始实时推送，无额外事件。是否正确？
