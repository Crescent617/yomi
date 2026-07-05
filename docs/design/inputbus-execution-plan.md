# InputBus 重构 — 执行计划（无 AgentHandle）

**`EventSink` trait 替换 `event_bus` 字段**

```rust
pub trait EventSink: Send + Sync {
    fn emit(&self, event: Event);
}

impl EventSink for EventBusHandle {
    fn emit(&self, event: Event) {
        self.try_send(event).ok();
    }
}

impl EventSink for tokio::sync::mpsc::Sender<Event> {
    fn emit(&self, event: Event) {
        self.try_send(event).ok();
    }
}

pub struct NoOpEventSink;
impl EventSink for NoOpEventSink {
    fn emit(&self, _: Event) {}
}
```

`Agent` 中用 `event_sink: Arc<dyn EventSink>` 替代 `event_bus: EventBusHandle`，所有 `emit_*` 统一为 `self.emit(event)`。主 agent 传入 `EventBusHandle` 实例，子 agent 传入 `mpsc::Sender<Event>` 由 `SubagentTool` 后台转发到父 session。

---

## 设计概要

所有 Agent 共享一个 `InputBus`（全局消息总线）。外部通过 `InputBus::publish(session_id, input)` 驱动 Agent。Agent 启动时 `subscribe` 获取 `mpsc::Receiver<AgentInput>`，loop 中只从这个 receiver 读取。状态变化通过 `EventBus` 广播。`AgentHandle` 完全删除。

## 核心改动

### 1. InputBus（全局消息总线）

```rust
pub struct InputBus {
    subscribers: DashMap<String, mpsc::Sender<AgentInput>>,
    stale_counters: DashMap<String, Arc<AtomicU64>>,
}

impl InputBus {
    pub fn new() -> Self;
    
    /// Agent 启动时调用，注册 session，返回 InputBusHandle
    /// Drop 时自动 unsubscribe，无需显式注销
    pub fn subscribe(&self, session_id: &str) -> InputBusHandle {
        let (tx, rx) = mpsc::channel(20);
        self.subscribers.insert(session_id.to_string(), tx);
        let counter = Arc::new(AtomicU64::new(0));
        self.stale_counters.insert(session_id.to_string(), counter.clone());
        
        InputBusHandle {
            session_id: session_id.to_string(),
            rx,
            counter,
            bus: self.arc(),
        }
    }
    
    /// 外部发送消息。如果 session 不存在，返回 NoSubscriber
    pub fn publish(&self, session_id: &str, input: AgentInput) -> Result<(), InputBusError> {
        let tx = self.subscribers.get(session_id).ok_or(NoSubscriber)?;
        tx.try_send(input).map_err(|_| Closed)
    }
    
    /// 发送用户消息（自动附带 generation）
    pub fn publish_user(&self, session_id: &str, content: Vec<ContentBlock>) -> Result<(), InputBusError> {
        let counter = self.stale_counters.get(session_id).ok_or(NoSubscriber)?;
        let gen = counter.load(Ordering::Relaxed);
        self.publish(session_id, AgentInput::User { content, generation: gen })
    }
    
    /// 发送取消（自动递增 stale counter）
    pub fn cancel(&self, session_id: &str) {
        if let Some(c) = self.stale_counters.get(session_id) {
            c.fetch_add(1, Ordering::Relaxed);
        }
        let _ = self.publish(session_id, AgentInput::Cancel);
    }
    
    pub fn is_alive(&self, session_id: &str) -> bool {
        self.subscribers.contains_key(session_id)
    }
    
    fn unsubscribe(&self, session_id: &str) {
        self.subscribers.remove(session_id);
        self.stale_counters.remove(session_id);
    }
}

pub enum InputBusError { NoSubscriber, Closed }

/// 输入通道句柄，Drop 时自动从 InputBus 注销
pub struct InputBusHandle {
    session_id: String,
    rx: mpsc::Receiver<AgentInput>,
    counter: Arc<AtomicU64>,
    bus: Arc<InputBus>,
}

impl InputBusHandle {
    pub async fn recv(&mut self) -> Option<AgentInput> {
        self.rx.recv().await
    }
    
    pub fn try_recv(&mut self) -> Result<AgentInput, TryRecvError> {
        self.rx.try_recv()
    }
    
    pub fn counter(&self) -> &Arc<AtomicU64> {
        &self.counter
    }
}

impl Drop for InputBusHandle {
    fn drop(&mut self) {
        self.bus.unsubscribe(&self.session_id);
    }
}
```

