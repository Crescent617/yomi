# Yomi InputBus 架构方案

## 核心洞察

用户的核心诉求：所有 Agent 共享一个统一的消息总线，Agent 变成**被动的消息消费者**，通过监听总线上的事件来驱动执行。不需要 `AgentHandle`（每个 Agent 的交互都走总线）。

这本质上是从"显式 channel 传递"变为"pub/sub 消息总线"，类似 loopy 的 `Coordinator` + `Mailbox` 设计。

## 问题分析

### 当前问题

`SubagentTool` 同步执行 `SimpleAgent`，父 agent 通过 `event_bus` 的 `Progress` 事件收到子 agent 的实时进度。但子 agent 的 `event_bus` 是子 session 的，父 agent 的 TUI 订阅的是父 session，所以父 agent 看不到子 agent 的进度。

之前 `progress_tx` 方案是 patch，InputBus 是更根本的架构。

### 同步 vs 异步的根本问题

`Tool::exec` 必须返回 `ToolOutput`，所以 `SubagentTool` 必须**同步等待**子 agent。无论用 channel 还是 InputBus，这个约束不变。

InputBus 的价值不在于让子 agent 异步，而在于：
1. 统一所有 Agent 的输入接口（外部驱动、子 agent 创建）
2. 子 agent 可以通过 EventBus 发通知，父 agent 的 listener 可以转发到父 session
3. 不需要 `AgentHandle` 持有 `input_tx` — 外部知道 `session_id` 就能发消息

## 架构设计

### 1. InputBus

```rust
/// 全局输入总线，session_id -> mpsc::Sender<AgentInput>
pub struct InputBus {
    subscribers: DashMap<String, mpsc::Sender<AgentInput>>,
}

impl InputBus {
    pub fn new() -> Self;
    
    /// 注册 session，返回 receiver（Agent 使用）
    pub fn subscribe(&self, session_id: &str) -> mpsc::Receiver<AgentInput>;
    
    /// 往 session 发送消息。如果 session 不存在，返回 NoSubscriber
    pub fn publish(&self, session_id: &str, input: AgentInput) -> Result<(), InputBusError>;
    
    /// 注销 session
    pub fn unsubscribe(&self, session_id: &str);
    
    /// 检查 session 是否活跃
    pub fn is_alive(&self, session_id: &str) -> bool;
}

pub enum InputBusError {
    NoSubscriber, // session 没有活跃的 Agent
    Closed,       // channel 已关闭
}
```

Tokio `mpsc::Sender` 是 `Clone` 的，允许多个 publisher 同时往同一个 session 发消息。

### 2. 修改 AgentInput

```rust
pub enum AgentInput {
    User { content: Vec<ContentBlock>, generation: u64 },
    TaskResult { content: Vec<ContentBlock>, generation: u64 },
    Continue,
    Shutdown,
    Cancel,           // 新增：取消当前执行
    Steer(Vec<ContentBlock>), // 新增：steer 消息（不再走单独的 steer_rx）
    Compact,
    Rewind { message_id: MessageId, target: RewindTarget, result_tx: oneshot::Sender<Result<()>> },
    Clear,
}
```

`steer_rx` 被移除，steer 统一走 `AgentInput::Steer`。

### 3. Agent 改为从 InputBus 接收

