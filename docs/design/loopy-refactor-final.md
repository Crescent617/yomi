# Yomi Agent 重构 — 最终设计

## 核心改进

1. **History**：`sanitize` 是内部行为，不暴露到 trait；`build()` 内部自动调用
2. **去掉 Mailbox trait**：`AgentLoop::run_turn` 直接接收 `Mail`，`Agent` 包装层负责从 `input_rx` / `steer_rx` 读取分发
3. **Hook 改为 `&mut payload` 链式传递**：像 Loopy 一样，多个 handler 按顺序修改 payload；兼容现有 `HookHandler`（通过 `DefaultHookRegistry` 内部适配）
4. **HookListener 独立**：只通知，只读 payload，不修改

---

## 一、端口 Trait（3 个）

### 1. `History` — 对话历史

```rust
pub trait History: Send + Sync {
    /// 构建完整消息列表（system prompt + 所有步骤）。内部自动调用 sanitize。
    fn build(&mut self) -> Vec<Arc<Message>>;
    
    /// 追加单条消息
    fn append(&mut self, msg: Arc<Message>);
    
    /// 原子替换所有非 system 消息
    fn replace(&mut self, msgs: Vec<Arc<Message>>);
    
    /// 获取消息引用（只读，不 sanitize）
    fn messages(&self) -> &[Arc<Message>];
    
    /// 获取消息数量
    fn len(&self) -> usize;
    
    /// 清空（保留 system prompt）
    fn clear(&mut self);
    
    /// 截断到指定 message_id 之前，返回是否成功
    fn truncate_at(&mut self, msg_id: &MessageId) -> bool;
}
```

**实现**：`MemoryHistory` — 内部 `Vec<Arc<Message>>` + `Mutex`，`build()` 前自动 `sanitize()`。

### 2. `TurnTracker` — 回合跟踪

```rust
pub trait TurnTracker: Send + Sync {
    /// 启动新回合，返回当前 Turn（用于传递给工具做 checkpoint）
    fn start(&mut self, user_msg_id: &MessageId, summary: &str) -> Option<Arc<Turn>>;
    
    /// 获取当前回合
    fn current(&self) -> Option<Arc<Turn>>;
    
    /// 完成当前回合（创建 checkpoint）
    async fn complete(&mut self);
    
    /// 取消当前回合（清理 checkpoint）
    async fn cancel(&mut self);
    
    /// 重置
    fn reset(&mut self);
}
```

**实现**：`CheckpointTurnTracker` / `NoOpTurnTracker`

### 3. `EventStore` — 消息持久化

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

| 功能 | 类型 |
|------|------|
| 模型调用 | `Arc<dyn Provider>` |
| 工具注册表 | `ToolRegistry` |
| 权限检查 | `Option<Arc<Checker>>` |
| Compactor | `Option<Arc<dyn Compactor>>` |
| Usage 记录 | `Option<Arc<dyn UsageStore>>` |

---

## 二、Hook 新设计：Payload 链式传递

### 核心原则

- 每个生命周期一个**具体 struct**（`PreTurnPayload`, `PreToolPayload` 等）
- `DefaultHookRegistry` 的 `run_*` 方法签名：`&mut payload`
- 多个 `HookHandler` 按顺序执行，每个 handler 可以修改 payload（通过 `HookResult` 间接修改，适配后写回 payload）
- 所有 `HookHandler` 执行完后，遍历 `HookListener`（只读 `&payload`）发送事件
- 现有 `HookHandler` 实现**无需改动**，兼容层在 `DefaultHookRegistry` 内部完成

### Payload 定义

