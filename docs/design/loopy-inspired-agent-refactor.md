# Loopy-Inspired Agent Architecture Refactor

## 背景：为什么研究 Loopy

Yomi 的 `Agent`（主 agent，带完整生命周期）和 `SimpleAgent`（子 agent，简化实现）存在**核心执行逻辑重复**的问题：
- `SimpleAgent` 串行执行工具，无 hooks，无 compaction，无权限检查交互
- 新增功能（如 `max_tool_output_length` 配置化）需要在 `Agent` 和 `SimpleAgent` 两处修改
- 事件发送是硬编码的 `event_bus.try_send(...)`，耦合到具体执行路径中

Loopy 是 managed-agents 项目中的纯 Go Agent SDK，其核心设计为这个问题提供了一个清晰的参考方向。

---

## Loopy 核心设计逻辑分析

### 1. 架构分层：三层职责清晰

```
┌─────────────────────────────────────────────────────┐
│  Coordinator  │  生命周期管理：心跳、唤醒、调度、子 agent  │
│  (薄控制面)     │  不负责具体执行逻辑                       │
├─────────────────────────────────────────────────────┤
│  Loop           │  执行引擎：拉消息 → 执行 turn → 抽干即死  │
│  (纯引擎)       │  无状态机、无生命周期、无外部事件发送      │
├─────────────────────────────────────────────────────┤
│  Hook / Port    │  扩展机制：所有扩展通过 hook 或接口注入    │
│  (插件层)       │  持久化、事件投影、客户端通知都是 hook      │
└─────────────────────────────────────────────────────┘
```

**关键洞察**：Loop 是一个**无状态、无生命周期、纯执行**的引擎。它不关心 session 是主 agent 还是子 agent，不关心谁在发消息，不关心事件发给谁。这让主/子 agent 完全一致。

### 2. Loop 的执行流程（极简）

```
Start → loadHistory → resumeFromHistory → runLoop

runLoop:
  1. Pull 1 message from Mailbox
  2. processMessage → runTurn
  3. runTurn → runStep 循环直到完成
  4. 如果 Mailbox 为空 → ErrMailboxEmpty → 退出
```

`runStep` 的 10 步流程（所有 hook 都标注）：
```
1. PreTurn hook
2. Append user message to ContextMgr (History)
3. runStep → Build History + Check & Compact → PreModel hook
4. Generate / Stream → PostModel hook
5. Append assistant message to ContextMgr
6. HasToolCalls? → executeTools (parallel, with Pre/Post Tool hooks)
7. Append tool results to ContextMgr
8. return false (continue) → runStep again
9. No tool calls → PostTurn hook → return true (done)
```

### 3. Hook 系统：所有扩展都是 hook

Loopy 的 hook 覆盖全部生命周期：

| Hook | 触发时机 | 用途 |
|------|---------|------|
| `LoopStart` | loop 启动 | 系统初始化 |
| `LoopEnd` | loop 退出 | 清理、资源释放 |
| `PreTurn` | 处理新消息前 | 消息拦截、前置处理 |
| `PostTurn` | 一轮完成 | 后置通知 |
| `PreModel` | 请求模型前 | 修改历史、注入 steer |
| `PostModel` | 模型响应后 | 记录响应、事件投影 |
| `PreTool` | 工具执行前 | 权限检查、工具拦截 |
| `PostTool` | 工具执行后 | 结果处理、事件投影 |
| `PreCompact` | 压缩前 | 阻止压缩、自定义策略 |
| `PostCompact` | 压缩后 | 记录压缩事件 |
| `SubagentEvent` | 子 agent 创建/推送 | 生命周期跟踪 |

**事件持久化也是一个 hook listener**：`NewEventPersistListener` 将 hook payload 转换为内部事件写入 Store。loop 完全不感知持久化。

**客户端事件投影也是 hook listener**：Hive 控制面通过 `NewClientEventListener` 将 hook payload 转换为 `hex.v1` 类型化事件。loopy 对 `hex.v1` 完全无感知。

### 4. 端口接口（Ports）：纯抽象，无具体实现

