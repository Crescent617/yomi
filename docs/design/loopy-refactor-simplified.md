# Yomi Agent 重构 — 简化版执行计划

## 核心原则

1. **提取 `AgentLoop`**：纯执行引擎，无状态机、无事件发送硬编码
2. **主/子 agent 完全一致**：`Agent` 和 `SubagentTool` 都用同一个 `AgentLoop`
3. **Hook 系统替代硬编码事件**：所有事件通过 Hook + Listener 转发
4. **Rust 习惯**：不用显式 close，Drop 即关闭；trait 只抽象**必须**替换的边界

---

## 一、端口 Trait（只保留 5 个）

### 1. `History` — 对话历史

```rust
pub trait History: Send + Sync {
    fn build(&self) -> Vec<Arc<Message>>;
    fn append(&mut self, msg: Arc<Message>);
    fn replace(&mut self, msgs: Vec<Arc<Message>>);
    fn messages(&self) -> &[Arc<Message>];
    fn len(&self) -> usize;
    fn clear(&mut self); // 保留 system prompt
    fn sanitize(&mut self);
    fn truncate_at(&mut self, msg_id: &MessageId) -> bool;
}
```

**实现**：`MemoryHistory`（包装 `Vec<Arc<Message>>` + `Mutex`）

### 2. `Mailbox` — 消息输入

```rust
pub struct Mail {
    pub content: Vec<ContentBlock>,
    pub generation: u64, // for cancel fencing
}

pub trait Mailbox: Send + Sync {
    /// 阻塞拉取（None = channel 已关闭）
    async fn pull(&mut self) -> Option<Mail>;
    /// 非阻塞尝试拉取 steer
    fn try_pull_steer(&mut self) -> Option<Vec<ContentBlock>>;
}
```

**实现**：
- `ChannelMailbox` — 包装 `mpsc::Receiver<AgentInput>` + `mpsc::Receiver<Vec<ContentBlock>>`
- `DirectMailbox` — `VecDeque<Mail>`，用于 `SubagentTool` 直接注入

**不需要 `close`**：Rust 的 `mpsc::Receiver` drop 即关闭，`DirectMailbox` 同理。

### 3. `HookRegistry` — 生命周期扩展（扩展现有）

```rust
pub trait HookRegistry: Send + Sync {
    // Mutating hooks
    async fn pre_turn(&self, ctx: &LoopContext, msg: &mut Mail) -> PreTurnResult;
    async fn pre_model(&self, ctx: &LoopContext, msgs: &mut Vec<Arc<Message>>);
    async fn pre_tool(&self, ctx: &LoopContext, calls: &mut Vec<ToolCall>) -> PreToolResult;
    async fn pre_compact(&self, ctx: &LoopContext, msgs: &mut Vec<Arc<Message>>) -> bool;
    async fn pre_stop(&self, ctx: &LoopContext) -> PreStopResult;

    // Observation hooks (listeners)
    async fn post_turn(&self, ctx: &LoopContext, msg: &Arc<Message>);
    async fn post_model(&self, ctx: &LoopContext, msg: &Arc<Message>, usage: Option<&TokenUsage>);
    async fn post_tool(&self, ctx: &LoopContext, results: &[ToolExecutionResult]);
    async fn post_compact(&self, ctx: &LoopContext, old: usize, new: usize);
    async fn on_error(&self, ctx: &LoopContext, phase: ErrorPhase, err: &str, recoverable: bool);
    async fn loop_start(&self, ctx: &LoopContext);
    async fn loop_end(&self, ctx: &LoopContext, reason: LoopEndReason);
}
```

**实现**：`DefaultHookRegistry` — 维护 `Vec<Arc<dyn HookHandler>>` + `Vec<Arc<dyn HookListener>>`

### 4. `TurnTracker` — 回合跟踪

```rust
pub trait TurnTracker: Send + Sync {
    fn start(&mut self, user_msg_id: &MessageId, summary: &str) -> Option<Arc<Turn>>;
    fn current(&self) -> Option<Arc<Turn>>;
    async fn complete(&mut self);
    async fn cancel(&mut self);
}
```

**实现**：`CheckpointTurnTracker` / `NoOpTurnTracker`

### 5. `EventStore` — 消息持久化（扩展 MessageStore）