```rust
pub struct PreTurnPayload {
    pub mail: Mail,
    pub skip: bool,  // 设为 true 则跳过此 turn
}

pub struct PostTurnPayload {
    pub message: Arc<Message>,
}

pub struct PreModelPayload {
    pub messages: Vec<Arc<Message>>,
    // PreModel hook 可以修改 messages（如注入 steer）
}

pub struct PostModelPayload {
    pub message: Arc<Message>,
    pub token_usage: Option<TokenUsage>,
    pub finish_reason: Option<FinishReason>,
    /// 所有 chunk 文本（用于 listener 发送 TUI 事件）
    pub chunks: Vec<String>,
}

pub struct PreToolPayload {
    pub calls: Vec<ToolCall>,
    /// 已批准的 calls（handler 可修改：移入 blocked 或修改 args）
    pub approved: Vec<ToolCall>,
    /// 被拒绝的 calls（tool_call_id, reason）
    pub blocked: Vec<(String, String)>,
    /// 额外注入的 context 消息
    pub contexts: Vec<String>,
}

pub struct PostToolPayload {
    pub results: Vec<ToolExecutionResult>,
    pub continue_session: bool,
    pub contexts: Vec<String>,
}

pub struct PreCompactPayload {
    pub messages: Vec<Arc<Message>>,
    pub allow: bool,  // 设为 false 阻止压缩
}

pub struct PostCompactPayload {
    pub old_count: usize,
    pub new_count: usize,
    pub result: Option<CompactionResult>,
}

pub struct PreStopPayload {
    pub continue_session: bool,
    pub steer_blocks: Vec<ContentBlock>,
}

pub struct OnErrorPayload {
    pub phase: ErrorPhase,
    pub error: String,
    pub is_recoverable: bool,
}

pub struct LoopEndPayload {
    pub reason: LoopEndReason,
}

pub enum LoopEndReason {
    Completed(Option<FinishReason>),
    Cancelled,
    Failed(String),
    MaxSteps,
}
```

### HookRegistry 接口

```rust
pub trait HookRegistry: Send + Sync {
    // === Mutating hooks（按顺序修改 payload） ===
    async fn pre_turn(&self, ctx: &LoopContext, payload: &mut PreTurnPayload);
    async fn post_turn(&self, ctx: &LoopContext, payload: &PostTurnPayload);
    async fn pre_model(&self, ctx: &LoopContext, payload: &mut PreModelPayload);
    async fn post_model(&self, ctx: &LoopContext, payload: &PostModelPayload);
    async fn pre_tool(&self, ctx: &LoopContext, payload: &mut PreToolPayload);
    async fn post_tool(&self, ctx: &LoopContext, payload: &mut PostToolPayload);
    async fn pre_compact(&self, ctx: &LoopContext, payload: &mut PreCompactPayload);
    async fn post_compact(&self, ctx: &LoopContext, payload: &PostCompactPayload);
    async fn pre_stop(&self, ctx: &LoopContext, payload: &mut PreStopPayload);
    async fn on_error(&self, ctx: &LoopContext, payload: &mut OnErrorPayload);
    
    // === Lifecycle hooks（无 payload 或只读） ===
    async fn loop_start(&self, ctx: &LoopContext);
    async fn loop_end(&self, ctx: &LoopContext, payload: &mut LoopEndPayload);
    
    // === Listener 注册（只读通知） ===
    fn add_listener(&mut self, listener: Arc<dyn HookListener>);
}
```

### HookListener 接口（只读通知）

```rust
pub trait HookListener: Send + Sync {
    async fn pre_turn(&self, ctx: &LoopContext, payload: &PreTurnPayload);
    async fn post_turn(&self, ctx: &LoopContext, payload: &PostTurnPayload);
    async fn pre_model(&self, ctx: &LoopContext, payload: &PreModelPayload);
    async fn post_model(&self, ctx: &LoopContext, payload: &PostModelPayload);
    async fn pre_tool(&self, ctx: &LoopContext, payload: &PreToolPayload);
    async fn post_tool(&self, ctx: &LoopContext, payload: &PostToolPayload);
    async fn pre_compact(&self, ctx: &LoopContext, payload: &PreCompactPayload);
    async fn post_compact(&self, ctx: &LoopContext, payload: &PostCompactPayload);
    async fn pre_stop(&self, ctx: &LoopContext, payload: &PreStopPayload);
    async fn on_error(&self, ctx: &LoopContext, payload: &OnErrorPayload);
    async fn loop_start(&self, ctx: &LoopContext);
    async fn loop_end(&self, ctx: &LoopContext, payload: &LoopEndPayload);
}
```

### DefaultHookRegistry 实现：兼容现有 HookHandler