```rust
pub struct Agent {
    // 删除 input_rx 和 steer_rx
    // 保留 session_id（用于 self-publish 和 event 发送）
    // 保留 input_bus 引用（用于给子 agent 发消息等）
    input_bus: Arc<InputBus>,
    // ...
}

impl Agent {
    /// 注册到 InputBus 并启动 loop
    pub async fn spawn(
        session_id: &str,
        input_bus: Arc<InputBus>,
        shared: Arc<AgentShared>,
        args: AgentSpawnArgs,
    ) -> Result<(), AgentError> {
        let rx = input_bus.subscribe(session_id);
        let mut agent = Self::create(session_id, input_bus, shared, args, rx).await;
        
        tokio::spawn(async move {
            agent.start_loop().await;
            agent.input_bus.unsubscribe(session_id);
        });
        
        Ok(())
    }
    
    async fn start_loop(mut self) {
        while let Some(input) = self.input_rx.recv().await {
            match input {
                AgentInput::User { content, generation } => {
                    // generation fencing
                    let current = self.input_stale_since.load(Ordering::Relaxed);
                    if generation < current { continue; }
                    
                    // 读取 steer（从 InputBus 的积压消息）
                    let mut steer = Vec::new();
                    while let Ok(AgentInput::Steer(blocks)) = self.input_rx.try_recv() {
                        steer.extend(blocks);
                    }
                    
                    self.context.transition_to(AgentState::Streaming);
                    let result = self.execute_turn(content, steer).await;
                    // ... 错误处理
                    self.context.transition_to(AgentState::Idle);
                }
                AgentInput::Steer(blocks) => {
                    // 作为独立用户消息处理
                    self.context.transition_to(AgentState::Streaming);
                    let _ = self.execute_turn(blocks, vec![]).await;
                    self.context.transition_to(AgentState::Idle);
                }
                AgentInput::Continue => { ... }
                AgentInput::TaskResult { content, .. } => { ... }
                AgentInput::Cancel => {
                    self.cancel_token.cancel();
                    // 等待 execute_turn 返回（因为 execute_turn 检查 cancel_token）
                }
                AgentInput::Shutdown => {
                    if let Some(turn) = self.current_turn.take() {
                        turn.cancel().await.ok();
                    }
                    self.context.transition_to(AgentState::Closed);
                    break;
                }
                AgentInput::Compact => { ... }
                AgentInput::Rewind { ... } => { ... }
                AgentInput::Clear => { ... }
            }
        }
        
        // input_rx 关闭（所有 sender dropped），Agent 退出
        self.context.transition_to(AgentState::Closed);
    }
}
```

### 4. AgentHandle 变为 thin wrapper（可选保留）

```rust
/// 可选：保留 AgentHandle 作为 convenience，但内部不再持有 channel
pub struct AgentHandle {
    input_bus: Arc<InputBus>,
    session_id: String,
    state_rx: broadcast::Receiver<AgentState>,
    cancel_token: CancelToken,
    input_stale_since: Arc<AtomicU64>,
}

impl AgentHandle {
    pub fn send_message(&self, content: Vec<ContentBlock>) -> Result<(), InputBusError> {
        let gen = self.input_stale_since.load(Ordering::Relaxed);
        self.input_bus.publish(&self.session_id, AgentInput::User { content, generation: gen })
    }
    
    pub fn cancel(&self) {
        self.input_bus.publish(&self.session_id, AgentInput::Cancel).ok();
    }
    
    pub fn close(&self) {
        self.input_bus.publish(&self.session_id, AgentInput::Shutdown).ok();
    }
    
    pub fn steer(&self, blocks: Vec<ContentBlock>) -> Result<(), InputBusError> {
        self.input_bus.publish(&self.session_id, AgentInput::Steer(blocks))
    }
    
    // state_rx 仍然用于监听状态变化（从 EventBus 订阅或保留独立 channel）
}
```

**AgentHandle 变成完全 optional**。外部可以直接 `InputBus::publish`。

### 5. SubagentTool 在 InputBus 架构下的实现

```rust
impl Tool for SubagentTool {
    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let task = parse_task(&args)?;
        let preset = parse_preset(&args)?;
        
        // 1. 创建子 session
        let subsession_id = self.session_store.create_subsession(&ctx.session_id).await?;
        
        // 2. 构建子 agent 的 spawn args
        let spawn_args = ...;
        
        // 3. 创建子 agent（注册到 InputBus）
        //    子 agent 的 event_bus 指向子 session
        Agent::spawn(&subsession_id, self.input_bus.clone(), self.shared.clone(), spawn_args).await?;
        
        // 4. 发送任务给子 agent
        self.input_bus.publish(&subsession_id, AgentInput::User {
            content: vec![ContentBlock::Text { text: task }],
            generation: 0,
        }).map_err(|e| anyhow!("Failed to publish to subagent: {:?}", e))?;
        
        // 5. 同步等待子 agent 完成
        //    方案：在 EventBus 上订阅子 session 的 Stopped 事件
        let result = self.wait_for_subagent(&subsession_id).await?;
        
        Ok(result)
    }
    
    async fn wait_for_subagent(&self, subsession_id: &str) -> Result<ToolOutput> {
        // 在 EventBus 上订阅子 session 的 AgentEvent::Lifecycle(Stopped)
        let mut rx = self.event_bus.subscribe(SessionId(subsession_id.to_string()));
        
        // 同时监听子 agent 的 progress 事件（AgentEvent 的所有事件）
        // 并转发到父 session 的 event_bus（这样 TUI 能看到）
        let parent_bus = self.event_bus.handle(SessionId(self.parent_session_id.clone()));
        let mut progress_rx = self.event_bus.subscribe(SessionId(subsession_id.to_string()));
        
        let forward_handle = tokio::spawn(async move {
            while let Ok(event) = progress_rx.recv().await {
                // 转发子 agent 的事件到父 session（作为 Progress 事件）
                let _ = parent_bus.try_send(Event::Tool(ToolEvent::Progress {
                    id: subsession_id.to_string(),
                    output: event_to_text(&event),
                }));
            }
        });
        
        // 等待 Stopped 事件
        while let Ok(event) = rx.recv().await {
            if let Event::Agent(AgentEvent::Lifecycle { state: AgentStatus::Stopped, .. }) = event {
                break;
            }
        }
        
        forward_handle.abort();
        
        // 从子 session 的 message_store 读取结果
        let messages = self.message_store.get_messages(subsession_id).await?;
        let result = format_subagent_result(&messages);
        
        Ok(ToolOutput::text(result))
    }
}
```