```rust
#[async_trait]
pub trait EventStore: Send + Sync {
    async fn append(&self, session_id: &str, msg: &Message);
    async fn replace(&self, session_id: &str, msgs: &[Message]);
    async fn load(&self, session_id: &str) -> Result<Vec<Message>>;
}
```

**实现**：`JsonlEventStore`（包装 `JsonlMessageStore`） / `NoOpEventStore`

### 不抽象的（直接用具体类型）

| 功能 | 理由 | 类型 |
|------|------|------|
| 模型调用 | Provider trait 已存在，AgentLoop 直接 `Arc<dyn Provider>` | `Provider` |
| 工具注册表 | 已有 `ToolRegistry` struct，不需要 trait | `ToolRegistry` |
| 权限检查 | 可选功能，直接 `Option<Checker>` | `Checker` |
| Compactor | 已有 trait，AgentLoop 直接 `Option<Arc<dyn Compactor>>` | `Compactor` |
| Usage 记录 | 可选，直接 `Option<Arc<dyn UsageStore>>` | `UsageStore` |

---

## 二、AgentLoop 引擎

```rust
pub struct AgentLoop {
    config: LoopConfig,
}

pub struct LoopConfig {
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub system_prompt: String,
    pub provider: Arc<dyn Provider>,
    pub model_config: Arc<ModelConfig>,
    pub tools: ToolRegistry,
    pub compactor: Option<Arc<dyn Compactor>>,
    pub max_steps: usize,
    pub history: Box<dyn History>,
    pub hooks: Arc<dyn HookRegistry>,
    pub turn_tracker: Box<dyn TurnTracker>,
    pub event_store: Option<Arc<dyn EventStore>>,
    pub usage_store: Option<Arc<dyn UsageStore>>,
    pub data_dir: PathBuf,
    pub working_dir: PathBuf,
    pub skills: Vec<Arc<Skill>>,
    pub max_tool_output_length: usize,
    pub checker: Option<Arc<Checker>>,
}

pub struct LoopContext {
    pub cancel_token: CancelToken,
    pub session_id: SessionId,
    pub agent_id: AgentId,
}

pub enum TurnResult {
    Done { finish_reason: Option<FinishReason> },
    Stopped,        // hook 请求停止
    MaxSteps,       // 达到最大步数
}

impl AgentLoop {
    /// 执行一个完整的 turn（streaming + tool + continue 循环）
    pub async fn run_turn(&mut self, ctx: &LoopContext, mail: Mail) -> Result<TurnResult, LoopError> {
        // 1. PreTurn hook
        let mut mail = mail;
        let pre = self.config.hooks.pre_turn(ctx, &mut mail).await;
        if pre.skip { return Ok(TurnResult::Done { finish_reason: None }); }
        let msg = Arc::new(Message::with_blocks(Role::User, mail.content));

        // 2. Append & persist
        self.config.history.append(msg.clone());
        self.persist(&msg).await;

        // 3. Start turn tracking
        let summary = extract_summary(&msg.content);
        let turn = self.config.turn_tracker.start(&msg.id, &summary);

        // 4. Run step loop
        let mut step = 0;
        loop {
            if ctx.cancel_token.is_cancelled() { return Err(LoopError::Cancelled); }
            if step >= self.config.max_steps { return Ok(TurnResult::MaxSteps); }
            step += 1;

            // 4a. Compact
            self.check_compact(ctx).await?;

            // 4b. Build history + PreModel
            let mut msgs = self.config.history.build();
            msgs = resolve_assets(&msgs, &self.config.data_dir).await;
            self.config.hooks.pre_model(ctx, &mut msgs).await;

            // 4c. Stream model
            let tools = self.config.tools.definitions();
            let assistant_id = MessageId::new();
            let result = self.stream(ctx, &msgs, &tools, assistant_id.clone()).await?;

            // 4d. PostModel hook
            let assistant_msg = Arc::new(result.message);
            self.config.hooks.post_model(ctx, &assistant_msg, result.token_usage.as_ref()).await;
            self.config.history.append(assistant_msg.clone());
            self.persist(&assistant_msg).await;

            // 4e. Record usage
            if let Some(u) = &result.token_usage {
                self.record_usage(ctx, u).await;
            }

            // 4f. Tool calls
            if let Some(calls) = &assistant_msg.tool_calls {
                let continue_session = self.execute_tools(ctx, calls, &assistant_msg.id, turn.clone()).await?;
                if !continue_session {
                    self.config.turn_tracker.complete().await.ok();
                    return Ok(TurnResult::Stopped);
                }
                continue; // 继续下一轮 streaming
            }

            // 4g. Handle finish_reason
            match result.finish_reason {
                None | Some(FinishReason::MaxTokens) => {
                    let cont = Arc::new(Message::user("continue"));
                    self.config.history.append(cont.clone());
                    self.persist(&cont).await;
                    continue;
                }
                _ => {
                    let pre_stop = self.config.hooks.pre_stop(ctx).await;
                    if pre_stop.continue_session {
                        if let Some(blocks) = pre_stop.steer_blocks {
                            let steer = Arc::new(Message::with_blocks(Role::User, blocks));
                            self.config.history.append(steer.clone());
                            self.persist(&steer).await;
                        }
                        continue;
                    }
                    self.config.turn_tracker.complete().await.ok();
                    return Ok(TurnResult::Done { finish_reason: result.finish_reason });
                }
            }
        }
    }

    async fn stream(&mut self, ctx: &LoopContext, msgs: &[Arc<Message>], tools: &[Arc<ToolDefinition>], msg_id: MessageId)
        -> Result<StreamResult, LoopError> { ... }

    async fn execute_tools(&mut self, ctx: &LoopContext, calls: &[ToolCall], assistant_id: &MessageId, turn: Option<Arc<Turn>>)
        -> Result<bool, LoopError> { ... }

    async fn check_compact(&mut self, ctx: &LoopContext) -> Result<(), LoopError> { ... }

    async fn persist(&self, msg: &Message) {
        if let Some(store) = &self.config.event_store {
            store.append(&self.config.session_id.0, msg).await.ok();
        }
    }

    async fn record_usage(&self, ctx: &LoopContext, usage: &TokenUsage) { ... }
}
```