```rust
pub struct DefaultHookRegistry {
    /// 现有 handler（mutating，通过 HookResult 间接修改）
    handlers: HashMap<HookEvent, Vec<Arc<dyn HookHandler>>>,
    /// 新 listener（只读通知）
    listeners: Vec<Arc<dyn HookListener>>,
}

impl HookRegistry for DefaultHookRegistry {
    async fn pre_tool(&self, ctx: &LoopContext, payload: &mut PreToolPayload) {
        // 1. 遍历现有 HookHandler，调用 run(ctx) 获取 HookResult，写回 payload
        for handler in self.handlers.get(&HookEvent::PreToolUse).unwrap_or_default() {
            let hook_ctx = build_hook_context(ctx, payload); // 从 payload 构建 HookContext
            match handler.run(&hook_ctx).await {
                Ok(HookResult::PreTool(decision)) => {
                    // 将 decision 应用回 payload
                    if matches!(decision.action, PreToolAction::Block) {
                        // 从 approved 移到 blocked
                        // 注意：这里需要 tool_call_id 匹配
                    }
                    if let Some(input) = decision.updated_input {
                        // 更新对应 tool_call 的 arguments
                    }
                    if let Some(ctx) = decision.context {
                        payload.contexts.push(ctx);
                    }
                }
                // ...
            }
        }
        
        // 2. 所有 handler 执行完后，通知 listeners（只读）
        for listener in &self.listeners {
            listener.pre_tool(ctx, payload).await;
        }
    }
    
    // ... 其他方法同理
}
```

**关键**：现有 `HookHandler`（`CommandHookHandler`, `SkillHookHandler`, `GoalPreStopHandler`）完全不需要改。`DefaultHookRegistry` 的兼容层负责把 `HookResult` 写回 payload。

### 3 个核心 Listener 实现

**EventPersistListener**：监听 `post_turn`, `post_model`, `post_tool`，调用 `EventStore::append`

**TuiEventListener**：监听 `loop_start`, `post_model`, `post_tool`, `on_error`, `loop_end`，转发到 `EventBus`

**UsageRecordListener**：监听 `post_model` 的 `token_usage`，调用 `UsageStore::record`

---

## 三、AgentLoop 引擎（无 Mailbox）

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
    Stopped,
    MaxSteps,
}

impl AgentLoop {
    pub fn new(config: LoopConfig) -> Self { ... }
    