| 接口 | 职责 | 实现示例 |
|------|------|---------|
| `history.History` | 对话历史管理（Build/Append/Set） | `MemoryHistory`（内存）、持久化版（从 Store 加载） |
| `mailbox.Mailbox` | 消息队列（Push/Pull） | `ChannelMailbox`（内存）、`RedisMailbox` |
| `session.Store` | 事件日志（AppendEvents/LoadEvents/LoadHistory） | `JSONLStore`、Hive 的 Walkman 实现 |
| `model.ChatModel` | 模型调用（Generate/Stream/WithTools） | `OpenAIModel`、MockModel |
| `compactor.Compactor` | 上下文压缩（Compact） | `RecentNCompactor`、`SummarizeCompactor` |
| `heartbeat.Keeper` | 存活注册（Register/IsAlive/Cancel/Refresh） | `MemoryKeeper`、RedisKeeper |

**所有接口都是纯抽象，loop 只依赖接口，不依赖实现**。

### 5. 子 agent 与主 agent 完全一致

```go
// SubagentTool 不自己创建 Loop，它只向 Coordinator PushMessage
// Coordinator 收到 PushMessage 后，检查 IsAlive，如果未激活则 WakeSession
// WakeSession 构建 LoopConfig（和主 agent 完全一致）→ 启动 Loop
// 子 agent 的 Loop 和主 agent 的 Loop 是同一个 struct，同一个代码路径
```

**关键**：子 agent 不是 "简化版"，而是通过**不同的配置**（工具集、系统提示、是否启用子 agent）来区分能力。

### 6. 事件日志而非状态存储

Store 是**追加式事件流**，不是状态快照。恢复时：
- `LoadHistory` 从事件流中读取 `step` 消息，遇到 `step.compact` 停止（之前已被截断）
- `LoadEvents` 用于客户端翻页，从游标切片
- 系统提示由调用方自行构建，不存储

这保证了恢复的**幂等性**和**正确性**。

---

## Yomi 当前架构问题

### 1. Agent 状态机过重

```
Idle → Streaming → ExecutingTool → Streaming → ... → Idle → Closed
```

状态机带来：
- `handle_idle` / `handle_streaming` / `handle_execute_tool` 三个大方法
- `transition_after_streaming` 的复杂逻辑
- `cancel_token` 的跨状态传递
- 350 行以上的 `handle_streaming` 和 `handle_streaming_with_retry`

### 2. SimpleAgent 是独立实现

`SimpleAgent` 和 `Agent` 的核心差异：
- 工具串行 vs 并行（`execute_tools_parallel` 不可用）
- 无 hooks（Pre/Post ToolUse 技能级 hook 不生效）
- 无 compaction（`MessageBuffer` 不压缩）
- 无权限检查交互（`Checker` 简单版）
- 无 steer/消息拦截
- 无 checkpoint

### 3. 事件发送是硬编码的

```rust
// 在 handle_streaming 中硬编码事件发送
if let Err(e) = self.event_bus.try_send(Event::Model(ModelEvent::Chunk { ... })) {
    tracing::warn!("Failed to send chunk event: {}", e);
}
```

这导致：
- 事件发送逻辑散落在各处
- 无法灵活替换事件消费者（如改为写入日志而非发送到 TUI）
- 无法支持"同一个 hook 触发多个消费者"（如同时持久化 + 通知 TUI）

### 4. MessageBuffer 是具体结构

```rust
pub struct MessageBuffer { ... }
```

不是接口，无法替换为持久化版本或其他策略。

### 5. AgentShared 是超大 struct

包含 15+ 个字段：provider, model_config, task_store, todo_storage, compactor, session_store, message_store, usage_store, permission_state, skill_folders, file_state_store, checkpoint_store, data_dir, message_interceptor, hook_registry, channel_hub, goal_store, event_bus...

子 agent 和主 agent 需要不同的资源子集，但共用同一个 struct。

### 6. 没有 Mailbox 抽象

消息直接通过 `AgentHandle` 的 `input_tx` 发送，没有统一的消息队列接口。`AgentInput` 包含多种变体，处理逻辑在 `handle_idle` 中按 match 分发。

