# Agent Event Bus 设计

## 背景

当前事件流是管道式接力，不是真正的总线：

```
Agent (mpsc::Sender<Event>)
    ↓
Kernel::forward_session_events (task) 从 mpsc 读
    ↓
per-session broadcast::Sender<Event>
    ↓
TUI / GUI / Server 订阅
```

**问题：**
1. **Agent 只发不管往哪发** — Agent 通过 `mpsc::Sender<Event>` 把事件丢给 Kernel，自己不知道 session 上下文。
2. **Kernel 太重** — 既要管 session 生命周期，又要管 `forward_session_events` 任务、维护 `session_event_senders: DashMap<...>`。
3. **Session 有两套事件出口** — `Session` 自己持有一个 `event_tx: Option<broadcast::Sender<Event>>` 发系统事件（TitleUpdated 等），Agent 又走另一套 mpsc。事件源不统一。
4. **Subscribe 逻辑分散** — 外部订阅者要找 `Kernel::subscribe_session_events`，它再查 `DashMap` 拿 per-session broadcast。没有"一个总线"的概念。

## 目标

1. 统一所有事件出口到单一总线
2. Kernel 不再管理事件转发
3. 支持全局订阅和单 session 订阅
4. 简化架构，减少中间任务
5. **Event bus 与 session 生命周期完全解耦** — 异步触发的 agent 也能订阅已结束或尚未开始的 session

## 核心设计

### 方案：单 forwarder 任务 + 显式 listener 列表

不用 `tokio::sync::broadcast`（它绑定到 channel 生命周期），改用 `mpsc::unbounded_channel` 作为每个 subscriber 的独立通道。所有 listener 状态由 forwarder 任务内部维护。

```
Agent / Session                    EventBus
    │                                 │
    │ try_send(Event)                 │
    │────────────────────────────────>│
    │                                 │
    │                                 │  ┌─────────────────────┐
    │                                 │  │   forwarder task    │
    │                                 │  │                     │
    │                                 │  │  session_listeners  │
    │                                 │  │  HashMap<Sid, Vec>  │
    │                                 │  │                     │
    │                                 │  │  global_listeners   │
    │                                 │  │  Vec<Listener>      │
    │                                 │  └─────────────────────┘
    │                                 │
TUI / GUI / Server                  │
    │ subscribe(sid)                  │
    │────────────────────────────────>│
    │  return EventBusSubscriber      │
    │  (guard: Drop -> unsubscribe)   │
    │<────────────────────────────────│
    │                                 │
    │ recv() <── clone 事件 ──────────│
```

### 为什么不用 broadcast

`tokio::sync::broadcast` 的问题是：
- 它绑定到 `Sender` 的生命周期。如果最后一个 `Receiver` 被 drop，后续 `subscribe` 无法收到历史事件（虽然这里不需要历史），但更重要的是，**per-session broadcast 的创建和销毁需要与 session 生命周期同步**。
- 用户要求"event bus 和 session 生命周期无关"，所以不能用 broadcast 的"session 存在时创建 channel，session 销毁时删除 channel"模型。

### 为什么用 mpsc::unbounded

每个 listener 是一个 `mpsc::UnboundedSender<Event>`：
- **无界缓冲**：forwarder 发送不会阻塞，不会因为某个 subscriber 慢而拖慢整个系统。
- **独立消费**：每个 subscriber 有自己的 `UnboundedReceiver`，事件会被 clone 给每个 subscriber。
- **自然广播**：`Vec<Listener>` 遍历发送，天然实现广播语义。
- **Receiver drop 检测**：如果 subscriber 被 drop，其 `UnboundedReceiver` 关闭，但 sender 的 `send` 只是返回 `Err`（被忽略），不会 panic。

## 类型设计

### 1. EventBus