    /// 执行一个完整 turn（streaming + tool + continue 循环）
    /// 调用方负责提供 Mail，AgentLoop 不感知消息来源
    pub async fn run_turn(&mut self, ctx: &LoopContext, mail: Mail) -> Result<TurnResult, LoopError> {
        // 1. PreTurn
        let mut payload = PreTurnPayload { mail, skip: false };
        self.config.hooks.pre_turn(ctx, &mut payload).await;
        if payload.skip { return Ok(TurnResult::Done { finish_reason: None }); }
        let msg = Arc::new(Message::with_blocks(Role::User, payload.mail.content));
        
        // 2. Append & persist
        self.config.history.append(msg.clone());
        self.persist(ctx, &msg).await;
        
        // 3. Start turn
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
            let mut pre_model = PreModelPayload { messages: msgs };
            self.config.hooks.pre_model(ctx, &mut pre_model).await;
            
            // 4c. Stream model
            let tools = self.config.tools.definitions();
            let assistant_id = MessageId::new();
            let mut post_model = self.stream(ctx, &pre_model.messages, &tools, assistant_id).await?;
            
            // 4d. PostModel hook
            self.config.hooks.post_model(ctx, &mut post_model).await;
            
            // 4e. Record usage
            if let Some(u) = &post_model.token_usage {
                self.record_usage(ctx, u).await;
            }
            
            // 4f. Append assistant message
            self.config.history.append(post_model.message.clone());
            self.persist(ctx, &post_model.message).await;
            
            // 4g. Tool calls
            if let Some(calls) = &post_model.message.tool_calls {
                let mut pre_tool = PreToolPayload {
                    calls: calls.clone(),
                    approved: calls.clone(),
                    blocked: vec![],
                    contexts: vec![],
                };
                self.config.hooks.pre_tool(ctx, &mut pre_tool).await;
                
                let continue_session = self.execute_tools(ctx, &pre_tool, &assistant_id, turn.clone()).await?;
                
                if !continue_session {
                    self.config.turn_tracker.complete().await.ok();
                    return Ok(TurnResult::Stopped);
                }
                continue;
            }
            
            // 4h. Handle finish_reason
            match post_model.finish_reason {
                None | Some(FinishReason::MaxTokens) => {
                    let cont = Arc::new(Message::user("continue"));
                    self.config.history.append(cont.clone());
                    self.persist(ctx, &cont).await;
                    continue;
                }
                _ => {
                    let mut pre_stop = PreStopPayload { continue_session: false, steer_blocks: vec![] };
                    self.config.hooks.pre_stop(ctx, &mut pre_stop).await;
                    if pre_stop.continue_session {
                        if !pre_stop.steer_blocks.is_empty() {
                            let steer = Arc::new(Message::with_blocks(Role::User, pre_stop.steer_blocks));
                            self.config.history.append(steer.clone());
                            self.persist(ctx, &steer).await;
                        }
                        continue;
                    }
                    self.config.turn_tracker.complete().await.ok();
                    return Ok(TurnResult::Done { finish_reason: post_model.finish_reason });
                }
            }
        }
    }
    
    async fn stream(&mut self, ctx: &LoopContext, msgs: &[Arc<Message>], tools: &[Arc<ToolDefinition>], msg_id: MessageId)
        -> Result<PostModelPayload, LoopError> { ... }
    
    async fn execute_tools(&mut self, ctx: &LoopContext, pre_tool: &PreToolPayload, assistant_id: &MessageId, turn: Option<Arc<Turn>>)
        -> Result<bool, LoopError> { ... }
    
    async fn check_compact(&mut self, ctx: &LoopContext) -> Result<(), LoopError> { ... }
    
    async fn persist(&self, ctx: &LoopContext, msg: &Message) {
        if let Some(store) = &self.config.event_store {
            store.append(&ctx.session_id.0, msg).await.ok();
        }
    }
    
    async fn record_usage(&self, ctx: &LoopContext, usage: &TokenUsage) {
        if let Some(store) = &self.config.usage_store {
            let record = UsageRecord::new(
                ctx.session_id.clone(), ctx.agent_id.clone(), usage.clone(),
                &self.config.model_config.model_id, &self.config.model_config.provider.to_string(),
                UsageType::Normal
            );
            store.record(&record).await.ok();
        }
    }
}
```

---

## 四、Agent 包装层（状态机 + 消息分发）

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
        let mut history = Box::new(MemoryHistory::from_messages(&messages));
        let mut hooks = Arc::new(DefaultHookRegistry::new());
        
        // 注册现有 HookHandler
        if let Some(base) = shared.hook_registry.as_ref() {
            hooks.merge(base); // 把现有 handler 复制到 DefaultHookRegistry
        }
        for skill in &args.skills {
            // ... 加载 skill hooks
        }
        
        // 注册 3 个 listener
        hooks.add_listener(Arc::new(EventPersistListener::new(shared.message_store.clone())));
        hooks.add_listener(Arc::new(TuiEventListener::new(shared.event_bus.clone(), id.clone())));
        hooks.add_listener(Arc::new(UsageRecordListener::new(shared.usage_store.clone())));
        
        let loop_config = LoopConfig { ... };
        let loop_engine = AgentLoop::new(loop_config);
        
        // 2. 创建 Agent
        let (input_tx, input_rx) = mpsc::channel(20);
        let (steer_tx, steer_rx) = mpsc::channel(20);
        let (context, state_rx) = AgentExecutionContext::new(AgentState::Idle);
        let cancel_token = args.cancel_token.clone().unwrap_or_default();
        let input_stale_since = Arc::new(AtomicU64::new(0));
        
        let agent = Self { loop_engine, context, input_rx, steer_rx, cancel_token: cancel_token.clone(), input_stale_since: input_stale_since.clone() };
        
        tokio::spawn(async move { agent.start_loop().await; });
        
        AgentHandle::new(id, input_tx, state_rx, cancel_token, input_stale_since, steer_tx)
    }
    
    async fn start_loop(mut self) {
        let mut ctx = LoopContext {
            cancel_token: self.cancel_token.clone(),
            session_id: self.loop_engine.config.session_id.clone(),
            agent_id: self.loop_engine.config.agent_id.clone(),
        };
        
        self.loop_engine.hooks.loop_start(&ctx).await;
        
        loop {
            let state = self.context.current_state();
            if state == AgentState::Closed { break; }
            
            match state {
                AgentState::Idle => {
                    self.context.reset_iteration();
                    
                    tokio::select! {
                        biased;
                        Some(input) = self.input_rx.recv() => {
                            match self.handle_input(&ctx, input).await {
                                Ok(Some(mail)) => {
                                    self.context.transition_to(AgentState::Streaming);
                                    match self.loop_engine.run_turn(&ctx, mail).await {
                                        Ok(_) => self.context.transition_to(AgentState::Idle),
                                        Err(LoopError::Cancelled) => {
                                            self.loop_engine.hooks.on_error(&ctx, &mut OnErrorPayload {
                                                phase: ErrorPhase::Streaming, error: "cancelled".to_string(), is_recoverable: true
                                            }).await;
                                            self.context.transition_to(AgentState::Idle);
                                        }
                                        Err(e) => {
                                            self.loop_engine.hooks.on_error(&ctx, &mut OnErrorPayload {
                                                phase: ErrorPhase::Streaming, error: e.to_string(), is_recoverable: false
                                            }).await;
                                            self.context.transition_to(AgentState::Idle);
                                        }
                                    }
                                }
                                Ok(None) => {}
                                Err(()) => { self.context.transition_to(AgentState::Closed); break; }
                            }
                        }
                        Some(steer) = self.steer_rx.recv() => {
                            let mail = Mail { content: steer, generation: 0 };
                            self.context.transition_to(AgentState::Streaming);
                            let _ = self.loop_engine.run_turn(&ctx, mail).await;
                            self.context.transition_to(AgentState::Idle);
                        }
                        else => { self.context.transition_to(AgentState::Closed); break; }
                    }
                }
                AgentState::Streaming => {
                    // run_turn 是同步阻塞的，这里不应该出现
                    tracing::warn!("Unexpected Streaming state");
                    self.context.transition_to(AgentState::Idle);
                }
                AgentState::Closed => break,
            }
        }
        
        let mut end_payload = LoopEndPayload { reason: LoopEndReason::Completed(None) };
        self.loop_engine.hooks.loop_end(&ctx, &mut end_payload).await;
    }
    
    async fn handle_input(&mut self, ctx: &LoopContext, input: AgentInput) -> Result<Option<Mail>, ()> {
        match input {
            AgentInput::User { content, generation } => {
                let current = self.input_stale_since.load(Ordering::Relaxed);
                if generation < current {
                    tracing::info!("discarding stale input");
                    return Ok(None);
                }
                Ok(Some(Mail { content, generation }))
            }
            AgentInput::Continue => {
                Ok(Some(Mail { content: vec![ContentBlock::Text { text: "continue".to_string() }], generation: 0 }))
            }
            AgentInput::TaskResult { content, .. } => {
                Ok(Some(Mail { content, generation: 0 }))
            }
            AgentInput::Shutdown => {
                self.loop_engine.config.turn_tracker.cancel().await.ok();
                Err(()) // signal close
            }
            AgentInput::Compact => {
                let mut compact_ctx = ctx.clone();
                let _ = self.loop_engine.check_compact(&compact_ctx).await;
                Ok(None)
            }
            AgentInput::Rewind { message_id, target, result_tx } => {
                self.loop_engine.config.turn_tracker.cancel().await.ok();
                let truncated = self.loop_engine.config.history.truncate_at(&message_id);
                // ... checkpoint rewind ...
                // ... persist ...
                let _ = result_tx.send(Ok(()));
                Ok(None)
            }
            AgentInput::Clear => {
                self.loop_engine.config.history.clear();
                // clear file_state, todo, persist ...
                Ok(None)
            }
            _ => Ok(None),
        }
    }
}
```