---

## 改进方案：Loopy-Inspired Yomi 架构

### 核心原则

1. **提取 `AgentLoop` 作为纯执行引擎**：无状态机、无生命周期、无事件发送
2. **主 agent 和子 agent 用同一个 `AgentLoop`**：通过配置区分能力
3. **Hook 系统替代硬编码事件发送**：所有生命周期事件走 hook
4. **端口接口化**：History、Mailbox、Store、Compactor 都定义为 trait
5. **Coordinator 负责生命周期管理**：唤醒、心跳、子 agent 调度

### 架构图

```
┌─────────────────────────────────────────────────────────────────┐
│  Coordinator  │  管理 Session 生命周期、心跳、子 agent 唤醒      │
│  (app/coordinator.rs)  │  持有 Store、Mailbox、Waker、HookRegistry    │
├─────────────────────────────────────────────────────────────────┤
│  AgentLoop    │  纯执行引擎：拉消息 → 执行 turn → 抽干即死       │
│  (agent/loop.rs)       │  无状态机、无事件发送、无持久化逻辑            │
├─────────────────────────────────────────────────────────────────┤
│  HookRegistry │  所有生命周期扩展：持久化、事件投影、客户端通知    │
│  (hooks/hook.rs)       │  可注册多个 listener（如 persist + tui）      │
├─────────────────────────────────────────────────────────────────┤
│  Ports (traits)                                               │
│  ├─ History (Build/Append/Set)  ← MessageBuffer 抽象化        │
│  ├─ Mailbox (Push/Pull)         ← AgentInput 抽象化           │
│  ├─ Store (Append/Load)         ← 事件日志存储                 │
│  ├─ Compactor (Compact)         ← 已有 Compactor 提取         │
│  ├─ ChatModel (Generate/Stream)   ← Provider 抽象化           │
│  └─ Heartbeat (Register/Alive)    ← 新增（可选）               │
└─────────────────────────────────────────────────────────────────┘
```

### 1. 提取 `AgentLoop`（核心引擎）

```rust
// agent/loop.rs
pub struct AgentLoop {
    config: LoopConfig,
    tools: HashMap<String, Arc<dyn Tool>>,
    history: Box<dyn History>,
    step_count: usize,
}

pub struct LoopConfig {
    pub system_prompt: String,
    pub model: Arc<dyn ChatModel>,
    pub tools: Vec<Arc<dyn Tool>>,
    pub compactor: Option<Box<dyn Compactor>>,
    pub max_steps: usize,
    pub max_context_tokens: usize,
    pub stream_mode: bool,
    pub tool_timeout: Duration,
    pub session_id: String,
    pub hooks: HookRegistry,
    pub mailbox: Box<dyn Mailbox>,
    pub store: Option<Box<dyn Store>>,
    pub on_done: Option<Box<dyn FnOnce()>>,
}

impl AgentLoop {
    /// 同步执行，外层需要 spawn
    pub fn run(&mut self, ctx: &mut LoopContext) {
        self.step_count = 0;
        let _ = self.run_inner(ctx);
    }

    fn run_inner(&mut self, ctx: &mut LoopContext) -> Result<(), AgentError> {
        // 1. Load history from store
        self.load_history(ctx)?;
        // 2. Resume from history (pending tool calls, etc.)
        self.resume_from_history(ctx)?;
        // 3. Loop start hook
        self.config.hooks.run_loop_start(ctx, LoopStartPayload { ... });
        // 4. Main loop: pull message from mailbox
        self.run_loop(ctx)
    }

    fn run_loop(&mut self, ctx: &mut LoopContext) -> Result<(), AgentError> {
        loop {
            if ctx.is_cancelled() { return Err(AgentError::Cancelled); }
            match self.config.mailbox.pull(&self.config.session_id, 1) {
                Ok(msgs) if !msgs.is_empty() => {
                    self.process_message(ctx, &msgs[0])?;
                }
                Ok(_) | Err(MailboxError::Empty) => {
                    // Mailbox empty → exit
                    return Ok(());
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    fn process_message(&mut self, ctx: &mut LoopContext, msg: &Mail) -> Result<(), AgentError> {
        // 1. PreTurn hook
        let pre_result = self.config.hooks.run_pre_turn(ctx, PreTurnPayload { ... });
        if pre_result.skip_processing { return Ok(()); }
        // 2. Append user message to history
        if let Some(chat_msg) = &pre_result.mail.chat_msg {
            self.history.append(ctx, chat_msg.clone())?;
        }
        // 3. Run turn
        self.run_turn(ctx)
    }

    fn run_turn(&mut self, ctx: &mut LoopContext) -> Result<(), AgentError> {
        loop {
            if self.step_count >= self.config.max_steps {
                return Err(AgentError::MaxStepsExceeded);
            }
            self.step_count += 1;
            let done = self.run_step(ctx)?;
            if done { return Ok(()); }
        }
    }

    fn run_step(&mut self, ctx: &mut LoopContext) -> Result<bool, AgentError> {
        // 3-4. Build history + check & compact
        let history = self.check_and_compact(ctx)?;
        // 5. PreModel hook
        let history = self.config.hooks.run_pre_model(ctx, PreModelPayload { history });
        // 6. Generate / Stream
        let (resp, usage) = self.generate(ctx, &history.messages)?;
        // 7. PostModel hook
        let resp = self.config.hooks.run_post_model(ctx, PostModelPayload { response: resp });
        // 8. Append assistant message
        self.history.append(ctx, resp.clone())?;
        // 9. Tool calls
        if has_tool_calls(&resp) {
            let tool_msgs = self.execute_tools(ctx, &resp.tool_calls)?;
            for m in &tool_msgs {
                self.history.append(ctx, m.clone())?;
            }
            return Ok(false); // continue
        }
        // 10. PostTurn hook
        self.config.hooks.run_post_turn(ctx, PostTurnPayload { message: resp });
        Ok(true) // done
    }
}
```