`InputBus` 管理所有 Agent 的输入 channel 和 generation counter。外部不直接持有 `Sender`，只通过 `session_id` 发消息。`subscribe` 返回 `InputBusHandle`，Drop 时自动注销。

### 2. AgentInput 扩展

```rust
pub enum AgentInput {
    User { content: Vec<ContentBlock>, generation: u64 },
    TaskResult { content: Vec<ContentBlock>, generation: u64 },
    Continue,
    Shutdown,
    Cancel,                    // 新增
    Steer(Vec<ContentBlock>),  // 新增（替代 steer_rx）
    Compact,
    Rewind { message_id: MessageId, target: RewindTarget, result_tx: oneshot::Sender<Result<()>> },
    Clear,
}
```

### 3. Agent 改动

#### 字段删除
- `input_rx`（从 `spawn` 参数传入 `start_loop`，不存 struct）
- `steer_rx`（合并到 `AgentInput::Steer`）
- `event_bus`（替换为 `event_sink: Arc<dyn EventSink>`）

#### 字段变化
```rust
pub struct Agent {
    // ...
    event_sink: Arc<dyn EventSink>,  // 替换 event_bus: EventBusHandle
    // ...
}
```

所有 `emit_*` 方法统一为 `self.emit(event)`：

```rust
impl Agent {
    fn emit(&self, event: Event) {
        self.event_sink.emit(event);
    }

    fn emit_user_message_event(&mut self, id: &MessageId, content: &[ContentBlock]) {
        self.emit(Event::User(UserEvent::Message { ... }));
    }

    fn emit_error(&mut self, phase: ErrorPhase, error: &str, is_recoverable: bool) {
        self.emit(Event::Agent(AgentEvent::Error { ... }));
    }

    // ... 其他 emit_* 同理
}
```

#### spawn 签名

```rust
impl Agent {
    pub async fn spawn(
        id: AgentId,
        shared: &Arc<AgentShared>,
        args: AgentSpawnArgs,
        input_bus: &Arc<InputBus>,
        event_sink: Arc<dyn EventSink>,
    ) {
        let mut handle = input_bus.subscribe(&args.session_id);
        let mut agent = Self::new(id, shared, args, handle.counter().clone(), event_sink).await;
        
        tokio::spawn(async move {
            agent.start_loop(handle).await;
            // handle 在这里 drop，自动 unsubscribe
        });
    }
}
```

`spawn` 不再返回任何东西。外部通过 `InputBus` 和 `EventBus` 与 Agent 交互。`InputBusHandle` 的 `Drop` 自动注销 session，无需显式 `unsubscribe`。

#### start_loop