**状态机只保留 3 个**：`Idle` / `Streaming` / `Closed`。`ExecutingTool` 和 `Compacting` 在 `AgentLoop::run_turn` 内部处理。

---

## 五、SubagentTool 改用 AgentLoop

```rust
impl Tool for SubagentTool {
    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let task = parse_task(&args)?;
        let preset = parse_preset(&args)?;
        
        // 1. 构建子 agent 的 LoopConfig（和主 agent 完全一致）
        let mut history = Box::new(MemoryHistory::from_messages(
            if self.inherit_context { parent_history(&ctx) } else { vec![] }
        ));
        
        let mut hooks = Arc::new(DefaultHookRegistry::new());
        // 可选：注册 skill hooks（复用 parent 的 skills）
        
        let config = LoopConfig {
            session_id: SessionId(self.subsession_id.clone()),
            agent_id: AgentId::new(),
            system_prompt: build_subagent_prompt(&preset, &ctx),
            provider: self.provider.clone(),
            model_config: self.model_config.clone(),
            tools: build_subagent_tools(&preset, &ctx),
            compactor: self.compactor.clone(),
            max_steps: self.max_iterations,
            history,
            hooks,
            turn_tracker: Box::new(NoOpTurnTracker),
            event_store: None,
            usage_store: None,
            data_dir: self.data_dir.clone(),
            working_dir: ctx.working_dir.clone(),
            skills: self.skills.clone(),
            max_tool_output_length: self.max_tool_output_length,
            checker: None,
        };
        
        let mut loop_engine = AgentLoop::new(config);
        
        // 2. 直接执行 turn
        let mail = Mail { content: vec![ContentBlock::Text { text: task }], generation: 0 };
        let loop_ctx = LoopContext {
            cancel_token: ctx.cancel_token.clone().unwrap_or_default(),
            session_id: SessionId(self.subsession_id.clone()),
            agent_id: loop_engine.config.agent_id.clone(),
        };
        
        loop_engine.hooks.loop_start(&loop_ctx).await;
        
        match loop_engine.run_turn(&loop_ctx, mail).await {
            Ok(TurnResult::Done { .. } | TurnResult::Stopped | TurnResult::MaxSteps) => {
                // 3. 从 history 收集结果
                let msgs = loop_engine.config.history.messages();
                let result = format_subagent_result(msgs);
                
                let mut end_payload = LoopEndPayload { reason: LoopEndReason::Completed(None) };
                loop_engine.hooks.loop_end(&loop_ctx, &mut end_payload).await;
                
                Ok(ToolOutput::text(result))
            }
            Ok(_) => Ok(ToolOutput::error("Subagent turn was skipped")),
            Err(e) => Ok(ToolOutput::error(format!("Subagent failed: {}", e))),
        }
    }
}
```