**关键变化**：
- `AgentLoop` 没有 `AgentState`，没有 `input_rx`，没有 `event_bus`
- 所有扩展都通过 `LoopConfig` 的端口（History、Mailbox、Hooks、Store）注入
- `AgentLoop` 不关心 session 是主 agent 还是子 agent

### 2. 主 Agent 包装 `AgentLoop`

```rust
// agent/agent.rs（简化后）
pub struct Agent {
    loop: Option<AgentLoop>,
    handle: AgentHandle,
    // 以下字段不再在 Agent 中，而是注入到 LoopConfig
    // - event_bus → 通过 HookRegistry listener
    // - message_buffer → 通过 History trait
    // - input_rx → 通过 Mailbox trait
    // - cancel_token → 通过 LoopContext
}

impl Agent {
    pub async fn spawn(...) -> AgentHandle {
        // 1. 构建 LoopConfig
        let config = LoopConfig {
            system_prompt: ...,
            model: ...,
            tools: ...,
            compactor: ...,
            max_steps: ...,
            hooks: build_hooks(&event_bus, &store), // 注册 persist listener + TUI listener
            mailbox: build_channel_mailbox(),
            store: Some(Box::new(sqlite_store)),
            on_done: Some(Box::new(|| { /* send shutdown event */ })),
            ...
        };
        // 2. 创建 AgentLoop
        let mut loop = AgentLoop::new(config);
        // 3. 在 tokio::spawn 中运行
        tokio::spawn(async move { loop.run(&mut LoopContext::new(cancel_token)); });
        // 4. 返回 AgentHandle（包装 mailbox Push 接口）
        AgentHandle::new(mailbox_tx)
    }
}
```

### 3. Hook 系统替代硬编码事件发送