### 6. 子 agent 通知父 agent 的机制

子 agent 在退出时自动发送 `AgentEvent::Lifecycle(Stopped)` 到 EventBus。父 agent 的 `wait_for_subagent` 订阅这个事件。

如果子 agent 需要向父 agent 发送中间结果（如 checkpoint 完成），可以通过 EventBus 发送自定义事件。

## 对比：InputBus vs progress_tx

| 维度 | progress_tx 方案 | InputBus 方案 |
|------|-----------------|--------------|
| 改动量 | 8 个任务 | 25+ 个任务 |
| 架构影响 | 最小（Agent 加字段） | 中等（引入全局总线） |
| 子 agent 进度 | 通过 progress_tx 转发 | 通过 EventBus 订阅 + 转发 |
| 外部驱动 Agent | 仍需要 AgentHandle | 知道 session_id 即可 |
| steer 机制 | 单独 steer_rx | 统一走 InputBus::Steer |
| 取消机制 | cancel_token | cancel_token + InputBus::Cancel |
| 生命周期管理 | AgentHandle drop | InputBus unsubscribe |
| 未来扩展（多 agent 协作） | 受限 | 天然支持 |

## 关键问题

### 1. 没有 subscriber 怎么办？

`InputBus::publish` 返回 `NoSubscriber`。这表示 session 没有活跃的 Agent。调用方（如 TUI）需要处理这个错误（可能是 session 已关闭）。

对于 SubagentTool：创建子 agent 时先 `spawn`，确保 subscriber 已注册，再 `publish`。这消除了竞态。

### 2. Agent 的创建顺序

当前 `Agent::spawn` 是 async 的（因为构建 system_prompt 需要异步）。`InputBus::subscribe` 是同步的。所以顺序是：
1. `subscribe` 创建 channel（此时 InputBus 上有 subscriber）
2. `Agent::create` 构建 Agent（异步）
3. `spawn` 启动 loop（异步）

这没问题。如果 `publish` 在第 1 步和第 3 步之间发生，消息会缓冲在 channel 中。

### 3. AgentHandle 是否保留？

建议**保留但变薄**：AgentHandle 内部只持有 `input_bus` + `session_id`，作为 convenience API。不强制使用。这保持向后兼容。

### 4. 子 agent 的同步等待

`SubagentTool::exec` 仍然同步等待子 agent。InputBus 不改变这个模型。改变的是：子 agent 的创建和消息发送通过统一接口。

### 5. 取消语义

`AgentInput::Cancel` 发送后，Agent 的 `execute_turn` 检查 `cancel_token`，优雅退出。这比 `AgentHandle::cancel`（直接调用 `cancel_token.cancel()`）多了一层消息传递，但语义一致。

## 执行计划（22 个任务）

### Phase 1: InputBus 基础设施（3 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 1.1 | `event_bus/input_bus.rs`（新文件） | 定义 `InputBus` struct + `subscribe` / `publish` / `unsubscribe` / `is_alive` |
| 1.2 | `event_bus/mod.rs` | 导出 `InputBus` |
| 1.3 | — | 测试 `InputBus` 基本功能（publish/subscribe/unsubscribe） |

### Phase 2: AgentInput 扩展（2 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 2.1 | `agent/types.rs` | 新增 `AgentInput::Cancel` 和 `AgentInput::Steer(Vec<ContentBlock>)` |
| 2.2 | `agent/agent.rs` | `start_loop` 的 `match` 添加 `Cancel` 和 `Steer` 处理 |