```rust
pub struct EventBus {
    /// 生产者入口（缓冲，防丢包）
    event_tx: mpsc::Sender<(SessionId, Event)>,
    /// 命令通道（subscribe / unsubscribe）
    cmd_tx: mpsc::UnboundedSender<Command>,
    /// forwarder 任务句柄
    _forwarder: JoinHandle<()>,
}

struct Listener {
    id: u64,
    tx: mpsc::UnboundedSender<Event>,
}

enum Command {
    SubscribeSession {
        session_id: SessionId,
        listener: Listener,
    },
    SubscribeGlobal {
        listener: Listener,
    },
    Unsubscribe {
        id: u64,
        session_id: Option<SessionId>, // None = global
    },
}
```

### 2. EventBusHandle（生产者）

绑定 `session_id`，调用方不需要每次传 session ID。

```rust
#[derive(Clone)]
pub struct EventBusHandle {
    event_tx: mpsc::Sender<(SessionId, Event)>,
    session_id: SessionId,
}

impl EventBusHandle {
    pub async fn send(&self, event: Event) {
        let _ = self.event_tx.send((self.session_id.clone(), event)).await;
    }
    
    pub fn try_send(&self, event: Event) {
        let _ = self.event_tx.try_send((self.session_id.clone(), event));
    }
}
```

### 3. EventBusSubscriber（消费者 Guard）

`subscribe` 返回 guard，Drop 时自动发送 `Unsubscribe` command。

```rust
pub struct EventBusSubscriber {
    cmd_tx: mpsc::UnboundedSender<Command>,
    id: u64,
    session_id: Option<SessionId>, // None = global
    rx: mpsc::UnboundedReceiver<Event>,
}

impl EventBusSubscriber {
    pub async fn recv(&mut self) -> Option<Event> {
        self.rx.recv().await
    }
}

impl Drop for EventBusSubscriber {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(Command::Unsubscribe {
            id: self.id,
            session_id: self.session_id.clone(),
        });
    }
}
```

### 4. Forwarder 任务

单任务，零锁竞争，所有状态在任务内部管理。

```rust
async fn run_forwarder(
    mut event_rx: mpsc::Receiver<(SessionId, Event)>,
    mut cmd_rx: mpsc::UnboundedReceiver<Command>,
) {
    let mut session_listeners: HashMap<SessionId, Vec<Listener>> = HashMap::new();
    let mut global_listeners: Vec<Listener> = Vec::new();
    
    loop {
        tokio::select! {
            biased;
            
            // 优先处理命令，避免 subscribe 的事件被遗漏
            Some(cmd) = cmd_rx.recv() => {
                match cmd {
                    Command::SubscribeSession { session_id, listener } => {
                        session_listeners.entry(session_id).or_default().push(listener);
                    }
                    Command::SubscribeGlobal { listener } => {
                        global_listeners.push(listener);
                    }
                    Command::Unsubscribe { id, session_id } => {
                        if let Some(sid) = session_id {
                            if let Some(ls) = session_listeners.get_mut(&sid) {
                                ls.retain(|l| l.id != id);
                                if ls.is_empty() {
                                    session_listeners.remove(&sid);
                                }
                            }
                        } else {
                            global_listeners.retain(|l| l.id != id);
                        }
                    }
                }
            }
            
            Some((sid, ev)) = event_rx.recv() => {
                // 发给该 session 的所有 listener
                if let Some(ls) = session_listeners.get(&sid) {
                    for l in ls {
                        let _ = l.tx.send(ev.clone());
                    }
                }
                // 发给所有全局 listener
                for l in &global_listeners {
                    let _ = l.tx.send(ev.clone());
                }
            }
            
            else => break,
        }
    }
}
```

## 为什么这样设计是优雅的