```rust
async fn start_loop(mut self, mut handle: InputBusHandle) {
    while let Some(input) = handle.recv().await {
        match input {
            AgentInput::User { content, generation } => {
                let current = self.input_stale_since.load(Ordering::Relaxed);
                if generation < current { continue; }
                
                // 读取积压的 steer（合并到同一个 channel）
                let mut steer = Vec::new();
                while let Ok(AgentInput::Steer(blocks)) = handle.try_recv() {
                    steer.extend(blocks);
                }
                
                self.context.transition_to(AgentState::Streaming);
                
                // 注入用户消息 + steer
                let msg = Arc::new(Message::with_blocks(Role::User, content));
                self.message_buffer.push(msg.clone());
                self.emit_user_message_event(&msg.id, &msg.content);
                self.persist_message(&msg).await;
                
                if !steer.is_empty() {
                    let steer_msg = Arc::new(Message::with_blocks(Role::User, steer));
                    self.message_buffer.push(steer_msg.clone());
                    self.emit_user_message_event(&steer_msg.id, &steer_msg.content);
                    self.persist_message(&steer_msg).await;
                }
                
                self.start_turn_if_needed().await;
                
                // 原有 streaming → tool → continue 循环
                let result = self.handle_streaming().await;
                // ... 错误处理、状态转换
                
                self.context.transition_to(AgentState::Idle);
            }
            
            AgentInput::Steer(blocks) => {
                // 单独收到的 steer，当作独立用户消息处理
                self.context.transition_to(AgentState::Streaming);
                let msg = Arc::new(Message::with_blocks(Role::User, blocks));
                self.message_buffer.push(msg.clone());
                self.emit_user_message_event(&msg.id, &msg.content);
                self.persist_message(&msg).await;
                
                self.start_turn_if_needed().await;
                let _ = self.handle_streaming().await;
                self.context.transition_to(AgentState::Idle);
            }
            
            AgentInput::Cancel => {
                self.cancel_token.cancel();
                // 取消后 continue 等待下一个 input，
                // 当前正在执行的 handle_streaming 会检查 cancel_token
            }
            
            AgentInput::Shutdown => {
                if let Some(turn) = self.current_turn.take() {
                    turn.cancel().await.ok();
                }
                self.context.transition_to(AgentState::Closed);
                break;
            }
            
            AgentInput::Continue => {
                self.context.transition_to(AgentState::Streaming);
                let msg = Arc::new(Message::user("continue"));
                self.message_buffer.push(msg.clone());
                self.emit_user_message_event(&msg.id, &msg.content);
                self.persist_message(&msg).await;
                
                self.start_turn_if_needed().await;
                let _ = self.handle_streaming().await;
                self.context.transition_to(AgentState::Idle);
            }
            
            AgentInput::TaskResult { content, generation } => {
                let current = self.input_stale_since.load(Ordering::Relaxed);
                if generation < current { continue; }
                
                self.context.transition_to(AgentState::Streaming);
                let msg = Arc::new(Message::with_blocks(Role::User, content));
                self.message_buffer.push(msg.clone());
                self.emit_user_message_event(&msg.id, &msg.content);
                self.persist_message(&msg).await;
                
                self.start_turn_if_needed().await;
                let _ = self.handle_streaming().await;
                self.context.transition_to(AgentState::Idle);
            }
            
            AgentInput::Compact => {
                let _ = self.force_full_compact().await;
            }
            
            AgentInput::Rewind { message_id, target, result_tx } => {
                let _ = self.process_rewind(message_id, target, result_tx).await;
            }
            
            AgentInput::Clear => {
                self.handle_clear().await;
            }
        }
    }
    
    // input_rx 关闭，Agent 退出
    // 发送 Stopped 事件到 EventBus，通知等待者（如 SubagentTool）
    let _ = self.event_bus.try_send(Event::Agent(AgentEvent::Lifecycle {
        agent_id: self.id.clone(),
        state: AgentStatus::Stopped,
    }));
    
    // 取消当前 turn 的 checkpoint
    if let Some(turn) = self.current_turn.take() {
        turn.cancel().await.ok();
    }
    
    self.context.transition_to(AgentState::Closed);
    // handle 在这里 drop，自动从 InputBus 注销
}
```
```

**关键点**：`start_loop` 不再用 `select!`，而是简单的 `while let Some(input) = input_rx.recv().await`。所有消息（包括 steer）都走 `AgentInput`。

`handle_streaming` 中的 `steer_rx` 读取代码删除（因为 steer 已经在 `User` 分支通过 `try_recv` 读取）。

`handle_streaming` 仍然可以检查 `cancel_token`（因为 `AgentInput::Cancel` 会设置它）。

### 4. 删除 AgentHandle

`AgentHandle` 完全删除。外部交互方式：

```rust
// 发送用户消息
input_bus.publish_user(&session_id, content);

// 取消
input_bus.cancel(&session_id);

// 关闭
input_bus.publish(&session_id, AgentInput::Shutdown);

// steer
input_bus.publish(&session_id, AgentInput::Steer(blocks));

// 强制压缩
input_bus.publish(&session_id, AgentInput::Compact);