```rust
// hooks/hook.rs（简化）
pub trait HookRegistry: Send + Sync {
    fn run_loop_start(&self, ctx: &LoopContext, payload: LoopStartPayload);
    fn run_loop_end(&self, ctx: &LoopContext, payload: LoopEndPayload);
    fn run_pre_turn(&self, ctx: &LoopContext, payload: PreTurnPayload) -> PreTurnResult;
    fn run_post_turn(&self, ctx: &LoopContext, payload: PostTurnPayload);
    fn run_pre_model(&self, ctx: &LoopContext, payload: PreModelPayload) -> PreModelResult;
    fn run_post_model(&self, ctx: &LoopContext, payload: PostModelPayload) -> PostModelResult;
    fn run_pre_tool(&self, ctx: &LoopContext, payload: PreToolPayload) -> PreToolResult;
    fn run_post_tool(&self, ctx: &LoopContext, payload: PostToolPayload) -> PostToolResult;
    fn run_pre_compact(&self, ctx: &LoopContext, payload: PreCompactPayload) -> PreCompactResult;
    fn run_post_compact(&self, ctx: &LoopContext, payload: PostCompactPayload);
    fn add_listener(&mut self, listener: Box<dyn HookListener>);
}

pub trait HookListener: Send + Sync {
    fn on_event(&self, ctx: &LoopContext, event: &dyn Any);
}
```

**事件持久化作为 listener**：

```rust
// 不再在 Agent 中硬编码 event_bus.send(...)
// 而是通过 HookRegistry 注册 listener

pub struct EventPersistListener {
    store: Arc<dyn Store>,
}

impl HookListener for EventPersistListener {
    fn on_event(&self, ctx: &LoopContext, event: &dyn Any) {
        if let Some(payload) = event.downcast_ref::<PostModelPayload>() {
            self.store.append_event(ctx.session_id, Event::Step(StepMessage(payload.response)));
        }
        if let Some(payload) = event.downcast_ref::<PostToolPayload>() {
            let content = tool_result_to_text(&payload.result);
            self.store.append_event(ctx.session_id, Event::Step(StepMessage(
                Message::tool(content, payload.tool_call.tool_call_id)
            )));
        }
        // ...
    }
}
```

**TUI 事件发送作为 listener**：

```rust
pub struct TuiEventListener {
    event_bus: EventBusHandle,
}

impl HookListener for TuiEventListener {
    fn on_event(&self, ctx: &LoopContext, event: &dyn Any) {
        if let Some(payload) = event.downcast_ref::<PostModelPayload>() {
            let _ = self.event_bus.try_send(Event::Model(ModelEvent::Chunk { ... }));
        }
        // ...
    }
}
```

### 4. History Trait 替代 MessageBuffer

```rust
// history.rs
pub trait History: Send + Sync {
    fn build(&self) -> Result<Vec<Arc<Message>>, AgentError>;
    fn append(&mut self, msg: Arc<Message>) -> Result<(), AgentError>;
    fn set(&mut self, msgs: Vec<Arc<Message>>) -> Result<(), AgentError>;
}

// 内存版（默认）
pub struct MemoryHistory {
    system_prompt: String,
    steps: Vec<Arc<Message>>,
}

// 持久化版（从 Store 加载）
pub struct PersistentHistory {
    store: Arc<dyn Store>,
    session_id: String,
    cache: Vec<Arc<Message>>,
}
```

### 5. Mailbox Trait 替代 AgentInput

```rust
// mailbox.rs
pub struct Mail {
    pub chat_msg: Option<Arc<Message>>,
    // 可扩展：permission response, steer, cancel, etc.
}

pub trait Mailbox: Send + Sync {
    fn push(&self, session_id: &str, mails: Vec<Mail>) -> Result<(), MailboxError>;
    fn pull(&self, session_id: &str, count: usize) -> Result<Vec<Mail>, MailboxError>;
}

pub enum MailboxError {
    Empty,
    Closed,
}

// 内存版（默认）
pub struct ChannelMailbox {
    channels: DashMap<String, mpsc::Sender<Mail>>,
}
```

### 6. 子 agent 与主 agent 完全一致