---

## 三、Agent 包装层（简化状态机）

```rust
pub struct Agent {
    loop_engine: AgentLoop,
    context: AgentExecutionContext, // 只保留 Idle / Streaming / Closed
    input_rx: mpsc::Receiver<AgentInput>,
    steer_rx: mpsc::Receiver<Vec<ContentBlock>>,
    cancel_token: CancelToken,
    input_stale_since: Arc<AtomicU64>,
}

impl Agent {
    pub async fn spawn(id: AgentId, shared: &Arc<AgentShared>, args: AgentSpawnArgs) -> AgentHandle {
        // 1. 构建 LoopConfig
        let loop_config = build_loop_config(id.clone(), shared, &args).await;
        let loop_engine = AgentLoop::new(loop_config);

        let (input_tx, input_rx) = mpsc::channel(20);
        let (steer_tx, steer_rx) = mpsc::channel(20);
        let (context, state_rx) = AgentExecutionContext::new(AgentState::Idle);
        let cancel_token = args.cancel_token.clone().unwrap_or_default();
        let input_stale_since = Arc::new(AtomicU64::new(0));

        let agent = Self { loop_engine, context, input_rx, steer_rx, cancel_token: cancel_token.clone(), input_stale_since: input_stale_since.clone() };

        // 2. 构建 HookRegistry + 注册 listeners
        let mut hooks = build_hooks(&shared, &args).await;
        hooks.add_listener(Arc::new(EventPersistListener::new(shared.message_store.clone())));
        hooks.add_listener(Arc::new(TuiEventListener::new(shared.event_bus.clone(), id.clone(), session_id.clone())));
        hooks.add_listener(Arc::new(UsageRecordListener::new(shared.usage_store.clone())));
        // ... 注册到 loop_engine

        // 3. Spawn
        let session_id = SessionId(args.session_id.clone());
        tokio::spawn(async move { agent.start_loop().await; });

        AgentHandle::new(id, input_tx, state_rx, cancel_token, input_stale_since, steer_tx)
    }

    async fn start_loop(mut self) {
        loop {
            let state = self.context.current_state();
            if state == AgentState::Closed { break; }

            match state {
                AgentState::Idle => {
                    self.context.reset_iteration();
                    tokio::select! {
                        biased;
                        Some(input) = self.input_rx.recv() => {
                            match self.handle_input(input).await {
                                Ok(Some(mail)) => {
                                    self.context.transition_to(AgentState::Streaming);
                                    let mut ctx = LoopContext { cancel_token: self.cancel_token.clone(), session_id: self.loop_engine.config.session_id.clone(), agent_id: self.loop_engine.config.agent_id.clone() };
                                    match self.loop_engine.run_turn(&mut ctx, mail).await {
                                        Ok(_) => self.context.transition_to(AgentState::Idle),
                                        Err(LoopError::Cancelled) => { /* handle cancel */ self.context.transition_to(AgentState::Idle); }
                                        Err(e) => { /* emit error via hook */ self.context.transition_to(AgentState::Idle); }
                                    }
                                }
                                Ok(None) => {} // continue waiting
                                Err(_) => { self.context.transition_to(AgentState::Closed); break; }
                            }
                        }
                        Some(steer) = self.steer_rx.recv() => {
                            // steer 直接当作 user message 处理，走同样的 run_turn
                            let mail = Mail { content: steer, generation: self.input_stale_since.load(Ordering::Relaxed) };
                            self.context.transition_to(AgentState::Streaming);
                            let mut ctx = LoopContext { ... };
                            let _ = self.loop_engine.run_turn(&mut ctx, mail).await;
                            self.context.transition_to(AgentState::Idle);
                        }
                        else => { self.context.transition_to(AgentState::Closed); break; }
                    }
                }
                AgentState::Streaming => {
                    // 不应该出现，run_turn 是同步阻塞的，完成后会回到 Idle
                    tracing::warn!("Unexpected Streaming state");
                    self.context.transition_to(AgentState::Idle);
                }
                AgentState::Closed => break,
            }
        }
    }

    async fn handle_input(&self, input: AgentInput) -> Result<Option<Mail>, ()> {
        match input {
            AgentInput::User { content, generation } => {
                let current = self.input_stale_since.load(Ordering::Relaxed);
                if generation < current {
                    tracing::info!("discarding stale input");
                    return Ok(None);
                }
                Ok(Some(Mail { content, generation }))
            }
            AgentInput::Shutdown => {
                self.loop_engine.config.turn_tracker.cancel().await.ok();
                Err(()) // signal close
            }
            AgentInput::Compact => {
                // 直接调用 loop_engine 的 compaction，不走状态机
                let mut ctx = LoopContext { ... };
                let _ = self.loop_engine.check_compact(&mut ctx).await;
                Ok(None)
            }
            AgentInput::Rewind { message_id, target, result_tx } => {
                // 直接操作 history
                self.loop_engine.config.history.truncate_at(&message_id);
                // ... rewind checkpoint ...
                // persist ...
                let _ = result_tx.send(Ok(()));
                Ok(None)
            }
            AgentInput::Clear => {
                self.loop_engine.config.history.clear();
                // clear file_state, todo, persist ...
                Ok(None)
            }
            AgentInput::Continue => {
                Ok(Some(Mail { content: vec![ContentBlock::Text { text: "continue".to_string() }], generation: 0 }))
            }
            AgentInput::TaskResult { content, .. } => {
                Ok(Some(Mail { content, generation: 0 }))
            }
            _ => Ok(None),
        }
    }
}
```