// 状态查询（通过 EventBus）
let mut rx = event_bus.subscribe(SessionId(session_id));
// 监听 AgentEvent::Lifecycle 获取状态变化
```

### 5. Session 改动

```rust
pub struct Session {
    id: SessionId,
    agent_id: AgentId,
    shared: Arc<AgentShared>,
    // 删除 handles: Arc<RwLock<HashMap<AgentId, AgentHandle>>>
    // 新增：
    input_bus: Arc<InputBus>,
    // ...
}

impl Session {
    pub async fn spawn(self: Arc<Self>, args: AgentSpawnArgs) -> Result<()> {
        let agent_id = AgentId::new();
        self.agent_id.store(Some(agent_id.clone()));
        
        Agent::spawn(agent_id, &self.shared, args, &self.input_bus).await;
        Ok(())
    }
    
    pub async fn send_message(&self, content: Vec<ContentBlock>) -> Result<()> {
        self.input_bus.publish_user(&self.id.0, content)
            .map_err(|e| anyhow!("Agent not alive: {:?}", e))
    }
    
    pub async fn cancel(&self) {
        self.input_bus.cancel(&self.id.0);
    }
    
    pub async fn close(&self) {
        self.input_bus.publish(&self.id.0, AgentInput::Shutdown).ok();
    }
    
    pub async fn steer(&self, blocks: Vec<ContentBlock>) -> Result<()> {
        self.input_bus.publish(&self.id.0, AgentInput::Steer(blocks))
            .map_err(|e| anyhow!("Agent not alive: {:?}", e))
    }
    
    pub async fn force_compact(&self) {
        self.input_bus.publish(&self.id.0, AgentInput::Compact).ok();
    }
    
    pub async fn rewind(&self, message_id: MessageId, target: RewindTarget) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.input_bus.publish(&self.id.0, AgentInput::Rewind { message_id, target, result_tx: tx })
            .map_err(|e| anyhow!("Agent not alive: {:?}", e))?;
        rx.await??;
        Ok(())
    }
    
    pub async fn clear(&self) {
        self.input_bus.publish(&self.id.0, AgentInput::Clear).ok();
    }
    
    pub fn is_agent_running(&self) -> bool {
        self.input_bus.is_alive(&self.id.0)
    }
    
    pub async fn send_continue(&self) {
        self.input_bus.publish(&self.id.0, AgentInput::Continue).ok();
    }
    
    pub async fn send_task_result(&self, content: Vec<ContentBlock>) {
        self.input_bus.publish_user(&self.id.0, content).ok();
    }
    
    pub async fn send_permission_response(&self, request_id: u64, approved: bool, mode: PermissionMode) {
        // 权限响应通过 InputBus 发送给 Agent
        // 需要新增 AgentInput::PermissionResponse 或保持现有机制
        // 现有 Checker 的 respond callback 是 oneshot::Sender，不依赖 AgentHandle
        // 保持现有机制：Checker 内部持有 oneshot channel，不经过 InputBus
        // 所以不需要改
    }
    
    pub async fn ask_user_response(&self, ask_id: String, response: Vec<ContentBlock>) {
        // 保持现有机制（通过 response_map 的 oneshot channel）
    }
}
```

### 6. KernelServer 改动

```rust
impl KernelServer {
    async fn handle_agent_message(&self, payload: AgentMessagePayload) -> Result<InvokeResponse> {
        let session = self.get_or_create_session(payload).await;
        
        match payload.message_type {
            AgentMessageType::User => {
                session.send_message(vec![ContentBlock::Text { text: payload.content }]).await?;
            }
            AgentMessageType::Steer => {
                session.steer(parse_content_blocks(&payload.content)).await?;
            }
            AgentMessageType::Continue => {
                session.send_continue().await;
            }
            AgentMessageType::Shutdown => {
                session.close().await;
            }
            AgentMessageType::Cancel => {
                session.cancel().await;
            }
            AgentMessageType::Compact => {
                session.force_compact().await;
            }
            AgentMessageType::Rewind => {
                session.rewind(...).await?;
            }
            AgentMessageType::Clear => {
                session.clear().await;
            }
            _ => {}
        }
        
        Ok(InvokeResponse::success())
    }
}
```

### 7. SubagentTool 重构

```rust
impl Tool for SubagentTool {
    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let task = parse_task(&args)?;
        let preset = parse_preset(&args)?;
        
