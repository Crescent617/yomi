# 无 Session 运行时架构设计

## 设计目标

- **删除 `Session` 运行时层**。`Session` struct（`app/session.rs`）完全消除，概念上只保留数据库里的 `SessionId` / `SessionInfo`（会话元数据）。
- **Coordinator 变纯 API 层**。所有操作 fire-and-forget：直接写 InputBus、直接写 Storage。不再 `require_session`，不再持有运行时对象。
- **Agent 是短暂的纯计算单元**。按需拉起，Idle + mailbox 空时自动退出。
- **总线即基础设施**。InputBus 是全局输入 channel，Conductor 消费并分发；EventBus 负责副作用与状态同步。

---

## 核心变化概览

| 当前架构 | 新架构 |
|---------|--------|
| `Coordinator → Session → InputBus → Agent` | `Coordinator → InputBus → Conductor → Mailbox → Agent` |
| `Session` 持有 `agent_state` 缓存、title 逻辑、cancel_token、ask_user/permission 转发 | `Session` 删除；title 内联在 Coordinator；其余状态嵌入 Agent |
| Agent 长期运行，显式 `close()` / `Shutdown` 关闭 | Agent 自动生命周期：spawn → run → idle & empty → exit |
| Coordinator `require_session_or_restore()` 同步查内存 | Coordinator 直接发命令，无需查内存；Conductor 负责 lazy spawn |

---

## 新架构组件

### 1. Coordinator（纯 API 层）

只保留业务编排和存储查询，不管理任何运行时对象。

```rust
pub struct Coordinator {
    input_bus: Arc<InputBus>,
    conductor: Arc<Conductor>,
    session_store: Arc<dyn SessionStore>,
    message_store: Arc<dyn MessageStore>,
    event_bus: Option<Arc<EventBus>>,
    // ...
}

impl Coordinator {
    pub async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()> {
        // 1. 持久化
        self.message_store.append(session_id, &[Message::user(blocks.clone())]).await?;

        // 2. 内联 title update
        let title = normalize_session_title(&text_from_blocks(&blocks));
        if !title.is_empty() {
            let _ = self.session_store.update_title(session_id, &title).await;
            self.emit(SystemEvent::TitleUpdated { session_id: session_id.clone(), title });
        }

        // 3. fire-and-forget
        self.input_bus.publish(&session_id, AgentInput::User { content: blocks })?;
        Ok(())
    }

    pub fn cancel(&self, session_id: &SessionId) {
        let _ = self.input_bus.publish(session_id, AgentInput::Cancel);
    }

    pub async fn send_permission_response(&self, sid: &SessionId, req_id: &str, approved: bool) -> Result<()> {
        self.input_bus.publish(sid, AgentInput::PermissionResponse { req_id: req_id.into(), approved })?;
        Ok(())
    }

    pub async fn get_session_status(&self, sid: &SessionId) -> Result<SessionStatus> {
        let phase = match self.conductor.get_state(sid) {
            Some(AgentState::Streaming) => "streaming",
            Some(AgentState::ExecutingTool) => "executing_tool",
            Some(AgentState::Compacting) => "compacting",
            Some(AgentState::Idle) | None => "idle",
        };
        Ok(SessionStatus { phase: phase.into() })
    }
}
```

> Coordinator 通过 Conductor 查询 AgentState，不自己缓存。

---

### 2. Mailbox（双队列缓冲）

与 Agent 1:1 绑定。纯 `VecDeque`，同步操作。

```rust
pub struct Mailbox {
    steer: Mutex<VecDeque<ContentBlock>>,   // steer 直接存 content blocks，flat
    normal: Mutex<VecDeque<AgentInput>>,
}

impl Mailbox {
    pub fn push(&self, input: AgentInput);
    pub fn push_steer(&self, content: Vec<ContentBlock>);
    pub fn try_pull(&self, count: usize) -> Vec<AgentInput>;
    pub fn try_pull_steer(&self, count: usize) -> Vec<ContentBlock>;
    pub fn is_steer_empty(&self) -> bool;   // Idle 分支插队判断
    pub fn clear(&self);   // cancel 时清空
}
```

- `steer` 高优先级：Agent 进入 Streaming 前批量消费（最多 20 条），作为用户上下文注入 `message_buffer`。
- `normal` 普通消息：Agent Idle 时每次取 1 条。

---

### 3. InputBus（极简 channel）

多生产者单消费者。Conductor 是唯一消费者。

```rust
pub struct InputBus {
    tx: mpsc::Sender<(SessionId, AgentInput)>,
}

impl InputBus {
    pub fn new() -> (Arc<Self>, mpsc::Receiver<(SessionId, AgentInput)>);
    pub fn publish(&self, sid: &SessionId, input: AgentInput) -> Result<(), InputBusError>;
}
```

---

### 4. Conductor（Agent 生命周期 + Cancel 分发）