**关键**：子 agent 直接复用 `AgentLoop`，自动获得并行工具、hooks、compaction。`NoOpTurnTracker` + `None` store 关闭不需要的功能。

---

## 六、删除的代码

| 文件/方法 | 替代 | 说明 |
|-----------|------|------|
| `agent/simple.rs` | `AgentLoop::run_turn` | 删除整个文件 |
| `Agent::handle_streaming` | `AgentLoop::stream` | 提取到引擎 |
| `Agent::handle_streaming_with_retry` | `AgentLoop::stream` | 重试内聚 |
| `Agent::handle_execute_tool` | `AgentLoop::execute_tools` | 提取到引擎 |
| `Agent::transition_after_streaming` | `AgentLoop::run_turn` 尾部 | 内聚到引擎 |
| `Agent::collect_stream_output` | `AgentLoop::stream` + `StreamCollector` | 内聚到引擎 |
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

## 七、执行任务表（30 个任务，6 个 Phase）

### Phase 1: Trait 定义（4 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 1.1 | `agent/loop/ports.rs` | `History` trait（5 个方法，无 `sanitize`）、`TurnTracker` trait、`EventStore` trait |
| 1.2 | `agent/loop/hook_payloads.rs` | 11 个 payload struct + `LoopEndReason` enum + `LoopContext` + `LoopError` + `TurnResult` |
| 1.3 | `hooks/mod.rs` | 新 `HookRegistry` trait（`pre_turn`/`post_turn`/`pre_model`/`post_model`/`pre_tool`/`post_tool`/`pre_compact`/`post_compact`/`pre_stop`/`on_error`/`loop_start`/`loop_end` + `add_listener`） + 新 `HookListener` trait（12 个方法） |
| 1.4 | `agent/loop/mod.rs` | 模块组织，导出 `AgentLoop`, `LoopConfig`, `LoopContext`, 所有端口 trait |