**状态机只保留 3 个状态**：`Idle` / `Streaming` / `Closed`。`ExecutingTool` 和 `Compacting` 在 `AgentLoop::run_turn` 内部处理，外部不可见。

---

## 四、SubagentTool 改用 AgentLoop

```rust
impl Tool for SubagentTool {
    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let task = parse_task(&args)?;
        let preset = parse_preset(&args)?;

        // 1. 构建子 agent 的 LoopConfig（和主 agent 完全一致的路径）
        let mut config = LoopConfig {
            session_id: SessionId(self.subsession_id.clone()),
            agent_id: AgentId::new(),
            system_prompt: build_subagent_prompt(&preset, &ctx),
            provider: self.provider.clone(),
            model_config: self.model_config.clone(),
            tools: build_subagent_tools(&preset, &ctx),
            compactor: self.compactor.clone(),
            max_steps: self.max_iterations,
            history: Box::new(MemoryHistory::from_messages(self.inherit_history(&ctx))),
            hooks: Arc::new(DefaultHookRegistry::new()), // 无 hooks，或复用 parent 的 skill hooks
            turn_tracker: Box::new(NoOpTurnTracker),    // 子 agent 不需要 checkpoint
            event_store: None,                          // 子 agent 默认不持久化
            usage_store: None,
            data_dir: self.data_dir.clone(),
            working_dir: ctx.working_dir.clone(),
            skills: self.skills.clone(),
            max_tool_output_length: self.max_tool_output_length,
            checker: None, // 子 agent 默认不检查权限
        };

        // 2. 可选：注册一个 listener 收集结果
        let collector = Arc::new(ResultCollector::new());
        config.hooks.add_listener(collector.clone());

        // 3. 创建 AgentLoop
        let mut loop_engine = AgentLoop::new(config);

        // 4. 直接执行 turn
        let mail = Mail { content: vec![ContentBlock::Text { text: task }], generation: 0 };
        let mut loop_ctx = LoopContext {
            cancel_token: ctx.cancel_token.clone().unwrap_or_default(),
            session_id: SessionId(self.subsession_id.clone()),
            agent_id: loop_engine.config.agent_id.clone(),
        };

        match loop_engine.run_turn(&mut loop_ctx, mail).await {
            Ok(TurnResult::Done { .. } | TurnResult::Stopped | TurnResult::MaxSteps) => {
                // 5. 从 history 收集结果
                let result = collector.format_result(&loop_engine.config.history);
                Ok(ToolOutput::text(result))
            }
            Ok(_) => Ok(ToolOutput::error("Subagent turn was skipped")),
            Err(e) => Ok(ToolOutput::error(format!("Subagent failed: {}", e))),
        }
    }
}
```