```rust
// SubagentTool 不再创建 SimpleAgent，而是向 Coordinator PushMessage

impl Tool for SubagentTool {
    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let sub_id = self.create_or_reuse_subsession(ctx.session_id).await?;
        // 向子 session 的 Mailbox Push 消息
        self.coordinator.push_message(&sub_id, Mail::user(task)).await?;
        // 等待子 agent 完成（通过 Heartbeat 或 OnDone hook）
        let result = self.wait_for_completion(&sub_id).await?;
        Ok(ToolOutput::text(result))
    }
}

// Coordinator 的 WakeSession 为主/子 agent 构建完全相同的 LoopConfig
// 区别仅在：
// - 子 agent 的 tools 可能不同（无 subagent 工具）
// - 子 agent 的 system_prompt 不同
// - 子 agent 的 max_steps 可能不同
// 但 Loop 的代码路径完全相同
```

### 7. 删除 SimpleAgent

`SimpleAgent` 被 `AgentLoop` 完全替代。不再需要：
- `simple.rs`
- `AgentShared`（部分功能移到 LoopConfig 或 Coordinator）
- `AgentState` 状态机（Loop 无状态机）
- `AgentInput` 变体（改为 Mailbox::Mail）

---

## 迁移路径

### Phase 1：提取 Loop 引擎（最小化改动）

1. 创建 `agent/loop.rs`，将 `Agent::handle_streaming` + `handle_execute_tool` 的核心逻辑提取为 `AgentLoop::run_step`
2. `Agent` 持有 `AgentLoop`，原来的 `start_loop` 改为调用 `AgentLoop::run`
3. 保持 `AgentInput` 和 `event_bus` 不变，通过适配层连接
4. 验证：所有现有测试通过

### Phase 2：引入 Hook 系统

1. 将现有的 `event_bus.try_send(...)` 替换为 `hooks.run_post_model(...)` 等
2. 将 `event_bus` 的 consumer 注册为 hook listener
3. 将持久化逻辑（`persist_message`）注册为 hook listener
4. 验证：事件流和持久化行为不变

### Phase 3：引入 History / Mailbox 接口

1. 定义 `History` trait，将 `MessageBuffer` 实现为 `MemoryHistory`
2. 定义 `Mailbox` trait，将 `AgentInput` 适配为 `Mail`
3. `AgentLoop` 使用 `History` + `Mailbox` 接口
4. 验证：所有测试通过

### Phase 4：子 agent 统一

1. 删除 `SimpleAgent`
2. `SubagentTool` 改为向 Coordinator 的 Mailbox Push 消息
3. 子 agent 的 LoopConfig 和主 agent 共用同一套构建逻辑
4. 验证：子 agent 并行工具、hooks、compaction 全部生效

### Phase 5：清理和优化

1. 删除 `AgentState` 状态机（如果不需要）
2. 简化 `AgentHandle`（只保留 Mailbox Push 接口）
3. 删除 `AgentShared` 中不再需要的字段
4. 可选：引入 `Heartbeat` 接口用于崩溃恢复

---

## 预期收益

| 指标 | 当前 | 改进后 |
|------|------|--------|
| 主/子 agent 代码重复 | `Agent` 1500行 + `SimpleAgent` 450行 | 统一 `AgentLoop` ~800行 |
| 新增功能修改点 | 2处（Agent + SimpleAgent） | 1处（AgentLoop） |
| 事件发送耦合 | 硬编码在 15+ 处 | 通过 hook listener 解耦 |
| 测试复杂度 | 需分别测试 Agent 和 SimpleAgent | 只需测试 AgentLoop |
| 恢复逻辑 | 在 Agent 中硬编码 | 通过 Store + History 接口自动恢复 |
| 子 agent 能力 | 串行工具、无 hooks | 并行工具、完整 hooks、compaction |

---

## 风险与缓解

| 风险 | 缓解 |
|------|------|
| 重构范围大，可能引入 bug | 分 5 个 phase，每 phase 都有完整测试验证 |
| 现有 TUI 事件流依赖 | Phase 2 用 hook listener 适配，保持现有 event bus 不变 |
| 持久化逻辑变化 | Phase 3 用 hook listener 做事件持久化，等价于现有逻辑 |
| 性能回归 | 引入 Mailbox channel 可能增加一层转发，但无额外序列化开销 |
| 状态机取消后，如何支持 "Compaction" 状态 | Loop 中 compaction 是同步阻塞操作（在 PreModel 前完成），无需独立状态 |