### Phase 3: Agent 接入 InputBus（5 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 3.1 | `agent/agent.rs` | 从 `Agent` struct 移除 `input_rx` 和 `steer_rx` 字段 |
| 3.2 | `agent/agent.rs` | 新增 `input_bus: Arc<InputBus>` 字段 |
| 3.3 | `agent/agent.rs` | 修改 `Agent::create`：接收 `input_bus` 和 `mpsc::Receiver<AgentInput>`（从 InputBus.subscribe 获取） |
| 3.4 | `agent/agent.rs` | 修改 `Agent::spawn`：调用 `input_bus.subscribe` 获取 rx，然后 `create` + 启动 loop；loop 退出时 `unsubscribe` |
| 3.5 | `agent/agent.rs` | `start_loop` 的 `select!` 简化：只从 `input_rx` 读取，不再 select `steer_rx`；`Steer` 通过 `AgentInput::Steer` 处理 |

### Phase 4: AgentHandle 变薄（3 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 4.1 | `agent/handle.rs` | 移除 `input_tx` 和 `steer_tx` 字段，改为 `input_bus: Arc<InputBus>` + `session_id: String` |
| 4.2 | `agent/handle.rs` | `send_message` / `send_text` / `send_ask_user_response` / `send_permission_response` 改为 `input_bus.publish(...)` |
| 4.3 | `agent/handle.rs` | `cancel` 改为 `input_bus.publish(session_id, AgentInput::Cancel)`；`close` 改为 `AgentInput::Shutdown`；`steer` 改为 `AgentInput::Steer`；`force_compact` 改为 `AgentInput::Compact`；`rewind` 改为 `AgentInput::Rewind`；`clear` 改为 `AgentInput::Clear`；`send_continue` 改为 `AgentInput::Continue`；`send_task_result` 改为 `AgentInput::TaskResult` |

### Phase 5: 全局 InputBus 集成（4 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 5.1 | `app/session.rs` | `Session::create` 创建 `InputBus` 实例并传给 `Agent::spawn` |
| 5.2 | `app/session.rs` | `Session::spawn` 中，`Agent::spawn` 参数添加 `input_bus` |
| 5.3 | `server/mod.rs` | 修改 `start_agent` 和 `send_message`：通过 `session` 获取 `input_bus`，直接 `publish` 或调用 `AgentHandle`（如果保留） |
| 5.4 | `app/coordinator.rs` | 如果需要，添加 `InputBus` 的持有和生命周期管理 |

### Phase 6: SubagentTool 重构（4 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 6.1 | `tools/subagent.rs` | 从 `SubagentTool` struct 移除 `event_bus` 字段（不再需要直接持有） |
| 6.2 | `tools/subagent.rs` | 新增 `input_bus: Arc<InputBus>` 字段 |
| 6.3 | `tools/subagent.rs` | `exec`：创建子 session → `Agent::spawn`（传入 `input_bus`）→ `input_bus.publish` 发送任务 → `wait_for_subagent` 同步等待 |
| 6.4 | `tools/subagent.rs` | `wait_for_subagent`：订阅 EventBus 的子 session 事件，转发 Progress 到父 session，等待 Stopped 事件，从 message_store 读取结果 |

### Phase 7: 删除 SimpleAgent（1 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 7.1 | `agent/mod.rs` + `agent/simple.rs` | 删除 `simple.rs`，从 `mod.rs` 移除导出 |

### Phase 8: 编译与验证（4 个任务）

| # | 内容 |
|---|------|
| 8.1 | `cargo build -p kernel` 编译通过 |
| 8.2 | `cargo clippy -p kernel -p cli -p tui` 无警告 |
| 8.3 | `cargo test -p kernel --lib` 全部通过 |
| 8.4 | 手动测试：主 agent 对话、子 agent 调用、进度上报、取消、steer |

## 风险

| 风险 | 影响 | 缓解 |
|------|------|------|
| `publish` 时 session 不存在 | 中 | 先 `spawn` 再 `publish`，SubagentTool 的 `wait_for_subagent` 处理 `NoSubscriber` 错误 |
| steer 从单独 channel 移到 InputBus 的积压消息 | 中 | `start_loop` 的 `User` 分支处理 `try_recv` 读取所有 `Steer` 消息 |
| AgentHandle 的 `state_rx` 来源 | 低 | 保留 `state_rx` 从 `AgentExecutionContext` 获取，与 InputBus 无关 |
| 并发 publish 到同一 session | 低 | Tokio mpsc Sender 是 Clone 的，并发安全 |
| 子 agent 的 EventBus 事件转发 | 中 | 在 `wait_for_subagent` 中显式转发，注意事件类型映射 |
| 测试依赖 AgentHandle 的 input_tx | 中 | 测试中改用 `InputBus::publish` 或直接构造 `Agent` |