1. **单任务管理所有状态**：`session_listeners` 和 `global_listeners` 都在 forwarder 任务内部，没有 `Arc<Mutex<...>>` 或 `DashMap` 的锁竞争。
2. **subscribe / unsubscribe 是消息而非方法调用**：`EventBusSubscriber` 的 Drop 只是发一个 command 到 channel，不需要 `Weak<Arc<...>>` 的复杂生命周期管理。
3. **与 session 生命周期完全解耦**：event bus 根本不知道 session 是否存在。任何代码可以在任何时候 `subscribe(session_id)`，即使该 session 从未创建过。当 session 有事件时，subscriber 自然会收到。
4. **异步触发的 agent 也能订阅**：cron 或外部 channel 触发的任务，可以直接 `bus.subscribe(session_id)` 获取 guard，监听事件流。
5. **零丢包**：`mpsc::Sender` 有缓冲（256），`unbounded` 的 listener 通道也有缓冲。即使所有 subscriber 都慢，事件也会缓存在各自的 channel 中。

## API 设计

```rust
impl EventBus {
    /// 创建总线
    pub fn new() -> Arc<Self>;
    
    /// 获取某个 session 的生产者句柄
    pub fn handle(&self, session_id: SessionId) -> EventBusHandle;
    
    /// 订阅单个 session 的事件
    pub fn subscribe(&self, session_id: SessionId) -> EventBusSubscriber;
    
    /// 订阅所有事件
    pub fn subscribe_all(&self) -> EventBusSubscriber;
}
```

## 数据流变化

### 当前

```rust
// Agent
self.event_tx.try_send(Event::Agent(...))

// Kernel
let (broadcast_tx, _) = broadcast::channel(256);
session.set_event_sender(broadcast_tx.clone());
tokio::spawn(forward_session_events(agent_rx, broadcast_tx, ...));

// 订阅
coordinator.subscribe_session_events(&sid) 
    -> DashMap 查 broadcast_tx -> tx.subscribe()
```

### 新设计

```rust
// Agent
self.event_bus.try_send(Event::Agent(...))

// Kernel
let event_bus = Arc::new(EventBus::new());
// ...
let handle = event_bus.handle(session_id.clone());
let session = Session::init(session_id, ..., handle).await?;
// 不再需要 forward_session_events task！
// 不再需要 session_event_senders DashMap！

// 订阅（可以在任何时刻，session 不需要存在）
let mut sub = event_bus.subscribe(session_id);
while let Some(ev) = sub.recv().await {
    // ...
}
// sub 被 drop 时自动 unsubscribe
```

## 改动范围

### 1. 新建 `crates/kernel/src/event_bus.rs`

核心就是上面的 `EventBus` + `EventBusHandle` + `EventBusSubscriber` + `run_forwarder`。

### 2. 修改 `AgentShared`（`agent/types.rs`）

```rust
pub struct AgentShared {
    pub event_bus: Arc<EventBus>,
    // ... 其他字段不变
}
```

### 3. 修改 `Agent`（`agent/agent.rs`）

- `event_tx: mpsc::Sender<Event>` → `event_bus: EventBusHandle`
- `session_id: String` → `session_id: SessionId`（统一类型）
- `Agent::spawn` 返回值从 `(AgentHandle, mpsc::Receiver<Event>)` 改为 `AgentHandle`
- 所有 `self.event_tx.try_send(...)` → `self.event_bus.try_send(...)`
- `AgentInput` 不变（它管的是输入，不是事件输出）
- Agent 结束时的 cleanup：
  - 当前 `forward_session_events` 在 `agent_rx` 关闭时自动发 `Shutdown` 并清理。
  - 新设计中，Agent 的 `start_loop` 退出前显式发送 `Shutdown` 事件（通过 `event_bus`）。

### 4. 修改 `Session`（`app/session.rs`）

- `event_tx: Option<broadcast::Sender<Event>>` → `event_bus: EventBusHandle`
- `Session::init` 签名变化：
  ```rust
  // 旧
  pub async fn init(...) -> Result<(Self, mpsc::Receiver<Event>)>
  // 新
  pub async fn init(..., event_bus: EventBusHandle) -> Result<Self>
  ```
- 所有 `emit_title_updated`、`emit_goal_updated` 等用 `event_bus.try_send(...)` 代替 `event_tx.send(...)`
- 去掉 `set_event_sender`

### 5. 修改 `Kernel`（`app/coordinator.rs`）