**关键变化**：
- `SubagentTool` 不再创建 `SimpleAgent`，而是直接 `AgentLoop::new` + `run_turn`
- 自动获得并行工具执行、hooks、compaction
- 通过 `NoOpTurnTracker` 和 `None` store 关闭不需要的功能

---

## 五、Hook Listener（3 个核心）

### 1. `EventPersistListener` — 消息持久化

```rust
struct EventPersistListener { store: Option<Arc<dyn EventStore>> }

impl HookListener for EventPersistListener {
    async fn post_turn(&self, ctx: &LoopContext, msg: &Arc<Message>) {
        if let Some(store) = &self.store {
            store.append(&ctx.session_id.0, msg).await.ok();
        }
    }
    async fn post_model(&self, ctx: &LoopContext, msg: &Arc<Message>, _: Option<&TokenUsage>) {
        if let Some(store) = &self.store {
            store.append(&ctx.session_id.0, msg).await.ok();
        }
    }
    async fn post_tool(&self, ctx: &LoopContext, results: &[ToolExecutionResult]) {
        if let Some(store) = &self.store {
            for r in results {
                store.append(&ctx.session_id.0, &r.message).await.ok();
            }
        }
    }
    async fn post_compact(&self, ctx: &LoopContext, _: usize, _: usize) {
        // compaction 后的 replace 由 AgentLoop 直接调用 EventStore::replace
        // 这里不需要额外操作
    }
}
```

### 2. `TuiEventListener` — TUI 事件转发

```rust
struct TuiEventListener { bus: EventBusHandle, agent_id: AgentId }

impl HookListener for TuiEventListener {
    async fn loop_start(&self, ctx: &LoopContext) {
        self.bus.try_send(Event::Agent(AgentEvent::Lifecycle { agent_id: self.agent_id.clone(), state: AgentStatus::Running })).ok();
    }
    async fn post_model(&self, ctx: &LoopContext, msg: &Arc<Message>, usage: Option<&TokenUsage>) {
        // 发送 Chunk / TokenUsage / Completed / End 事件
        // 需要在 StreamCollector 中保留 chunk 数据，或重新设计事件模型
        // 简化：AgentLoop 的 stream 方法直接收集所有 chunk，post_model 时一次性发送 Completed + End
    }
    async fn post_tool(&self, ctx: &LoopContext, results: &[ToolExecutionResult]) {
        for r in results {
            self.bus.try_send(Event::Tool(r.event.clone())).ok();
        }
    }
    async fn on_error(&self, ctx: &LoopContext, phase: ErrorPhase, err: &str, recoverable: bool) {
        self.bus.try_send(Event::Agent(AgentEvent::Error { agent_id: self.agent_id.clone(), phase, error: err.to_string(), is_recoverable: recoverable })).ok();
    }
    async fn loop_end(&self, ctx: &LoopContext, reason: LoopEndReason) {
        let state = match reason { ... };
        self.bus.try_send(Event::Agent(AgentEvent::Lifecycle { agent_id: self.agent_id.clone(), state })).ok();
    }
}
```