### Phase 2: 适配器实现（7 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 2.1 | `agent/loop/history.rs` | `MemoryHistory`：从 `MessageBuffer` 迁移，实现 `History` trait，`build()` 内部自动 `sanitize()` |
| 2.2 | `storage/event_store.rs` | `JsonlEventStore`：包装 `JsonlMessageStore`；`NoOpEventStore` |
| 2.3 | `agent/loop/turn_tracker.rs` | `CheckpointTurnTracker`（包装 `Turn` + `CheckpointStore`）；`NoOpTurnTracker` |
| 2.4 | `hooks/listeners/event_persist.rs` | `EventPersistListener`：监听 `post_turn`/`post_model`/`post_tool`，调用 `EventStore::append` |
| 2.5 | `hooks/listeners/tui_event.rs` | `TuiEventListener`：监听 `loop_start`/`post_model`/`post_tool`/`on_error`/`loop_end`，转发到 `EventBus` |
| 2.6 | `hooks/listeners/usage_record.rs` | `UsageRecordListener`：监听 `post_model`，记录 `TokenUsage` 到 `UsageStore` |
| 2.7 | `hooks/registry.rs` | `DefaultHookRegistry`：维护 `HashMap<HookEvent, Vec<Arc<dyn HookHandler>>>` + `Vec<Arc<dyn HookListener>>`，实现 `HookRegistry` trait（`run_pre_tool` 等方法的 `HookResult` → payload 兼容层） |

### Phase 3: AgentLoop 引擎（6 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 3.1 | `agent/loop/agent_loop.rs` | `AgentLoop` struct + `LoopConfig` struct + `AgentLoop::new` |
| 3.2 | `agent/loop/stream.rs` | `AgentLoop::stream`：从 `Agent::handle_streaming` + `StreamingHandler` 提取，返回 `PostModelPayload`（含 `Message` + `TokenUsage` + `chunks`） |
| 3.3 | `agent/loop/stream_collector.rs` | 迁移 `StreamCollectorState`，收集 `ModelStreamItem`，返回 `PostModelPayload` |
| 3.4 | `agent/loop/execute_tools.rs` | `AgentLoop::execute_tools`：权限检查 + `PreToolPayload` → PreTool hook + `execute_tools_parallel` + `PostToolPayload` → PostTool hook + 追加结果到 history |
| 3.5 | `agent/loop/compact.rs` | `AgentLoop::check_compact`：`PreCompactPayload` → `Compactor::auto_compact` + `PostCompactPayload` + 更新 history + 持久化 |
| 3.6 | `agent/loop/run_turn.rs` | `AgentLoop::run_turn`：完整编排（PreTurn → append → start turn → step loop → compact → stream → PostModel → append → tool/continue/stop → complete turn） |

### Phase 4: Agent 包装层重构（5 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 4.1 | `agent/agent.rs` | 重构 `Agent` struct：删除 `message_buffer`, `tool_registry`, `hook_registry`, `current_turn`, `max_tool_output_length`, `skills`，替换为 `loop_engine: AgentLoop` |
| 4.2 | `agent/agent.rs` | 重构 `Agent::spawn`：构建 `LoopConfig`（含 `MemoryHistory`, `DefaultHookRegistry` + 3 个 listener, `CheckpointTurnTracker`），创建 `AgentLoop` |
| 4.3 | `agent/agent.rs` | 重构 `Agent::start_loop`：简化状态机（Idle → 处理 input → Streaming → `run_turn` → Idle / Closed），`input_rx`/`steer_rx` select 读取，`handle_input` 分发 |
| 4.4 | `agent/agent.rs` | 重构 `Agent::handle_input`：处理所有 `AgentInput` variant，转换为 `Mail` 或控制命令（`Compact`/`Rewind`/`Clear`/`Shutdown`） |
| 4.5 | `agent/agent.rs` | 删除所有已迁移方法（`handle_streaming`, `handle_execute_tool`, `transition_after_streaming`, `collect_stream_output`, `maybe_compact`, `force_compact`, `emit_*`, `fail_agent`, `persist_message`, `record_compactor_usage`, `apply_compacted_messages`, `truncate_at`, `start_turn_if_needed`, `complete_turn_if_needed`） |