- 移除：
  - `session_event_senders: Arc<DashMap<SessionId, broadcast::Sender<Event>>>`
  - `forward_session_events` 方法
  - `init_session` 中的 `tokio::spawn(forward_session_events(...))`
- 添加：
  - `event_bus: Arc<EventBus>`（构造时创建）
- `init_session` 简化：
  ```rust
  async fn init_session(&self, session_id: SessionId, ...) -> Result<()> {
      let event_bus = self.event_bus.handle(session_id.clone());
      let session = Session::init(session_id.clone(), ..., event_bus).await?;
      ...
  }
  ```
- `subscribe_session_events` 直接透传：
  ```rust
  pub fn subscribe_session_events(&self, session_id: &SessionId) -> EventBusSubscriber {
      self.event_bus.subscribe(session_id.clone())
  }
  ```
  - 注意：返回类型从 `Option<broadcast::Receiver<Event>>` 改为 `EventBusSubscriber`（因为 subscribe 不再依赖 session 是否存在）。
- `spawn_session_pruner` 调整：
  - 当前检查 `broadcast_tx.receiver_count() == 0`。
  - 新设计中，检查 `event_bus` 的某个方法（如 `listener_count(session_id) == 0`），或者干脆不检查（因为 listener 是 guard，没有 subscriber 时自然没有 listener）。
  - 实际上，如果没有任何 subscriber 监听某个 session，那该 session 的 `session_listeners` 是空的。`EventBus` 可以暴露 `has_subscribers(session_id)` 方法。
  - 或者更简单：`spawn_session_pruner` 改为检查 session 是否 Idle 且**没有外部引用**（`AgentHandle` 是否还在）。但这属于 Kernel 的职责，bus 不需要管。
  - **最简方案**：pruner 不需要检查 subscriber 数量。它检查 `Arc<RwLock<Session>>` 的引用计数，或者只是定时检查 session 状态。

### 6. 修改 `Server`（`server/mod.rs`）

`RequestMethod::Subscribe` 的处理：
- 返回类型从 `Option<broadcast::Receiver<Event>>` 改为 `EventBusSubscriber`。
- 订阅循环：
  ```rust
  let mut sub = coordinator.subscribe_session_events(&sid);
  loop {
      tokio::select! {
          biased;
          () = cancel2.cancelled() => break,
          Some(ev) = sub.recv() => {
              let msg = WireMsg::Event { session_id: ..., event: ev };
              send_tx2.try_send(msg)?;
          }
          else => break,
      }
  }
  ```
  - 注意：`sub.recv()` 不会返回 `Err`（不像 `broadcast::Receiver::recv` 在 lag 时返回 `RecvError`），所以不需要处理 lag 错误。

### 7. 修改 `client/mod.rs`

`KernelApi` 接口：
- `subscribe_session_events` 签名变化：
  ```rust
  // 旧
  async fn subscribe_session_events(&self, session_id: &SessionId) -> Result<broadcast::Receiver<Event>>;
  // 新
  async fn subscribe_session_events(&self, session_id: &SessionId) -> Result<EventBusSubscriber>;
  ```
- `LocalKernel`（`Kernel` 的 `KernelApi` 实现）直接透传。
- `RemoteKernel` 需要调整：
  - 当前 `subscribe_events_internal` 在本地维护一个 `broadcast::Sender` router，然后 subscribe 到远程，把远程事件转发到本地 broadcast。
  - 新设计中，可以保持类似的本地 router 逻辑，但用 `mpsc::UnboundedSender` 代替 `broadcast::Sender`。
  - 或者更简单：`RemoteKernel` 的 `subscribe_session_events` 返回一个包装了 WebSocket 接收逻辑的 `EventBusSubscriber`（或者自定义的 subscriber 类型）。
  - 这取决于 `RemoteKernel` 的实现细节。如果 `EventBusSubscriber` 是 `kernel` 的内部类型，远程客户端可能无法直接持有它。
  - **替代方案**：保持 `EventBusSubscriber` 为 `kernel` 内部类型，但 `KernelApi` 的 `subscribe_session_events` 返回一个 `Box<dyn EventStream>` trait object，或者干脆保留 `broadcast::Receiver<Event>` 作为外部 API。
  - **建议**：先不改动 `KernelApi` 的返回类型。`Kernel::subscribe_session_events` 内部从 `EventBusSubscriber` 转换为 `broadcast::Receiver`（在 `EventBus` 内部加一个 `subscribe_broadcast` 方法，或者让 Kernel 自己桥接）。这样外部 API 不变，内部用新的 bus。
  - 但用户要求简化，所以可能直接改 API 更好。
  - 先搁置这个问题，实现时再看。