**注意**：`post_model` 需要 chunk 数据。方案：
- 方案 A：`StreamCollector` 保留 chunk 列表，`post_model` 时遍历发送
- 方案 B：`AgentLoop::stream` 中直接发送 chunk 事件（但这就又硬编码了）
- **推荐方案**：`AgentLoop::stream` 接受一个 `FnMut(ModelStreamItem)` 回调，由 `HookRegistry` 提供。但这又复杂了。
- **最简方案**：保留 `StreamCollector` 的 `event_tx` 概念，但 `event_tx` 不是 `EventBusHandle`，而是一个 `Fn` trait 对象，由 `TuiEventListener` 提供。`post_model` 只发送 `Completed` + `End`，chunk 在 stream 过程中通过回调发送。

实际上更简单的做法是：**stream 过程中的 chunk 不通过 hook 发送**。`AgentLoop` 的 `stream` 方法接受一个 `&mut dyn FnMut(ModelStreamItem)` 回调，由 `Agent` 包装层注入。`Agent` 包装层注入 `TuiEventListener` 的闭包。`post_model` hook 只发送 `Completed`/`End`/`TokenUsage` 事件。

但这样 `AgentLoop` 又耦合了 TUI 事件...

**最终决定**：保持 `AgentLoop` 内部完全无事件发送。stream 过程中的 `Chunk` 事件由 `AgentLoop` 的 `stream` 方法返回一个 `Vec<String>`（所有 chunk），然后 `Agent` 包装层或 `SubagentTool` 自行处理。`TuiEventListener` 的 `post_model` 发送 `Completed` + `End` + `TokenUsage`。

### 3. `UsageRecordListener` — Token 使用记录

```rust
struct UsageRecordListener { store: Option<Arc<dyn UsageStore>>, model_id: String, provider: String }

impl HookListener for UsageRecordListener {
    async fn post_model(&self, ctx: &LoopContext, _: &Arc<Message>, usage: Option<&TokenUsage>) {
        if let (Some(store), Some(u)) = (&self.store, usage) {
            let record = UsageRecord::new(ctx.session_id.clone(), ctx.agent_id.clone(), u.clone(), &self.model_id, &self.provider, UsageType::Normal);
            store.record(&record).await.ok();
        }
    }
}
```

---

## 六、删除的代码

| 文件/方法 | 替代 | 说明 |
|-----------|------|------|
| `agent/simple.rs` | `AgentLoop::run_turn` | 子 agent 直接走 AgentLoop |
| `Agent::handle_streaming` | `AgentLoop::stream` | 提取到引擎 |
| `Agent::handle_streaming_with_retry` | `AgentLoop::stream` | 重试逻辑内聚 |
| `Agent::handle_execute_tool` | `AgentLoop::execute_tools` | 提取到引擎 |
| `Agent::transition_after_streaming` | `AgentLoop::run_turn` 尾部 | 内聚到引擎 |
| `Agent::collect_stream_output` | `AgentLoop::stream` | 内聚到引擎 |
| `Agent::maybe_compact_messages` | `AgentLoop::check_compact` | 提取到引擎 |
| `Agent::force_compact` | `AgentLoop::check_compact` | 提取到引擎 |
| `Agent::handle_compaction_result` | `AgentLoop::check_compact` | 提取到引擎 |
| `Agent::emit_user_message_event` | `TuiEventListener::post_turn` | 改为 listener |
| `Agent::emit_error` | `TuiEventListener::on_error` | 改为 listener |
| `Agent::emit_retrying` | `TuiEventListener::on_error` | 改为 listener |
| `Agent::emit_operation_cancelled` | `TuiEventListener::on_error` | 改为 listener |
| `Agent::emit_stopped_completed` | `TuiEventListener::loop_end` | 改为 listener |
| `Agent::emit_compaction_event` | `TuiEventListener::post_compact` | 改为 listener |
| `Agent::fail_agent` | `TuiEventListener::on_error` + `loop_end` | 改为 listener |
| `Agent::handle_clear` | `Agent::handle_input` | 内联到 input 处理 |
| `Agent::process_rewind` | `Agent::handle_input` | 内联到 input 处理 |
| `Agent::inject_user_message` | `AgentLoop::run_turn` 入口 | 内聚到引擎 |
| `Agent::truncate_at` | `MemoryHistory::truncate_at` | 迁移到 History |
| `Agent::start_turn_if_needed` | `TurnTracker::start` | 迁移到 TurnTracker |
| `Agent::complete_turn_if_needed` | `AgentLoop::run_turn` | 内聚到引擎 |
| `Agent::persist_message` | `AgentLoop::persist` | 迁移到引擎 |
| `Agent::record_compactor_token_usage` | `UsageRecordListener` | 改为 listener |
| `Agent::apply_compacted_messages` | `AgentLoop::check_compact` | 内聚到引擎 |
| `Agent::extract_summary` | 公共函数 | 提取为独立函数 |
| `MessageBuffer` | `MemoryHistory` | 重命名/迁移 |
| `StreamingHandler` | `AgentLoop::stream` + `StreamCollector` | 合并到引擎 |
| `AgentState::ExecutingTool` | 删除 | 内部状态，外部不可见 |
| `AgentState::Compacting` | 删除 | 内部状态，外部不可见 |