InputBus 唯一消费者。职责：
1. 接收 `(SessionId, AgentInput)`。
2. `Cancel` 直接捅 `CancelToken` + `mailbox.clear()`（不排队）。
3. 其他消息分发到对应 Mailbox；无活跃 Agent 时创建 Mailbox 并 spawn。
4. Agent 退出后通过 EventBus `Shutdown` 清理活跃表。

```rust
pub struct Conductor {
    agent_shared: Arc<AgentShared>,
    active: DashMap<SessionId, ActiveAgent>,
    rx: mpsc::Receiver<(SessionId, AgentInput)>,
    event_bus: Arc<EventBus>,
}

struct ActiveAgent {
    mailbox: Arc<Mailbox>,
    handle: JoinHandle<()>,
    cancel_token: CancelToken,
    state: Atomic<AgentState>,
}

impl Conductor {
    pub async fn run(mut self) {
        let mut subscriber = self.event_bus.subscribe_all();

        loop {
            tokio::select! {
                Some((sid, input)) = self.rx.recv() => {
                    self.handle_input(sid, input).await;
                }
                Some((sid, event)) = subscriber.recv() => {
                    match event {
                        Event::Agent(AgentEvent::StateChanged { state }) => {
                            if let Some(agent) = self.active.get(&sid) {
                                agent.state.store(state);
                            }
                        }
                        Event::System(SystemEvent::Shutdown { .. }) => {
                            self.active.remove(&sid);
                        }
                        _ => {}
                    }
                }
                else => break,
            }
        }
    }

    pub fn get_state(&self, sid: &SessionId) -> Option<AgentState> {
        self.active.get(sid).map(|a| a.state.load())
    }

    async fn handle_input(&self, sid: SessionId, input: AgentInput) {
        match input {
            AgentInput::Cancel => {
                if let Some(agent) = self.active.get(&sid) {
                    agent.cancel_token.cancel();
                    agent.mailbox.clear();
                }
            }
            _ => {
                match self.active.get(&sid) {
                    Some(agent) => match &input {
                        AgentInput::Steer(content) => agent.mailbox.push_steer(content),
                        _ => agent.mailbox.push(input),
                    },
                    None => {
                        let mailbox = Arc::new(Mailbox::new());
                        match &input {
                            AgentInput::Steer(content) => mailbox.push_steer(content.clone()),
                            _ => mailbox.push(input),
                        }
                        self.spawn_agent(sid, mailbox).await;
                    }
                }
            }
        }
    }

    async fn spawn_agent(&self, sid: SessionId, mailbox: Arc<Mailbox>) {
        let history = self.agent_shared.message_store
            .as_ref()
            .and_then(|s| s.get(&sid.0).await.ok())
            .unwrap_or_default();

        let cancel_token = CancelToken::new();
        let args = AgentSpawnArgs::new(system_prompt, sid.0.clone())
            .with_history(history)
            .with_cancel_token(cancel_token.clone())
            .with_mailbox(mailbox.clone());

        let agent = Agent::new(&self.agent_shared, args);
        let handle = tokio::spawn(async move {
            let _ = agent.start_loop().await;
        });

        self.active.insert(sid, ActiveAgent {
            mailbox,
            handle,
            cancel_token,
            state: Atomic::new(AgentState::Idle),
        });
    }
}
```

> **关键点**：Cancel 直接走 `cancel_token.cancel()`，Agent streaming 时用 `select! { biased; _ = token.cancelled() => ... }` 立即中断。不经过 Mailbox 排队。

---

### 5. Agent（纯计算，自动退出）

运行时状态自建，随 Agent 生死。`spawn` 不是 async。

```rust
pub struct Agent {
    mailbox: Arc<Mailbox>,
    cancel_token: CancelToken,
    permission_state: Option<PermissionState>,
    ask_user_state: Option<AskUserState>,
    shared: Arc<AgentShared>,
    message_buffer: MessageBuffer,
    context: AgentExecutionContext,
    session_id: SessionId,
    // ...
}

impl Agent {
    pub fn new(shared: &Arc<AgentShared>, args: AgentSpawnArgs) -> Self {
        Self { /* ... */ }
    }

    pub async fn start_loop(mut self) -> Result<(), AgentError> {
        // 1. emit start
        self.emit(AgentEvent::Started).await;

        // 2. defer emit stop（无论正常退出、cancel、panic 都保证发送）
        let sid = self.session_id.clone();
        let event_sink = self.event_sink.clone();
        let _guard = scopeguard::guard((), move |_| {
            let _ = event_sink.try_send(Event::System(
                SystemEvent::Shutdown { session_id: sid }
            ));
        });

        loop {
            match self.context.current_state() {
                AgentState::Idle => {
                    self.context.reset_iteration();

                    // steer 插队：有 steer 直接进入 Streaming，不消费
                    if !self.mailbox.is_steer_empty() {
                        self.context.transition_to(AgentState::Streaming);
                        continue;
                    }

                    // 取一条普通消息
                    match self.mailbox.try_pull(1).into_iter().next() {
                        Some(input) => self.handle_input(input).await?,
                        None => break, // guard 会发 Shutdown
                    }
                }
                AgentState::Streaming => self.handle_streaming().await?, // 内部先 try_pull_steer(20)
                AgentState::ExecutingTool => self.handle_execute_tool().await?,
                AgentState::Compacting | AgentState::Closed => break, // guard 会发 Shutdown
            }
        }

        Ok(())
    }
}
```