### Phase 5: SubagentTool 重构（3 个任务）

| # | 文件 | 内容 |
|---|------|------|
| 5.1 | `tools/subagent.rs` | 重构 `SubagentTool::exec`：删除 `SimpleAgent`，改为 `AgentLoop::new` + `run_turn` + 从 `history.messages()` 收集结果 |
| 5.2 | `tools/subagent.rs` | 实现 `format_subagent_result`：从 `History::messages()` 提取 assistant 回复和工具输出，格式化 `ToolOutput`（复用 `SimpleAgent::build_result` 逻辑） |
| 5.3 | `agent/mod.rs` | 删除 `simple` 模块导出，删除 `simple.rs` 文件 |

### Phase 6: 清理与验证（5 个任务）

| # | 内容 |
|---|------|
| 6.1 | 删除 `agent/message_buffer.rs`（或保留为 `MemoryHistory` 的内部实现） |
| 6.2 | 删除 `agent/streaming.rs`（`StreamingHandler` 合并到 `AgentLoop::stream`） |
| 6.3 | `cargo build -p kernel` 编译通过 |
| 6.4 | `cargo clippy -p kernel -p cli -p tui` 无警告 |
| 6.5 | `cargo test -p kernel --lib` 全部通过（380+ 测试） |

---

## 八、Hook 兼容层说明

现有 `HookHandler` 完全不修改：

```rust
// 现有 handler 不需要改
#[async_trait]
impl HookHandler for CommandHookHandler {
    fn name(&self) -> &str { ... }
    fn events(&self) -> &[HookEvent] { ... }
    async fn run(&self, ctx: &HookContext) -> Result<HookResult> { ... }
}
```

`DefaultHookRegistry::pre_tool` 内部兼容：

```rust
async fn pre_tool(&self, ctx: &LoopContext, payload: &mut PreToolPayload) {
    for handler in self.handlers.get(&HookEvent::PreToolUse).unwrap_or_default() {
        // 从 payload 构建 HookContext（session_id, tool_name, tool_input 等）
        let hook_ctx = HookContext::pre_tool(
            &ctx.session_id.0,
            payload.calls[0].name.as_str(), // 取第一个 call（现有 handler 只处理单个）
            payload.calls[0].id.as_str(),
            &self.working_dir, // 需要保存到 registry
            payload.calls[0].arguments.clone(),
        );
        
        match handler.run(&hook_ctx).await {
            Ok(HookResult::PreTool(decision)) => {
                // 将 decision 应用回 payload
                if matches!(decision.action, PreToolAction::Block) {
                    // 把第一个 call 从 approved 移到 blocked
                    if let Some(call) = payload.approved.get(0) {
                        payload.blocked.push((call.id.clone(), decision.reason.unwrap_or_default()));
                        payload.approved.remove(0);
                    }
                }
                if let Some(input) = decision.updated_input {
                    if let Some(call) = payload.approved.get_mut(0) {
                        call.arguments = input;
                    }
                }
                if let Some(ctx) = decision.context {
                    payload.contexts.push(ctx);
                }
            }
            // ...
        }
    }
    
    // 通知 listeners
    for listener in &self.listeners {
        listener.pre_tool(ctx, payload).await;
    }
}
```

**注意**：现有 handler 的 `HookContext` 只处理单个 tool call，而 `AgentLoop` 的 `PreToolPayload` 包含多个 calls。兼容层处理方式是：每个 handler 处理 payload 中的第一个 call（或所有 calls），然后应用结果。

这只是一个过渡方案。后续可以逐步将现有 `HookHandler` 迁移到新的 `ModernHookHandler` trait（直接 `&mut payload`），但最终 `DefaultHookRegistry` 内部仍然可以把新旧风格统一。

---

**总计：30 个任务，分 6 个 Phase。**