---

## 七、执行任务表（简化版，36 个任务）

### Phase 1: Trait 定义（5 个任务）

| 任务 | 文件 | 内容 |
|------|------|------|
| 1.1 | `agent/loop/ports.rs` | 定义 `History`, `Mailbox`, `Mail`, `TurnTracker`, `EventStore` trait |
| 1.2 | `agent/loop/hook_payloads.rs` | 定义 `PreTurnResult`, `PreToolResult`, `PreStopResult`, `LoopEndReason`, `LoopContext`, `LoopError`, `TurnResult` |
| 1.3 | `hooks/mod.rs` | 扩展 `HookRegistry` trait：添加 `pre_turn`, `post_turn`, `pre_model`, `post_model`, `pre_tool`, `post_tool`, `pre_compact`, `post_compact`, `pre_stop`, `on_error`, `loop_start`, `loop_end` |
| 1.4 | `hooks/listener.rs` | 定义 `HookListener` trait（对应 HookRegistry 的 observation 方法） |
| 1.5 | `agent/loop/mod.rs` | 模块组织，导出 `AgentLoop`, `LoopConfig`, `LoopContext`, 所有端口 trait |

### Phase 2: 适配器实现（8 个任务）

| 任务 | 文件 | 内容 |
|------|------|------|
| 2.1 | `agent/loop/history.rs` | `MemoryHistory`：从 `MessageBuffer` 迁移，实现 `History` trait |
| 2.2 | `agent/loop/mailbox.rs` | `ChannelMailbox`（包装 `mpsc::Receiver`）和 `DirectMailbox`（`VecDeque`），实现 `Mailbox` trait |
| 2.3 | `storage/event_store.rs` | `JsonlEventStore`：包装 `JsonlMessageStore`，实现 `EventStore` trait；`NoOpEventStore` |
| 2.4 | `agent/loop/turn_tracker.rs` | `CheckpointTurnTracker`（包装 `Turn` + `CheckpointStore`）和 `NoOpTurnTracker` |
| 2.5 | `hooks/listeners/event_persist.rs` | `EventPersistListener`：监听 `post_turn`, `post_model`, `post_tool`，调用 `EventStore::append` |
| 2.6 | `hooks/listeners/tui_event.rs` | `TuiEventListener`：监听 `loop_start`, `post_model`, `post_tool`, `on_error`, `loop_end`，转发到 `EventBus` |
| 2.7 | `hooks/listeners/usage_record.rs` | `UsageRecordListener`：监听 `post_model`，记录 `TokenUsage` 到 `UsageStore` |
| 2.8 | `hooks/registry.rs` | `DefaultHookRegistry`：维护 `Vec<Arc<dyn HookHandler>>` + `Vec<Arc<dyn HookListener>>`，实现所有生命周期方法 |

### Phase 3: AgentLoop 引擎（8 个任务）