---

## 关键数据流

### 发送用户消息

```
CLI/GUI ──► Coordinator::send_message(sid, blocks)
  │
  ├──► MessageStore.append()
  ├──► SessionStore.update_title() + EventBus::TitleUpdated
  └──► InputBus.publish(sid, AgentInput::User { content: blocks })
           │
           └──► mpsc::channel ──► Conductor::run()
                                    │
                                    ├─[活跃]──► ActiveAgent.mailbox.push()
                                    │              └──► Agent::try_pull(1)
                                    │
                                    └─[未活跃]─► 新建 Mailbox + push
                                                    ├──► Agent::new(args) + tokio::spawn(start_loop)
                                                    └──► Agent 从 MessageStore 加载历史
```

### Cancel

```
CLI/GUI ──► Coordinator::cancel(sid)
  │
  └──► InputBus.publish(sid, Cancel)
           │
           └──► Conductor::handle_input
                    │
                    └──► ActiveAgent.cancel_token.cancel()
                         ActiveAgent.mailbox.clear()
                              │
                              ├──► Agent streaming: select! 检测到 cancel → 立即中断
                              └──► Agent idle: mailbox 空 + cancel → 直接 break
```

### Agent 自然退出

```
Agent → Idle → is_steer_empty() == true → try_pull(1) 空
  │
  ├──► EventBus::System(Shutdown)
  └──► break loop

Conductor 侧：
  EventBus::Shutdown ──► active.remove(sid) ──► Mailbox Arc ref count 归零，释放
```

---

## 删除清单

| 删除项 | 理由 |
|--------|------|
| `app/session.rs` | Session 运行时层完全消除 |
| `Coordinator.sessions: DashMap<...>` | 运行时内存表删除 |
| `Coordinator::require_session()` / `require_session_or_restore()` | 不再需要 |
| `Coordinator.state_cache` | Coordinator 通过 Conductor 查询状态 |
| `InputBusHandle` / `InputBus::subscribe` | Conductor 直接消费 channel |
| `SessionRuntimeMap` / `SessionRuntimeState` | runtime state 嵌入 Agent |
| `last_activity_at` / `touch` / pruner | Agent 自动退出 |
| InputBus backlog | 消息已在 MessageStore |
| Conductor 额外 `tokio::spawn(async { handle.await; remove })` | 用 EventBus Shutdown 清理 |

---

## 修改清单

| 文件 | 改动 |
|------|------|
| `app/session.rs` | **删除** |
| `app/coordinator.rs` | 删除 `sessions` / `require_session`；方法改为 fire-and-forget；`send_message` 内联 title update；状态查询通过 Conductor |
| `app/conductor.rs` | **新增**：InputBus 消费者，Mailbox 管理，Agent lazy spawn，Cancel 直接分发 |
| `event_bus/input_bus.rs` | 退化为 `mpsc::Sender<(SessionId, AgentInput)>` 包装 |
| `event_bus/mailbox.rs` | **新增**：双队列 `Mailbox` |
| `agent/agent.rs` | `spawn()` 改为 `new()`，只负责构造；`start_loop()` 由 Conductor 调用；内部自建 cancel_token / permission_state / ask_user_state；Idle 空载退出 |
| `agent/types.rs` | `AgentSpawnArgs` 移除 `ask_user_state`、`cancel_token`；新增 `mailbox: Arc<Mailbox>` |
| `tools/subagent.rs` | 本地创建 `Mailbox`，`Agent::new()` + 自己 `tokio::spawn(start_loop)` |
| `lib.rs` | `pub use app::{Session, SessionConfig}` → `pub use app::{Coordinator, Conductor}` |

---

## 风险

| 风险 | 缓解 |
|------|------|
| Agent 频繁 spawn/exit | Tokio spawn 成本极低；若瓶颈，Conductor 加"退出冷静期"（Idle 后等待 5s 再 break） |
| Agent 退出时消息到达 | 消息已在 MessageStore；Conductor 收到 InputBus 后创建新 Mailbox + spawn，新 Agent 从历史加载 |
| Cancel 时 Agent 已退出 | `active.get(&sid)` 为空，无操作，无副作用 |

---

## 一句话总结

> **Coordinator 只管发命令，InputBus 只管传信，Conductor 只管拉 Agent 和管 Mailbox，Agent 只管算，算完没事就自己走。副作用全靠 EventBus。没有 Session 这层中转。**