## 时序图

```
Agent                    EventBus                    TUI
 │                        │                          │
 │ try_send(Event)        │                          │
 │───────────────────────>│                          │
 │                        │ (forwarder task)         │
 │                        │ recv(sid, event)         │
 │                        │──┬───────────────────────>│
 │                        │  │ push to all listeners  │
 │                        │<─┘                        │
 │                        │                          │
 │ start_loop ends        │                          │
 │ try_send(Shutdown)     │                          │
 │───────────────────────>│                          │
 │                        │ forward to TUI           │
 │                        │─────────────────────────>│
 │                        │                          │
 │ Kernel::shutdown  │                          │
 │                        │                          │
 │  (no need to          │                          │
 │   unregister from bus)│                          │
 │                        │                          │
```

## 关键决策

1. **不用 `broadcast`，用 `mpsc::unbounded` 实现广播**：每个 subscriber 独立 channel，forwarder 遍历 clone 发送。单任务管理，零锁。

2. **subscribe 返回 guard，Drop 自动解注册**：`EventBusSubscriber` 的 `Drop` 发送 `Unsubscribe` command 到 forwarder，不需要 `Weak<Arc<...>>`。

3. **Event bus 与 session 生命周期完全解耦**：
   - 没有 `register` / `unregister` 方法。
   - 任何代码可以在任何时候 `subscribe(session_id)`。
   - session 结束不需要通知 bus。
   - 异步触发的 agent 也能订阅任何 session。

4. **Agent 结束时的 cleanup**：Agent 显式发送 `SystemEvent::Shutdown`（通过 `event_bus`）。Kernel 的 pruner 改为检查 session 状态 + 引用计数，而不是 subscriber 数量。

5. **外部 API 的兼容性**：`KernelApi::subscribe_session_events` 返回类型从 `broadcast::Receiver` 改为 `EventBusSubscriber`。如果需要保留外部 API 不变，可以在 `Kernel` 层做桥接（`EventBusSubscriber -> broadcast::Sender`）。建议直接改，因为 `EventBusSubscriber` 更简洁（没有 lag 错误）。

## 优势

1. **统一出口** — Agent 和 Session 都通过 `EventBusHandle` 发事件，不再各自为政。
2. **Kernel 减负** — 不再需要 `forward_session_events` 任务和 `session_event_senders` DashMap，Kernel 只负责业务逻辑。
3. **零丢包** — mpsc 缓冲保证事件不丢（buffer 满时 `try_send` 返回错误，生产者可以感知）。
4. **订阅灵活** — 支持全局订阅和单 session 订阅，且与 session 生命周期无关。
5. **测试友好** — `EventBusHandle` 是纯发送端，测试时可以直接构造，mock 事件流更简单。
6. **无锁设计** — 所有 listener 状态在 forwarder 单任务内管理，没有 `Mutex`/`RwLock`/`DashMap` 的锁竞争。

## 实现顺序

1. `event_bus.rs`（新文件）
2. 改 `AgentShared`（加 `event_bus`）
3. 改 `Agent`（去掉 mpsc，接入 bus）
4. 改 `Session`（去掉独立 broadcast，接入 bus）
5. 改 `Kernel`（简化事件管理）
6. 改 `Server` 和 `Client`（适配新的 subscribe API）
7. `cargo check` 修编译错误