| 任务 | 文件 | 内容 |
|------|------|------|
| 3.1 | `agent/loop/agent_loop.rs` | `AgentLoop` struct + `LoopConfig` struct + `AgentLoop::new` |
| 3.2 | `agent/loop/stream.rs` | `AgentLoop::stream`：从 `Agent::handle_streaming` + `StreamingHandler` 提取，返回 `StreamResult`（含 `Message` + `TokenUsage` + `Vec<String>` chunks） |
| 3.3 | `agent/loop/stream_collector.rs` | 迁移现有的 `StreamCollectorState`，收集 `ModelStreamItem`，返回 `StreamResult` |
| 3.4 | `agent/loop/execute_tools.rs` | `AgentLoop::execute_tools`：从 `Agent::handle_execute_tool` 提取，权限检查 + PreTool hook + `execute_tools_parallel` + PostTool hook + 追加结果到 history |
| 3.5 | `agent/loop/compact.rs` | `AgentLoop::check_compact`：从 `Agent::maybe_compact_messages` + `force_compact` 提取，PreCompact hook + `Compactor::auto_compact` + 更新 history + PostCompact hook |
| 3.6 | `agent/loop/run_turn.rs` | `AgentLoop::run_turn`：编排整个 turn（PreTurn → append → start turn → step loop → compact → stream → PostModel → append → tool/continue/stop → complete turn） |
| 3.7 | `agent/loop/helpers.rs` | `extract_summary` 公共函数，`persist` 辅助方法，`record_usage` 辅助方法 |
| 3.8 | — | 编译验证 `AgentLoop` 模块（不依赖 `Agent`） |

### Phase 4: Agent 包装层重构（6 个任务）

| 任务 | 文件 | 内容 |
|------|------|------|
| 4.1 | `agent/agent.rs` | 重构 `Agent` struct：删除 `message_buffer`, `tool_registry`, `hook_registry`, `current_turn`, `max_tool_output_length`, `skills`，替换为 `loop_engine: AgentLoop` |
| 4.2 | `agent/agent.rs` | 重构 `Agent::spawn`：构建 `LoopConfig`（包含所有端口），创建 `AgentLoop`，注册 3 个 listener，创建 `AgentHandle`，spawn 任务 |
| 4.3 | `agent/agent.rs` | 重构 `Agent::start_loop`：简化状态机（Idle → 处理 input → Streaming → run_turn → Idle / Closed） |
| 4.4 | `agent/agent.rs` | 重构 `Agent::handle_input`：内联 `handle_clear`, `process_rewind`, `inject_user_message` 的逻辑，处理所有 `AgentInput` variant |
| 4.5 | `agent/agent.rs` | 删除所有已迁移的方法：`handle_streaming`, `handle_execute_tool`, `transition_after_streaming`, `collect_stream_output`, `maybe_compact`, `force_compact`, `emit_*`, `fail_agent`, `persist_message`, `record_compactor_usage`, `apply_compacted_messages`, `truncate_at`, `start_turn_if_needed`, `complete_turn_if_needed` |
| 4.6 | `agent/types.rs` | 简化 `AgentState`：删除 `ExecutingTool` 和 `Compacting`，只保留 `Idle`, `Streaming`, `Closed` |

### Phase 5: SubagentTool 重构（3 个任务）

| 任务 | 文件 | 内容 |
|------|------|------|
| 5.1 | `tools/subagent.rs` | 重构 `SubagentTool::exec`：删除 `SimpleAgent` 创建，改为 `AgentLoop::new` + `run_turn` + 从 history 收集结果 |
| 5.2 | `tools/subagent.rs` | 实现 `ResultCollector`（`HookListener`）：收集 `post_model` 和 `post_tool` 的输出，格式化 `ToolOutput` |
| 5.3 | — | 删除 `agent/simple.rs`，从 `agent/mod.rs` 移除导出 |

### Phase 6: 清理与验证（6 个任务）

| 任务 | 内容 |
|------|------|
| 6.1 | 删除 `agent/message_buffer.rs`（或保留为 `MemoryHistory` 的内部实现） |
| 6.2 | 删除 `agent/streaming.rs`（`StreamingHandler` 合并到 `AgentLoop::stream`） |
| 6.3 | `cargo build -p kernel` 编译通过 |
| 6.4 | `cargo clippy -p kernel -p cli -p tui` 无警告 |
| 6.5 | `cargo test -p kernel --lib` 全部通过（380+ 测试） |
| 6.6 | 手动测试：主 agent 对话、子 agent 调用、compaction、cancel、rewind |

---

**总计：36 个任务，分 6 个 Phase。**