        // 1. 创建子 session
        let subsession_id = self.session_store.create_subsession(&ctx.session_id).await?;
        
        // 2. 构建子 agent 的 spawn args
        let mut spawn_args = AgentSpawnArgs::new(
            build_subagent_prompt(&preset, &ctx),
            subsession_id.clone(),
        );
        if self.inherit_context {
            spawn_args = spawn_args.with_history(parent_history(&ctx));
        }
        spawn_args = spawn_args
            .with_max_iterations(self.max_iterations)
            .with_subagent(false)
            .with_tool_blocklist(self.disallowed_tools.clone())
            .with_skills(self.skills.clone())
            .with_working_dir(ctx.working_dir.clone())
            .with_max_tool_output_length(self.max_tool_output_length)
            .with_allow_command_hooks(false);
        
        // 3. 创建 EventSink：子 agent 事件 → channel → 父 session EventBus
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(100);
        let event_sink = Arc::new(event_tx) as Arc<dyn EventSink>;
        
        let parent_bus = self.event_bus.handle(SessionId(ctx.session_id.to_string()));
        let forward_handle = tokio::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                let _ = parent_bus.try_send(event);
            }
        });
        
        // 4. 创建子 agent（使用 EventSink，不直接绑定 EventBus）
        let mut agent = Agent::create(
            AgentId::new(),
            &self.shared,
            spawn_args,
            event_sink,
        ).await;
        
        // 5. 直接执行 turn（streaming + tool + continue 全部走 Agent 核心路径）
        agent.execute_turn(
            vec![ContentBlock::Text { text: task }],
            vec![], // 无 steer
        ).await.map_err(|e| anyhow!("Subagent failed: {}", e))?;
        
        // 停止转发（释放 event_sink 后 channel 关闭，forward 自然退出）
        drop(agent);
        let _ = forward_handle.await;
        
        // 6. 从 message_buffer 收集结果
        let result = format_subagent_result(agent.message_buffer.messages());
        
        Ok(ToolOutput::text(result))
    }
}

fn format_subagent_result(messages: &[Arc<Message>]) -> String {
    let mut result = String::new();
    for msg in messages.iter().skip(1) { // skip system prompt
        if msg.role == Role::Assistant {
            for block in &msg.content {
                match block {
                    ContentBlock::Text { text } => result.push_str(text),
                    ContentBlock::Thinking { thinking } => result.push_str(thinking),
                    _ => {}
                }
            }
        }
    }
    result
}
```

**子 agent 与主 agent 完全一致**：
- 走 `Agent::spawn`（相同的启动路径）
- 走 `InputBus`（相同的输入接口）
- 走 `EventBus`（相同的事件输出）
- 自动获得并行工具、hooks、compaction

### 8. App / 全局 InputBus

```rust
pub struct App {
    event_bus: EventBus,
    input_bus: Arc<InputBus>,
    // ...
}

impl App {
    pub fn new(...) -> Self {
        Self {
            event_bus: EventBus::new(),
            input_bus: Arc::new(InputBus::new()),
            // ...
        }
    }
}
```

`InputBus` 在 `App` 创建时初始化，和 `EventBus` 一样全局共享。传递给 `Session` 和 `SubagentTool`。

### 9. 删除 SimpleAgent

```bash
rm crates/kernel/src/agent/simple.rs
```

从 `agent/mod.rs` 移除 `pub mod simple;`

---

## 执行任务表

### Phase 1: InputBus 基础设施（3 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 1.1 | 新 `event_bus/input_bus.rs` | 定义 `InputBus` struct + `InputBusHandle`（Drop 自动 unsubscribe），实现 `subscribe`/`publish`/`publish_user`/`cancel`/`is_alive` |
| 1.2 | `event_bus/mod.rs` | 导出 `InputBus` |
| 1.3 | — | 写 `InputBus` 单元测试（subscribe/publish/cancel/unsubscribe 基本流程） |

### Phase 2: AgentInput 扩展（1 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 2.1 | `agent/types.rs` | 在 `AgentInput` 添加 `Cancel` 和 `Steer(Vec<ContentBlock>)` 变体 |

### Phase 3: Agent 接入 InputBus + EventSink（6 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 3.1 | `agent/agent.rs` | 从 `Agent` struct 删除 `input_rx` 和 `steer_rx` 字段，`event_bus` 替换为 `event_sink: Arc<dyn EventSink>` |
| 3.2 | `agent/agent.rs` | 修改 `Agent::new`：接收 `Arc<AtomicU64>`（`input_stale_since` 由 `InputBusHandle::counter()` 提供）和 `Arc<dyn EventSink>`（替换 `EventBusHandle`） |
| 3.3 | `agent/agent.rs` | 统一 `emit_*` 方法为 `self.emit(event)`，所有事件通过 `EventSink` 发送 |
| 3.4 | `agent/agent.rs` | 修改 `Agent::spawn`：接收 `&Arc<InputBus>` 和 `Arc<dyn EventSink>`，调用 `input_bus.subscribe` 获取 `InputBusHandle`，构造 `Agent`（传入 `handle.counter().clone()` + `event_sink`），spawn `start_loop(handle)`。**不返回任何值，无需显式 unsubscribe** |
| 3.5 | `agent/agent.rs` | 重写 `start_loop`：从 `while let Some(input) = handle.recv().await` 读取，处理所有 `AgentInput` 变体（包括 `Cancel` 和 `Steer`）。`User` 分支中 `try_recv` 读取积压的 `Steer`。删除 `select!` 和 `steer_rx` 的 select |
| 3.6 | `agent/agent.rs` | 从 `handle_streaming` 中删除 `steer_rx` 读取代码（steer 已在 `start_loop` 的 `User` 分支处理） |

### Phase 4: 删除 AgentHandle（4 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 4.1 | 删除 `agent/handle.rs` | 删除整个文件 |
| 4.2 | `agent/mod.rs` | 移除 `pub mod handle;` 和 `pub use handle::AgentHandle;` |
| 4.3 | `agent/types.rs` | 删除 `AgentHandle` 相关类型（如果存在） |
| 4.4 | — | 编译检查 `kernel` crate，修复所有引用 `AgentHandle` 的代码 |

### Phase 5: Session 重构（5 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 5.1 | `app/session.rs` | 从 `Session` struct 删除 `handles` 字段，添加 `input_bus: Arc<InputBus>` 字段 |
| 5.2 | `app/session.rs` | 修改 `Session::create`：接收 `input_bus` 参数并存储 |
| 5.3 | `app/session.rs` | 修改 `Session::spawn`：删除 `AgentHandle` 获取和存储，直接调用 `Agent::spawn(..., &self.input_bus, event_sink)`（主 agent 传入 `EventBusHandle` 实现的 `EventSink`） |
| 5.4 | `app/session.rs` | 重写所有 `Session` 方法（`send_message`, `cancel`, `close`, `steer`, `force_compact`, `rewind`, `clear`, `is_agent_running`, `send_continue`, `send_task_result`）为直接调用 `self.input_bus` 的方法 |
| 5.5 | `app/session.rs` | 删除 `send_permission_response` 和 `ask_user_response` 的 `AgentHandle` 调用（权限响应通过现有 `Checker` 的 `respond` callback 机制，不依赖 AgentHandle） |

### Phase 6: Server 重构（2 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 6.1 | `server/mod.rs` | 修改 `handle_agent_message`：通过 `session.input_bus` 或 `session` 的方法发送消息，删除所有 `AgentHandle` 引用 |
| 6.2 | `server/mod.rs` | 修改 `handle_permission_response` 和 `handle_ask_user_response`：通过现有机制（Checker 的 respond callback / ask_user_map 的 oneshot）发送，不依赖 AgentHandle |

### Phase 7: EventSink 基础设施（2 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 7.1 | 新 `event_bus/event_sink.rs` | 定义 `EventSink` trait + `EventBusHandle` 实现 + `mpsc::Sender<Event>` 实现 + `NoOpEventSink` |
| 7.2 | `event_bus/mod.rs` | 导出 `EventSink` |

### Phase 8: App 集成 InputBus（2 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 8.1 | `app/mod.rs` | `App` struct 添加 `input_bus: Arc<InputBus>`，在 `App::new` 中创建 |
| 8.2 | `app/session.rs` | `Session::create` 传入 `input_bus` 参数，传给 `Agent::spawn`；创建 `EventSink`（`EventBusHandle` 实例）传给 `Agent::spawn` |

### Phase 9: SubagentTool 重构（3 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 9.1 | `tools/subagent.rs` | 从 `SubagentTool` struct 删除 `event_bus` 字段（不再需要直接持有），添加 `input_bus: Arc<InputBus>` 字段 |
| 9.2 | `tools/subagent.rs` | 重写 `exec`：创建 `mpsc::channel` 作为 `EventSink`，后台转发到父 session；直接调用 `Agent::create` + `execute_turn`；从 `message_buffer` 收集结果 |
| 9.3 | `tools/factory.rs` | 修改 `SubagentTool` 的构造参数，传入 `input_bus` |

### Phase 10: 删除 SimpleAgent（1 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 10.1 | `agent/mod.rs` + `agent/simple.rs` | 删除 `simple.rs`，从 `mod.rs` 移除 `pub mod simple;` |

### Phase 11: 编译与测试（4 个任务）

| # | 内容 |
|---|------|
| 11.1 | `cargo build -p kernel` 编译通过 |
| 11.2 | `cargo clippy -p kernel -p cli -p tui` 无警告 |
| 11.3 | `cargo test -p kernel --lib` 全部通过（380+ 测试） |
| 11.4 | 手动测试：主 agent 对话、子 agent 调用、进度上报、cancel、steer、权限检查 |

---

**总计：33 个任务，分 11 个 Phase。**

## 关键风险与缓解

| 风险 | 缓解 |
|------|------|
| `publish` 时 session 不存在 | `Agent::spawn` 先 `subscribe` 再启动 loop，确保 subscriber 存在。`SubagentTool` 先 `spawn` 再 `publish` |
| 权限响应不经过 InputBus | 权限响应走现有 `Checker` 的 `respond` callback（`oneshot::Sender`），不依赖 AgentHandle，无需改动 |
| 子 agent 的 cancel | 通过 `input_bus.cancel(&subsession_id)`，和主 agent 一致 |
| 子 agent 事件转发到父 session | `SubagentTool` 在 `exec` 中显式订阅子 session 的 EventBus 并转发到父 session |
| 取消后旧消息的 generation | `InputBus` 的 `stale_counters` 管理，和现有 `AgentHandle::cancel` 行为一致 |
| 编译失败（大量 AgentHandle 引用） | 分 Phase 4 专门处理，先删除 AgentHandle 再修复所有引用 |
| 测试依赖 AgentHandle | 测试中直接 `InputBus::publish` 或使用 `Session` 的辅助方法 |

## 设计验证

- 主 agent 启动：`App` 创建 `InputBus` → `Session::create` 传入 → `Agent::spawn` subscribe → `start_loop` 读取
- 主 agent 发送消息：`Session::send_message` → `InputBus::publish_user` → `Agent` 的 `start_loop` 收到 `User` → `handle_streaming` → 事件到 `EventBus`
- 子 agent 启动：`SubagentTool::exec` → `Agent::spawn` + `subscribe` → `publish_user` 发送任务 → `start_loop` 处理
- 子 agent 进度：子 agent 事件 → `EventBus`（子 session）→ `SubagentTool` 的 `forward_handle` 转发 → `EventBus`（父 session）→ TUI 显示
- 子 agent 完成：`Agent::create` + `execute_turn` 同步返回 → 从 `message_buffer` 收集结果 → `event_sink`（`mpsc::Sender`）drop 后 channel 关闭 → 后台转发任务自然退出
- 取消：`InputBus::cancel` → 递增 `stale_counters` + `publish(Cancel)` → `Agent` 收到 `Cancel` → `cancel_token.cancel()` → `handle_streaming` 检查 → 优雅退出
- steer：`InputBus::publish(Steer)` → `Agent` 的 `start_loop` 中 `User` 分支 `try_recv` 读取积压 steer → 合并到用户消息后处理
