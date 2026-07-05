# Yomi Agent 架构重构：Loopy-Inspired 设计

## 设计目标

1. 提取 `AgentLoop` 纯执行引擎，主/子 agent 完全一致
2. 每个功能独立 trait（端口），`AgentLoop` 只依赖 trait
3. 事件持久化从硬编码改为 Hook Listener
4. 删除 `SimpleAgent`，`SubagentTool` 直接复用 `AgentLoop`

---

## 一、端口 Trait 设计（核心抽象）

### 1. `History` — 对话历史管理

```rust
#[async_trait]
pub trait History: Send + Sync {
    /// 构建完整消息列表（包含 system prompt + 所有步骤）
    fn build(&self) -> Vec<Arc<Message>>;
    
    /// 追加单条消息
    fn append(&mut self, msg: Arc<Message>);
    
    /// 原子替换所有非 system 消息（用于 compaction / rewind / clear）
    fn replace(&mut self, msgs: Vec<Arc<Message>>);
    
    /// 获取所有消息引用
    fn messages(&self) -> &[Arc<Message>];
    
    /// 获取消息数量
    fn len(&self) -> usize;
    
    /// 清空（保留 system prompt）
    fn clear(&mut self);
    
    /// 清理非法状态（如孤立 tool result 等）
    fn sanitize(&mut self);
    
    /// 截断到指定 message_id 之前（rewind 用）
    fn truncate_at(&mut self, message_id: &MessageId) -> bool;
}
```

**实现**：
- `MemoryHistory` — 包装现有的 `MessageBuffer`（`Vec<Arc<Message>>` + `Mutex`）
- `PersistentHistory` — 包装 `MessageStore`，每次 `build` 时从存储加载，append/replace 时同步写入

---

### 2. `Mailbox` — 消息队列（替代 `AgentInput`）

```rust
pub struct Mail {
    pub content: Vec<ContentBlock>,
    /// Generation counter for cancellation fencing
    pub generation: u64,
    /// 可选：steer 消息标志
    pub is_steer: bool,
}

#[async_trait]
pub trait Mailbox: Send + Sync {
    /// 阻塞拉取一条消息（None = 队列关闭）
    async fn pull(&mut self) -> Result<Option<Mail>, MailboxError>;
    
    /// 非阻塞尝试拉取
    fn try_pull(&mut self) -> Result<Option<Mail>, MailboxError>;
    
    /// 推送消息
    async fn push(&self, mail: Mail) -> Result<(), MailboxError>;
    
    /// 是否已关闭
    fn is_closed(&self) -> bool;
    
    /// 关闭邮箱
    fn close(&mut self);
}

pub enum MailboxError {
    Empty,
    Closed,
    Full,
}
```

**实现**：
- `ChannelMailbox` — 包装 `tokio::sync::mpsc`（`Agent` 的 `input_rx` + `steer_rx` 合并适配）
- `DirectMailbox` — 直接持有 `Vec<Mail>`（用于 `SubagentTool` 直接注入）

---

### 3. `EventStore` — 事件/消息持久化（扩展 `MessageStore`）

```rust
#[async_trait]
pub trait EventStore: Send + Sync {
    /// 追加单条消息（append-only）
    async fn append_message(&self, session_id: &str, message: &Message) -> Result<()>;
    
    /// 批量追加
    async fn append_messages(&self, session_id: &str, messages: &[Message]) -> Result<()>;
    
    /// 原子替换所有消息（compaction / rewind / clear）
    async fn replace_messages(&self, session_id: &str, messages: &[Message]) -> Result<()>;
    
    /// 加载所有消息
    async fn load_messages(&self, session_id: &str) -> Result<Vec<Message>>;
    
    /// 清空
    async fn clear(&self, session_id: &str) -> Result<()>;
}
```

**实现**：
- `JsonlEventStore` — 包装 `JsonlMessageStore`（JSON Lines 格式不变）
- `NoOpEventStore` — 空实现（用于 `SubagentTool` 不需要持久化时）

---

### 4. `HookRegistry` — 生命周期扩展（扩展现有）

```rust
#[async_trait]
pub trait HookRegistry: Send + Sync {
    // ===== Mutating hooks（可以修改 payload） =====
    
    /// 处理新消息前。可拦截、修改、跳过
    async fn run_pre_turn(&self, ctx: &LoopContext, payload: &mut PreTurnPayload) -> PreTurnResult;
    
    /// 请求模型前。可修改历史、注入 steer
    async fn run_pre_model(&self, ctx: &LoopContext, payload: &mut PreModelPayload) -> PreModelResult;
    
    /// 工具执行前。可拦截、修改参数（已有 `PreToolUse`）
    async fn run_pre_tool(&self, ctx: &LoopContext, payload: &mut PreToolPayload) -> PreToolResult;
    
    /// 压缩前。可阻止压缩
    async fn run_pre_compact(&self, ctx: &LoopContext, payload: &mut PreCompactPayload) -> PreCompactResult;
    
    // ===== Observation hooks（只通知，不修改） =====
    
    async fn run_post_turn(&self, ctx: &LoopContext, payload: &PostTurnPayload);
    async fn run_post_model(&self, ctx: &LoopContext, payload: &PostModelPayload);
    async fn run_post_tool(&self, ctx: &LoopContext, payload: &PostToolPayload);
    async fn run_post_compact(&self, ctx: &LoopContext, payload: &PostCompactPayload);
    async fn run_loop_start(&self, ctx: &LoopContext, payload: &LoopStartPayload);
    async fn run_loop_end(&self, ctx: &LoopContext, payload: &LoopEndPayload);
    async fn run_on_error(&self, ctx: &LoopContext, payload: &OnErrorPayload);
    
    // ===== Listener 注册（用于事件投影、持久化） =====
    fn add_listener(&mut self, listener: Box<dyn HookListener>);
}

pub trait HookListener: Send + Sync {
    fn on_event(&self, ctx: &LoopContext, event: &HookEvent);
}

/// 统一的 Hook 事件类型（供 Listener 消费）
pub enum HookEvent {
    LoopStart(LoopStartPayload),
    LoopEnd(LoopEndPayload),
    PreTurn(PreTurnPayload),
    PostTurn(PostTurnPayload),
    PreModel(PreModelPayload),
    PostModel(PostModelPayload),
    PreTool(PreToolPayload),
    PostTool(PostToolPayload),
    PreCompact(PreCompactPayload),
    PostCompact(PostCompactPayload),
    OnError(OnErrorPayload),
}
```

**Payload 定义**：

```rust
pub struct PreTurnPayload {
    pub message: Arc<Message>,
}

pub struct PreTurnResult {
    pub skip: bool,
    pub message: Option<Arc<Message>>, // 替换后的消息（None = 保持原样）
}

pub struct PreModelPayload {
    pub messages: Vec<Arc<Message>>,
}

pub struct PreModelResult {
    pub messages: Vec<Arc<Message>>,
}

pub struct PostModelPayload {
    pub message: Arc<Message>,
    pub token_usage: Option<TokenUsage>,
    pub finish_reason: Option<FinishReason>,
}

pub struct PostTurnPayload {
    pub message: Arc<Message>,
}

pub struct PreToolPayload {
    pub tool_calls: Vec<ToolCall>,
}

pub struct PreToolResult {
    pub approved: Vec<ToolCall>,
    pub denied: Vec<(String, String)>, // (tool_call_id, error_msg)
}

pub struct PostToolPayload {
    pub results: Vec<ToolExecutionResult>,
    pub continue_session: bool,
    pub contexts: Vec<String>,
}

pub struct PreCompactPayload {
    pub messages: Vec<Arc<Message>>,
}

pub struct PreCompactResult {
    pub allow: bool,
}

pub struct PostCompactPayload {
    pub old_count: usize,
    pub new_count: usize,
    pub result: Option<CompactionResult>,
}

pub struct LoopStartPayload {
    pub session_id: String,
    pub agent_id: AgentId,
}

pub struct LoopEndPayload {
    pub session_id: String,
    pub agent_id: AgentId,
    pub reason: LoopEndReason,
}

pub struct OnErrorPayload {
    pub phase: ErrorPhase,
    pub error: String,
    pub is_recoverable: bool,
}
```

**实现**：
- `DefaultHookRegistry` — 维护 `Vec<Arc<dyn HookHandler>>`（现有 mutating hooks） + `Vec<Box<dyn HookListener>>`（新增 observation hooks）
- 保留 `CommandHookHandler`、`SkillHookHandler`、`GoalPreStopHandler` 等现有实现

---

### 5. `TurnTracker` — 回合跟踪（替代直接 `Turn` 使用）

```rust
pub trait TurnTracker: Send + Sync {
    /// 启动新回合
    fn start_turn(&mut self, user_msg_id: &MessageId, summary: &str) -> Option<Arc<Turn>>;
    
    /// 获取当前回合
    fn current_turn(&self) -> Option<Arc<Turn>>;
    
    /// 完成当前回合（创建 checkpoint）
    async fn complete_turn(&mut self) -> Result<()>;
    
    /// 取消当前回合（清理 checkpoint）
    async fn cancel_turn(&mut self) -> Result<()>;
    
    /// 重置
    fn reset(&mut self);
}
```

**实现**：
- `CheckpointTurnTracker` — 包装现有的 `Turn` + `CheckpointStore` 逻辑
- `NoOpTurnTracker` — 空实现（用于 `SubagentTool` 不需要 checkpoint 时）

---

### 6. `ModelClient` — 模型调用（统一 Provider 接口）

```rust
#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn stream(
        &self,
        messages: &[Arc<Message>],
        tools: &[Arc<ToolDefinition>],
        config: &ModelConfig,
    ) -> Result<ModelStream, ProviderError>;
}
```

**实现**：
- `ProviderModelClient` — 包装现有的 `Arc<dyn Provider>`

---

### 7. `Compactor` — 上下文压缩（已有，保持不变）

```rust
pub trait Compactor: Send + Sync {
    fn should_compact(&self, messages: &[Arc<Message>]) -> bool;
    async fn auto_compact(
        &self,
        messages: &[Arc<Message>],
        provider: Arc<dyn Provider>,
        model_config: &ModelConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Result<Option<CompactionResult>, CompactionError>;
    async fn full_compact(
        &self,
        messages: &[Arc<Message>],
        provider: Arc<dyn Provider>,
        model_config: &ModelConfig,
        cancel_token: Option<CancellationToken>,
    ) -> Result<CompactionResult, CompactionError>;
}
```

---

### 8. `PermissionChecker` — 权限检查（替代现有 `Checker`）

```rust
pub struct PermissionResult {
    pub approved: Vec<ToolCall>,
    pub denied: Vec<(String, String)>, // (tool_call_id, error_msg)
}

#[async_trait]
pub trait PermissionChecker: Send + Sync {
    async fn check(&self, tool_calls: &[ToolCall], agent_id: &AgentId) -> PermissionResult;
}
```

**实现**：
- `ConfigPermissionChecker` — 包装现有 `Checker` + `PermissionState`

---

### 9. `EventEmitter` — 事件发送（替代硬编码 `event_bus.try_send`）

```rust
pub trait EventEmitter: Send + Sync {
    fn emit(&self, event: Event) -> Result<(), EventEmitterError>;
    fn try_emit(&self, event: Event) -> Result<(), EventEmitterError>;
}

pub enum EventEmitterError {
    Full,
    Closed,
}
```

**实现**：
- `EventBusEmitter` — 包装 `EventBusHandle`
- `NoOpEventEmitter` — 空实现（用于 `SubagentTool` 不需要事件时）

---

### 10. `UsageRecorder` — Token 使用记录

```rust
#[async_trait]
pub trait UsageRecorder: Send + Sync {
    async fn record(
        &self,
        session_id: &SessionId,
        agent_id: &AgentId,
        usage: TokenUsage,
        model_id: &str,
        provider: &str,
        usage_type: UsageType,
    ) -> Result<()>;
}
```

**实现**：
- `SqliteUsageRecorder` — 包装 `UsageStore`
- `NoOpUsageRecorder` — 空实现

---

## 二、AgentLoop 引擎设计

### 核心结构

```rust
/// 纯执行引擎：无状态机、无生命周期、无 Handle
/// 主 agent 和子 agent 的核心执行逻辑完全一致
pub struct AgentLoop {
    config: LoopConfig,
    step_count: usize,
}

pub struct LoopConfig {
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub system_prompt: String,
    pub model: Arc<dyn ModelClient>,
    pub model_config: Arc<ModelConfig>,
    pub tools: ToolRegistry,
    pub compactor: Option<Arc<dyn Compactor>>,
    pub max_steps: usize,
    pub max_context_tokens: usize,
    pub tool_timeout: Duration,
    pub history: Box<dyn History>,
    pub hooks: Arc<dyn HookRegistry>,
    pub turn_tracker: Box<dyn TurnTracker>,
    pub event_store: Option<Arc<dyn EventStore>>,
    pub usage_recorder: Option<Arc<dyn UsageRecorder>>,
    pub data_dir: PathBuf,
    pub working_dir: PathBuf,
    pub skills: Vec<Arc<Skill>>,
    pub max_tool_output_length: usize,
    pub permission_checker: Option<Arc<dyn PermissionChecker>>,
    pub stream_retry_config: StreamRetryConfig,
}

pub struct LoopContext {
    pub cancel_token: CancelToken,
    pub is_cancelled: bool,
}

pub struct StreamRetryConfig {
    pub max_retries: u32,
    pub base_delay_secs: u64,
}

pub enum TurnResult {
    Completed { finish_reason: Option<FinishReason> },
    Stopped, // Hook 请求停止
    Skipped, // PreTurn hook 跳过
    MaxStepsReached,
}
```

### 核心方法

```rust
impl AgentLoop {
    /// 执行一个完整的用户消息 turn（streaming + tool execution + 内部 continue）
    /// 
    /// 这是 `Agent::handle_streaming` + `handle_execute_tool` + `transition_after_streaming` 的提取
    pub async fn run_turn(
        &mut self,
        ctx: &mut LoopContext,
        user_msg: Arc<Message>,
    ) -> Result<TurnResult, LoopError> {
        // 1. PreTurn hook
        let mut pre_payload = PreTurnPayload { message: user_msg.clone() };
        let pre_result = self.config.hooks.run_pre_turn(ctx, &mut pre_payload).await;
        if pre_result.skip {
            return Ok(TurnResult::Skipped);
        }
        let msg_to_append = pre_result.message.unwrap_or(user_msg);
        
        // 2. Append to history & persist
        self.history.append(msg_to_append.clone());
        self.persist_message(&msg_to_append).await;
        
        // 3. Start turn tracking
        let summary = extract_summary(&msg_to_append.content);
        self.config.turn_tracker.start_turn(&msg_to_append.id, &summary);
        
        // 4. Loop: streaming → tools → continue
        let mut step_count = 0;
        loop {
            if ctx.is_cancelled {
                return Err(LoopError::Cancelled);
            }
            if step_count >= self.config.max_steps {
                self.config.turn_tracker.complete_turn().await.ok();
                return Ok(TurnResult::MaxStepsReached);
            }
            step_count += 1;
            
            // 4a. Check & compact
            self.check_and_compact(ctx).await?;
            
            // 4b. Build history
            let mut messages = self.history.build();
            messages = resolve_assets(&messages, &self.config.data_dir).await;
            
            // 4c. PreModel hook
            let mut pre_model = PreModelPayload { messages };
            let pre_model_result = self.config.hooks.run_pre_model(ctx, &mut pre_model).await;
            
            // 4d. Stream model
            let tools = self.config.tools.definitions();
            let assistant_msg_id = MessageId::new();
            let stream_result = self.stream_with_retry(
                ctx,
                &pre_model_result.messages,
                &tools,
                assistant_msg_id.clone(),
            ).await?;
            
            // 4e. PostModel hook
            self.config.hooks.run_post_model(ctx, &PostModelPayload {
                message: stream_result.message.clone(),
                token_usage: stream_result.token_usage.clone(),
                finish_reason: stream_result.finish_reason,
            }).await;
            
            // 4f. Append assistant message & persist
            self.history.append(stream_result.message.clone());
            self.persist_message(&stream_result.message).await;
            
            // 4g. Handle tool calls
            if let Some(tool_calls) = stream_result.message.tool_calls {
                let should_continue = self.execute_tools(ctx, &tool_calls, &assistant_msg_id).await?;
                if !should_continue {
                    self.config.turn_tracker.complete_turn().await.ok();
                    return Ok(TurnResult::Stopped);
                }
                // Continue to next streaming
                continue;
            }
            
            // 4h. Handle finish_reason
            match stream_result.finish_reason {
                None | Some(FinishReason::MaxTokens) => {
                    // Auto-inject continue
                    let continue_msg = Message::user("continue");
                    self.history.append(continue_msg.clone());
                    self.persist_message(&continue_msg).await;
                    continue;
                }
                _ => {
                    // Check PreStop hooks
                    let pre_stop = self.config.hooks.run_pre_stop(ctx).await;
                    if pre_stop.continue_session {
                        if let Some(steer) = pre_stop.steer_blocks {
                            let steer_msg = Message::with_blocks(Role::User, steer);
                            self.history.append(steer_msg.clone());
                            self.persist_message(&steer_msg).await;
                        }
                        continue;
                    }
                    // Done
                    self.config.turn_tracker.complete_turn().await.ok();
                    return Ok(TurnResult::Completed {
                        finish_reason: stream_result.finish_reason,
                    });
                }
            }
        }
    }
    
    /// 带重试的 streaming
    async fn stream_with_retry(
        &mut self,
        ctx: &mut LoopContext,
        messages: &[Arc<Message>],
        tools: &[Arc<ToolDefinition>],
        assistant_msg_id: MessageId,
    ) -> Result<StreamResult, LoopError> {
        let max_retries = self.config.stream_retry_config.max_retries;
        let mut attempt = 0;
        loop {
            match self.do_stream(ctx, messages, tools, assistant_msg_id.clone()).await {
                Ok(result) => return Ok(result),
                Err(e) if e.is_cancelled() => return Err(e),
                Err(e) if !e.is_retryable() || attempt >= max_retries => return Err(e),
                Err(e) => {
                    attempt += 1;
                    self.config.hooks.run_on_error(ctx, &OnErrorPayload {
                        phase: ErrorPhase::Streaming,
                        error: e.to_string(),
                        is_recoverable: true,
                    }).await;
                    tokio::time::sleep(Duration::from_secs(
                        self.config.stream_retry_config.base_delay_secs * attempt as u64
                    )).await;
                }
            }
        }
    }
    
    /// 单次 streaming（包含 collect stream output）
    async fn do_stream(
        &mut self,
        ctx: &mut LoopContext,
        messages: &[Arc<Message>],
        tools: &[Arc<ToolDefinition>],
        assistant_msg_id: MessageId,
    ) -> Result<StreamResult, LoopError> {
        // 1. Start stream
        let stream = self.config.model.stream(messages, tools, &self.config.model_config).await
            .map_err(|e| LoopError::Provider(e))?;
        
        // 2. Collect output
        let mut collector = StreamCollector::new(assistant_msg_id.clone());
        let mut stream = stream;
        loop {
            tokio::select! {
                biased;
                () = ctx.cancel_token.cancelled() => {
                    return Err(LoopError::Cancelled);
                }
                item = stream.try_next() => {
                    match item {
                        Ok(Some(item)) => collector.handle_item(item),
                        Ok(None) => break,
                        Err(e) => return Err(LoopError::Provider(e)),
                    }
                }
            }
        }
        
        let result = collector.build_result();
        
        // 3. Record token usage
        if let Some(usage) = &result.token_usage {
            if let Some(recorder) = &self.config.usage_recorder {
                recorder.record(
                    &self.config.session_id,
                    &self.config.agent_id,
                    usage.clone(),
                    &self.config.model_config.model_id,
                    &self.config.model_config.provider.to_string(),
                    UsageType::Normal,
                ).await.ok();
            }
        }
        
        Ok(result)
    }
    
    /// 执行工具调用（并行）
    async fn execute_tools(
        &mut self,
        ctx: &mut LoopContext,
        tool_calls: &[ToolCall],
        assistant_msg_id: &MessageId,
    ) -> Result<bool, LoopError> {
        // 1. Permission check
        let permission_result = if let Some(checker) = &self.config.permission_checker {
            checker.check(tool_calls, &self.config.agent_id).await
        } else {
            PermissionResult { approved: tool_calls.to_vec(), denied: vec![] }
        };
        
        // 2. PreTool hooks
        let mut pre_payload = PreToolPayload { tool_calls: permission_result.approved };
        let pre_result = self.config.hooks.run_pre_tool(ctx, &mut pre_payload).await;
        
        // 3. Execute tools
        let cancel_token = ctx.cancel_token.runtime_token();
        let results = execute_tools_parallel(&ToolExecParams {
            agent_id: &self.config.agent_id,
            tool_calls: &pre_result.approved,
            tool_registry: &self.config.tools,
            cancel_token: Some(&cancel_token),
            parent_messages: Some(self.history.messages()),
            working_dir: &self.config.working_dir,
            session_id: &self.config.session_id.0,
            message_ids: &generate_tool_message_ids(&pre_result.approved),
            turn: self.config.turn_tracker.current_turn(),
            skills: &self.config.skills,
            max_tool_output_length: self.config.max_tool_output_length,
        }).await;
        
        // 4. PostTool hooks
        let mut post_payload = PostToolPayload {
            results: results.clone(),
            continue_session: true,
            contexts: vec![],
        };
        self.config.hooks.run_post_tool(ctx, &post_payload).await;
        
        // 5. Append results to history
        for result in &results {
            self.history.append(result.message.clone());
            self.persist_message(&result.message).await;
        }
        
        // 6. Append hook contexts
        for ctx_text in &post_payload.contexts {
            let msg = Message::user(ctx_text.clone());
            self.history.append(msg.clone());
            self.persist_message(&msg).await;
        }
        
        Ok(post_payload.continue_session)
    }
    
    /// 检查并执行 compaction
    async fn check_and_compact(&mut self, ctx: &mut LoopContext) -> Result<(), LoopError> {
        let Some(compactor) = &self.config.compactor else { return Ok(()) };
        if !compactor.should_compact(self.history.messages()) {
            return Ok(());
        }
        
        let mut pre_payload = PreCompactPayload {
            messages: self.history.build(),
        };
        let pre_result = self.config.hooks.run_pre_compact(ctx, &mut pre_payload).await;
        if !pre_result.allow {
            return Ok(());
        }
        
        let old_count = self.history.len();
        let result = compactor.auto_compact(
            self.history.messages(),
            self.config.model.clone(), // 需要 ModelClient 转 Provider，此处需要适配
            &self.config.model_config,
            Some(ctx.cancel_token.runtime_token()),
        ).await;
        
        match result {
            Ok(Some(compaction_result)) => {
                self.history.replace(compaction_result.messages);
                self.persist_all().await;
                
                // Clear file state if messages were reduced
                if old_count > self.history.len() {
                    // 通过 hook 通知 file state clear
                }
                
                self.config.hooks.run_post_compact(ctx, &PostCompactPayload {
                    old_count,
                    new_count: self.history.len(),
                    result: Some(compaction_result),
                }).await;
            }
            Ok(None) => {}
            Err(e) => {
                self.config.hooks.run_on_error(ctx, &OnErrorPayload {
                    phase: ErrorPhase::Compaction,
                    error: e.to_string(),
                    is_recoverable: false,
                }).await;
            }
        }
        
        Ok(())
    }
    
    /// 持久化单条消息
    async fn persist_message(&self, message: &Message) {
        if let Some(store) = &self.config.event_store {
            store.append_message(&self.config.session_id.0, message).await.ok();
        }
    }
    
    /// 持久化所有历史（compaction 后）
    async fn persist_all(&self) {
        if let Some(store) = &self.config.event_store {
            let messages: Vec<Message> = self.history.messages()
                .iter()
                .map(|m| (**m).clone())
                .collect();
            store.replace_messages(&self.config.session_id.0, &messages).await.ok();
        }
    }
}
```

---

## 三、Agent 包装层（保留状态机 + Handle）

```rust
pub struct Agent {
    loop_engine: AgentLoop,
    context: AgentExecutionContext, // 状态机 + iteration counter
    input_rx: mpsc::Receiver<AgentInput>,
    steer_rx: mpsc::Receiver<Vec<ContentBlock>>,
    cancel_token: CancelToken,
    input_stale_since: Arc<AtomicU64>,
    mailbox: Box<dyn Mailbox>, // 包装 input_rx + steer_rx
}

impl Agent {
    pub async fn spawn(
        id: AgentId,
        shared: &Arc<AgentShared>,
        args: AgentSpawnArgs,
    ) -> AgentHandle {
        // 1. 构建 LoopConfig
        let loop_config = build_loop_config(id.clone(), shared, &args).await;
        
        // 2. 创建 AgentLoop
        let loop_engine = AgentLoop::new(loop_config);
        
        // 3. 创建 Mailbox（包装 input_rx + steer_rx）
        let (mailbox, input_rx, steer_rx) = ChannelMailbox::new();
        
        // 4. 创建 Agent（包装层）
        let agent = Self {
            loop_engine,
            context: AgentExecutionContext::new(AgentState::Idle),
            input_rx,
            steer_rx,
            cancel_token: args.cancel_token.clone().unwrap_or_default(),
            input_stale_since: Arc::new(AtomicU64::new(0)),
            mailbox,
        };
        
        // 5. Spawn 任务
        tokio::spawn(agent.start_loop());
        
        // 6. 返回 Handle
        AgentHandle::new(...)
    }
    
    async fn start_loop(mut self) -> Result<(), AgentError> {
        loop {
            let state = self.context.current_state();
            if state.is_terminal() { break; }
            
            match state {
                AgentState::Idle => {
                    self.context.reset_iteration();
                    match self.mailbox.pull().await {
                        Ok(Some(mail)) => {
                            // 处理 generation fencing
                            // 构建 Message
                            let msg = Message::with_blocks(Role::User, mail.content);
                            let msg = Arc::new(msg);
                            
                            self.context.transition_to(AgentState::Streaming);
                            
                            // 调用 AgentLoop 引擎
                            let mut loop_ctx = LoopContext {
                                cancel_token: self.cancel_token.clone(),
                                is_cancelled: false,
                            };
                            match self.loop_engine.run_turn(&mut loop_ctx, msg).await {
                                Ok(TurnResult::Completed { .. }) => {
                                    self.context.transition_to(AgentState::Idle);
                                }
                                Ok(TurnResult::Stopped) => {
                                    self.context.transition_to(AgentState::Idle);
                                }
                                Ok(TurnResult::MaxStepsReached) => {
                                    // 发送 max iterations 事件
                                    self.context.transition_to(AgentState::Idle);
                                }
                                Ok(TurnResult::Skipped) => {
                                    self.context.transition_to(AgentState::Idle);
                                }
                                Err(e) => {
                                    // 错误处理
                                    self.context.transition_to(AgentState::Idle);
                                }
                            }
                        }
                        Ok(None) | Err(MailboxError::Closed) => {
                            self.context.transition_to(AgentState::Closed);
                        }
                        _ => {}
                    }
                }
                // Streaming / ExecutingTool / Compacting 不再作为独立状态
                // 所有执行逻辑在 AgentLoop::run_turn 内部完成
                _ => {
                    tracing::warn!("Unexpected state {:?} in Agent wrapper", state);
                    self.context.transition_to(AgentState::Idle);
                }
            }
        }
        Ok(())
    }
}
```

**状态机简化**：`AgentState::Streaming` / `ExecutingTool` / `Compacting` 在 `AgentLoop` 内部处理，`Agent` 包装层只保留 `Idle` / `Closed` 两个外部可见状态。

---

## 四、SubagentTool 重构

```rust
impl Tool for SubagentTool {
    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let task = parse_task(&args)?;
        let preset = parse_preset(&args)?;
        
        // 1. 构建子 agent 的 LoopConfig
        let loop_config = self.build_subagent_config(preset, &ctx).await?;
        
        // 2. 创建 AgentLoop（和主 agent 完全一致的核心引擎）
        let mut loop_engine = AgentLoop::new(loop_config);
        
        // 3. 构建用户消息
        let user_msg = Arc::new(Message::user(task));
        
        // 4. 直接执行 turn（streaming + tool execution + compaction 全部生效）
        let mut loop_ctx = LoopContext {
            cancel_token: ctx.cancel_token.clone().unwrap_or_default(),
            is_cancelled: false,
        };
        
        match loop_engine.run_turn(&mut loop_ctx, user_msg).await {
            Ok(TurnResult::Completed { .. } | TurnResult::Stopped | TurnResult::MaxStepsReached) => {
                // 5. 从 history 收集结果
                let history = loop_engine.history();
                let result = self.format_result(history);
                Ok(ToolOutput::text(result))
            }
            Ok(TurnResult::Skipped) => {
                Ok(ToolOutput::error("Subagent turn was skipped by hook"))
            }
            Err(e) => {
                Ok(ToolOutput::error(format!("Subagent failed: {}", e)))
            }
        }
    }
}
```

**关键**：`SubagentTool` 不再创建 `SimpleAgent`，而是直接创建 `AgentLoop`。`AgentLoop` 的代码路径和 `Agent` 内部完全一致，因此子 agent 自动获得：
- 并行工具执行（`execute_tools_parallel`）
- Pre/Post Tool hooks
- Compaction
- 权限检查（如果配置了 `PermissionChecker`）
- Token usage 记录
- 所有事件通过 `HookRegistry` 的 listener 处理

---

## 五、Hook Listener 实现（事件投影）

### 1. `EventPersistListener` — 消息持久化

```rust
pub struct EventPersistListener {
    store: Arc<dyn EventStore>,
}

impl HookListener for EventPersistListener {
    fn on_event(&self, ctx: &LoopContext, event: &HookEvent) {
        match event {
            HookEvent::PostModel(payload) => {
                // 持久化 assistant message
                self.store.append_message(&ctx.session_id.0, &payload.message).blocking_ok();
            }
            HookEvent::PostTool(payload) => {
                // 持久化 tool results
                for result in &payload.results {
                    self.store.append_message(&ctx.session_id.0, &result.message).blocking_ok();
                }
            }
            // 其他事件不持久化
            _ => {}
        }
    }
}
```

### 2. `TuiEventListener` — TUI 事件转发

```rust
pub struct TuiEventListener {
    event_bus: EventBusHandle,
    agent_id: AgentId,
}

impl HookListener for TuiEventListener {
    fn on_event(&self, ctx: &LoopContext, event: &HookEvent) {
        match event {
            HookEvent::LoopStart(_) => {
                self.emit(Event::Agent(AgentEvent::Lifecycle {
                    agent_id: self.agent_id.clone(),
                    state: AgentStatus::Running,
                }));
            }
            HookEvent::PostModel(payload) => {
                // 发送 Chunk / TokenUsage / Completed / End 事件
                // 从 StreamResult 中重建事件
            }
            HookEvent::PostTool(payload) => {
                // 发送 ToolEvent::Start / End
            }
            HookEvent::OnError(payload) => {
                self.emit(Event::Agent(AgentEvent::Error { ... }));
            }
            // ...
        }
    }
}
```

### 3. `UsageRecordListener` — Token 使用记录

```rust
pub struct UsageRecordListener {
    recorder: Arc<dyn UsageRecorder>,
    session_id: SessionId,
    agent_id: AgentId,
    model_id: String,
    provider: String,
}

impl HookListener for UsageRecordListener {
    fn on_event(&self, ctx: &LoopContext, event: &HookEvent) {
        if let HookEvent::PostModel(payload) = event {
            if let Some(usage) = &payload.token_usage {
                self.recorder.record(
                    &self.session_id,
                    &self.agent_id,
                    usage.clone(),
                    &self.model_id,
                    &self.provider,
                    UsageType::Normal,
                ).blocking_ok();
            }
        }
    }
}
```

---

## 六、依赖关系图

```
┌─────────────────────────────────────────────────────────────┐
│                        Agent (包装层)                          │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  AgentLoop (纯执行引擎)                               │    │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐ │    │
│  │  │ History │  │Mailbox  │  │HookReg. │  │TurnTrack│ │    │
│  │  │ (trait) │  │ (trait) │  │ (trait) │  │ (trait) │ │    │
│  │  └─────────┘  └─────────┘  └─────────┘  └─────────┘ │    │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐              │    │
│  │  │EventStore│  │ModelCl. │  │Compactor│              │    │
│  │  │ (trait) │  │ (trait) │  │ (trait) │              │    │
│  │  └─────────┘  └─────────┘  └─────────┘              │    │
│  └─────────────────────────────────────────────────────┘    │
│  状态机: Idle → Closed                                        │
│  Handle: AgentHandle (input_tx, state_rx, cancel, steer)   │
└─────────────────────────────────────────────────────────────┘
                              │
                              │  Agent::spawn
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                    SubagentTool (工具)                        │
│  ┌─────────────────────────────────────────────────────┐    │
│  │  AgentLoop (纯执行引擎) — 和主 agent 完全一致          │    │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐  ┌─────────┐ │    │
│  │  │ History │  │DirectMb.│  │HookReg. │  │NoOpTurn │ │    │
│  │  │ (trait) │  │ (trait) │  │ (trait) │  │ (trait) │ │    │
│  │  └─────────┘  └─────────┘  └─────────┘  └─────────┘ │    │
│  │  ┌─────────┐  ┌─────────┐  ┌─────────┐              │    │
│  │  │NoOpStore │  │ModelCl. │  │Compactor│              │    │
│  │  │ (trait) │  │ (trait) │  │ (trait) │              │    │
│  │  └─────────┘  └─────────┘  └─────────┘              │    │
│  └─────────────────────────────────────────────────────┘    │
│  无状态机、无 Handle、无 Mailbox channel                      │
│  直接调用: loop.run_turn(user_msg) → 收集结果 → 返回         │
└─────────────────────────────────────────────────────────────┘
```

---

## 七、接口 vs 实现对照表

| Trait | 主 Agent 实现 | SubagentTool 实现 | 说明 |
|-------|--------------|-------------------|------|
| `History` | `MemoryHistory` | `MemoryHistory` | 内存中的 `Vec<Arc<Message>>` |
| `Mailbox` | `ChannelMailbox` | `DirectMailbox` | 主 agent 用 channel，子 agent 直接注入 |
| `EventStore` | `JsonlEventStore` | `NoOpEventStore` | 子 agent 默认不持久化 |
| `HookRegistry` | `DefaultHookRegistry` + 所有 listener | `DefaultHookRegistry` + 精简 listener | 子 agent 可配置是否启用 TUI 事件 |
| `TurnTracker` | `CheckpointTurnTracker` | `NoOpTurnTracker` | 子 agent 不需要 checkpoint |
| `ModelClient` | `ProviderModelClient` | `ProviderModelClient` | 复用 parent 的 provider |
| `Compactor` | `ChainCompactor` | `ChainCompactor` 或 `None` | 可选 |
| `PermissionChecker` | `ConfigPermissionChecker` | `ConfigPermissionChecker` 或 `None` | 可选 |
| `UsageRecorder` | `SqliteUsageRecorder` | `NoOpUsageRecorder` | 子 agent 默认不记录 |

---

## 八、详细执行任务表

### Phase 1: 基础设施 — 定义所有端口 Trait

#### Task 1.1: 创建 `agent/loop/ports.rs` — 定义所有端口 trait
- 定义 `History` trait（含 `build`, `append`, `replace`, `messages`, `len`, `clear`, `sanitize`, `truncate_at`）
- 定义 `Mailbox` trait（含 `pull`, `try_pull`, `push`, `is_closed`, `close`）
- 定义 `Mail` struct（含 `content`, `generation`, `is_steer`）
- 定义 `MailboxError` enum
- 定义 `EventStore` trait（含 `append_message`, `append_messages`, `replace_messages`, `load_messages`, `clear`）
- 定义 `TurnTracker` trait（含 `start_turn`, `current_turn`, `complete_turn`, `cancel_turn`, `reset`）
- 定义 `ModelClient` trait（含 `stream`）
- 定义 `PermissionChecker` trait（含 `check`）
- 定义 `PermissionResult` struct
- 定义 `UsageRecorder` trait（含 `record`）
- 定义 `UsageType` enum（复用或迁移现有）

#### Task 1.2: 创建 `agent/loop/hook_payloads.rs` — 定义 Hook Payload 类型
- 定义 `PreTurnPayload` / `PreTurnResult`
- 定义 `PostTurnPayload`
- 定义 `PreModelPayload` / `PreModelResult`
- 定义 `PostModelPayload`（含 `message`, `token_usage`, `finish_reason`）
- 定义 `PreToolPayload` / `PreToolResult`（含 `approved`, `denied`）
- 定义 `PostToolPayload`（含 `results`, `continue_session`, `contexts`）
- 定义 `PreCompactPayload` / `PreCompactResult`
- 定义 `PostCompactPayload`（含 `old_count`, `new_count`, `result`）
- 定义 `LoopStartPayload` / `LoopEndPayload`（含 `session_id`, `agent_id`, `reason`）
- 定义 `OnErrorPayload`（含 `phase`, `error`, `is_recoverable`）
- 定义 `LoopEndReason` enum

#### Task 1.3: 扩展 `hooks/mod.rs` — 扩展 `HookRegistry` trait
- 扩展现有 `HookRegistry` 接口（或新建 `agent/loop/hook_registry.rs`）
- 添加所有生命周期方法（`run_pre_turn`, `run_post_turn`, `run_pre_model`, `run_post_model`, `run_pre_tool`, `run_post_tool`, `run_pre_compact`, `run_post_compact`, `run_loop_start`, `run_loop_end`, `run_on_error`）
- 添加 `HookListener` trait 和 `add_listener` 方法
- 定义 `HookEvent` enum（统一的事件类型）
- 保留现有 `HookHandler`（`CommandHookHandler`, `SkillHookHandler` 等）的兼容性

#### Task 1.4: 创建 `agent/loop/mod.rs` — 模块组织
- 导出 `AgentLoop`, `LoopConfig`, `LoopContext`, `LoopError`, `TurnResult`
- 导出所有端口 trait
- 导出所有 hook payload 类型

#### Task 1.5: 创建 `agent/loop/error.rs` — Loop 错误类型
- 定义 `LoopError` enum（`Cancelled`, `Provider`, `ToolExecution`, `Compaction`, `MaxSteps`, `MailboxClosed`）
- 实现 `is_cancelled()` 和 `is_retryable()` 方法
- 实现 `From<ProviderError>` 等转换

---

### Phase 2: 端口适配器实现

#### Task 2.1: 实现 `MemoryHistory` — 包装 `MessageBuffer`
- 创建 `agent/loop/history.rs`
- 实现 `History` trait，内部持有 `Vec<Arc<Message>>` + `Mutex`
- 实现 `sanitize` 方法（从 `MessageBuffer` 迁移）
- 实现 `truncate_at` 方法（从 `Agent::truncate_at` 迁移）
- 提供 `from_arc_messages` 构造函数
- 保留 system prompt 的过滤逻辑

#### Task 2.2: 实现 `PersistentHistory` — 可选的持久化 history
- 在 `history.rs` 中实现
- 内部持有 `MemoryHistory` + `Arc<dyn EventStore>`
- `append` 时同时写入内存和 `EventStore`
- `replace` 时同时替换内存和 `EventStore`
- `build` 时直接从内存返回（假设内存和存储一致）
- 这个可以延后到 Phase 7，Phase 2 先用 `MemoryHistory`

#### Task 2.3: 实现 `ChannelMailbox` — 包装 `mpsc` channel
- 创建 `agent/loop/mailbox.rs`
- 实现 `Mailbox` trait
- 内部持有 `mpsc::Receiver<AgentInput>` 和 `mpsc::Receiver<Vec<ContentBlock>>`
- `pull` 时 select 两个 channel，将 `AgentInput::User` 和 steer 都映射为 `Mail`
- `push` 对应 `AgentHandle` 的 `send_message`（需要反向引用或外部持有 `Sender`）
- 注意：Mailbox 的 `push` 在 `Agent` 中不是直接用的，而是通过 `AgentHandle`

#### Task 2.4: 实现 `DirectMailbox` — 直接注入的 mailbox
- 在 `mailbox.rs` 中实现
- 内部持有 `Vec<Mail>` 或 `Option<Mail>`
- `pull` 直接返回预存的消息
- 用于 `SubagentTool` 直接注入用户消息

#### Task 2.5: 实现 `JsonlEventStore` — 包装 `JsonlMessageStore`
- 创建 `storage/event_store.rs` 或在 `storage/message/` 下实现
- 实现 `EventStore` trait，委托给 `JsonlMessageStore`
- `append_message` → `JsonlMessageStore::append`
- `replace_messages` → `JsonlMessageStore::replace`
- `load_messages` → `JsonlMessageStore::get`
- 处理图片内联提取（复用现有 `resolve_messages` 逻辑）

#### Task 2.6: 实现 `NoOpEventStore` — 空实现
- 在 `storage/event_store.rs` 中实现
- 所有方法为空操作，返回 `Ok(())`
- 用于 `SubagentTool` 不需要持久化时

#### Task 2.7: 实现 `CheckpointTurnTracker` — 包装 `Turn`
- 创建 `agent/loop/turn_tracker.rs`
- 实现 `TurnTracker` trait
- 内部持有 `Option<Arc<Turn>>` + `CheckpointStore` + `data_dir`
- `start_turn` 调用 `Turn::new`
- `complete_turn` 调用 `Turn::complete`
- `cancel_turn` 调用 `Turn::cancel`

#### Task 2.8: 实现 `NoOpTurnTracker` — 空实现
- 在 `turn_tracker.rs` 中实现
- 所有方法为空操作
- 用于 `SubagentTool` 不需要 checkpoint 时

#### Task 2.9: 实现 `ProviderModelClient` — 包装 `Provider`
- 创建 `providers/model_client.rs` 或 `agent/loop/model_client.rs`
- 实现 `ModelClient` trait
- 内部持有 `Arc<dyn Provider>`
- `stream` 方法委托给 `Provider::stream`

#### Task 2.10: 实现 `ConfigPermissionChecker` — 包装 `Checker`
- 创建 `permissions/checker_adapter.rs` 或直接在 `permissions/` 下实现
- 实现 `PermissionChecker` trait
- 内部持有 `Arc<Checker>` 或 `PermissionState`
- `check` 方法调用 `check_tool_permissions`

#### Task 2.11: 实现 `SqliteUsageRecorder` — 包装 `UsageStore`
- 在 `storage/usage/` 下实现
- 实现 `UsageRecorder` trait
- 内部持有 `Arc<dyn UsageStore>`
- `record` 方法构建 `UsageRecord` 并调用 `store.record`

#### Task 2.12: 实现 `NoOpUsageRecorder` — 空实现
- 在 `storage/usage/` 下实现
- 所有方法为空操作

---

### Phase 3: 提取 AgentLoop 引擎

#### Task 3.1: 创建 `agent/loop/agent_loop.rs` — 定义 `AgentLoop` struct 和 `LoopConfig`
- 定义 `AgentLoop` struct（含 `config: LoopConfig`, `step_count: usize`）
- 定义 `LoopConfig` struct（含所有端口 trait 对象）
- 定义 `LoopContext` struct（含 `cancel_token`, `is_cancelled`）
- 定义 `TurnResult` enum（`Completed`, `Stopped`, `Skipped`, `MaxStepsReached`）
- 定义 `StreamResult` struct（从 `StreamCollectionResult` 迁移）
- 提供 `AgentLoop::new(config)` 构造函数

#### Task 3.2: 从 `Agent::handle_streaming` 提取 `AgentLoop::do_stream`
- 分析 `Agent::handle_streaming` 的 130 行代码
- 提取为 `AgentLoop::do_stream` 方法：
  - 构建 streaming 请求（`model.stream()`）
  - collect stream output（处理 `ModelStreamItem` 的各种 variant）
  - 构建 `Message`（`Role::Assistant`，包含 `content_blocks`, `tool_calls`, `token_usage`, `response_id`, `finish_reason`）
  - 返回 `StreamResult`
- 注意：事件发送（`Chunk`, `TokenUsage`, `Fallback`, `ToolCallDelta`, `Completed`, `End`）改为通过 `HookRegistry::run_post_model` 触发，内部不再硬编码 `event_bus.try_send`
- 注意：token usage 记录改为通过 `HookRegistry` 的 `UsageRecordListener` 触发
- 注意：model request 事件改为通过 `HookRegistry::run_pre_model` 或 `run_loop_start` 触发

#### Task 3.3: 从 `Agent::collect_stream_output` 提取 `StreamCollector`
- 将 `StreamCollectorState` 从 `stream_collector` 模块迁移或复用
- 在 `AgentLoop` 内部使用 `StreamCollector` 收集 streaming 输出
- 处理所有 `ModelStreamItem` variant：`Chunk`, `ToolCallDelta`, `ToolCall`, `Complete`, `Fallback`, `TokenUsage`, `ResponseMeta`
- `TokenUsage` 处理时，通过 `HookRegistry` 触发 `PostModel` 事件（携带 `token_usage`），由 `UsageRecordListener` 记录

#### Task 3.4: 从 `Agent::handle_streaming_with_retry` 提取 `AgentLoop::stream_with_retry`
- 提取重试逻辑到 `AgentLoop::stream_with_retry`
- 处理 `Cancelled`, `Retryable`, `NonRetryable` 错误分类
- 重试间隔：base_delay_secs * attempt
- 重试前触发 `OnError` hook（`is_recoverable: true`）
- 达到最大重试次数后返回 `LoopError`

#### Task 3.5: 从 `Agent::handle_execute_tool` 提取 `AgentLoop::execute_tools`
- 提取工具执行逻辑到 `AgentLoop::execute_tools`：
  - 权限检查（调用 `PermissionChecker::check`）
  - PreTool hooks（调用 `HookRegistry::run_pre_tool`）
  - 并行工具执行（调用 `execute_tools_parallel`）
  - PostTool hooks（调用 `HookRegistry::run_post_tool`）
  - 追加 tool results 到 history
  - 追加 hook contexts 到 history
- 注意：ToolEvent::Start/End 的硬编码发送改为通过 `HookRegistry` 的 `PostTool` 事件触发
- 注意：返回 `bool`（`continue_session`）

#### Task 3.6: 从 `Agent::maybe_compact_messages` 和 `force_compact` 提取 `AgentLoop::check_and_compact`
- 提取 compaction 逻辑到 `AgentLoop::check_and_compact`：
  - 检查 `Compactor::should_compact`
  - PreCompact hook（调用 `HookRegistry::run_pre_compact`）
  - 执行 `Compactor::auto_compact`
  - 更新 history（`History::replace`）
  - 持久化更新后的消息（`EventStore::replace_messages`）
  - 如果消息减少，清除 file state（通过 `PostCompact` hook 通知）
  - PostCompact hook（调用 `HookRegistry::run_post_compact`）
- 注意：compaction 的 `active`/`inactive` 事件发送改为通过 `HookRegistry` 触发
- 注意：compactor token usage 记录改为通过 `HookRegistry` 触发

#### Task 3.7: 从 `Agent::transition_after_streaming` 提取 `AgentLoop::run_turn` 的尾部逻辑
- 提取状态转换逻辑到 `AgentLoop::run_turn` 的尾部：
  - 检查 `tool_calls` → 执行工具 → 继续循环
  - 检查 `finish_reason`（None / MaxTokens）→ 注入 "continue" → 继续循环
  - 检查 `PreStop` hooks → 继续或停止
  - 完成 → 返回 `TurnResult::Completed`
- 注意：注入 "continue" 消息时持久化（`EventStore::append_message`）
- 注意：PreStop 的 steer 消息注入时持久化
- 注意：`AgentEvent::Lifecycle(Stopped)` 事件发送改为通过 `HookRegistry::run_loop_end` 触发

#### Task 3.8: 从 `Agent::handle_idle` 提取 `AgentLoop::run_turn` 的入口逻辑
- 提取 `handle_idle` 中的消息注入逻辑到 `AgentLoop::run_turn` 入口：
  - 构建 `Message::with_blocks(Role::User, content)`
  - 应用 `message_interceptor`（通过 `PreTurn` hook 实现）
  - 追加到 history
  - 持久化
- 注意：generation fencing 逻辑在 `Agent` 包装层处理，`AgentLoop` 不感知

#### Task 3.9: 从 `Agent::start_turn_if_needed` 提取 `TurnTracker::start_turn`
- 将 `start_turn_if_needed` 逻辑迁移到 `TurnTracker::start_turn`
- 在 `AgentLoop::run_turn` 开始时调用 `TurnTracker::start_turn`

#### Task 3.10: 实现 `AgentLoop::persist_message` 和 `persist_all`
- 实现内部辅助方法 `persist_message`（调用 `EventStore::append_message`）
- 实现内部辅助方法 `persist_all`（调用 `EventStore::replace_messages`）
- 当 `EventStore` 为 `None` 时跳过

#### Task 3.11: 实现 `AgentLoop::history` 访问器
- 提供 `history(&self) -> &dyn History` 方法（用于 `SubagentTool` 收集结果）
- 或提供 `into_history(self) -> Box<dyn History>` 方法（`SubagentTool` 消费后读取）

#### Task 3.12: 编译验证 AgentLoop
- 确保 `AgentLoop` 模块编译通过（不依赖 `Agent` 的具体实现）
- 写单元测试：测试 `AgentLoop` 的基本结构和方法签名

---

### Phase 4: 重构 Hook 系统

#### Task 4.1: 重构 `HookRegistry` 为支持 Listener 的接口
- 修改 `hooks/registry.rs` 或创建新的 `agent/loop/hook_registry.rs`
- 实现 `DefaultHookRegistry`：
  - 维护 `Vec<Arc<dyn HookHandler>>`（现有 mutating hooks）
  - 维护 `Vec<Box<dyn HookListener>>`（新增 observation hooks）
  - 每个 `run_*` 方法先调用 `HookHandler`（修改 payload），再调用所有 `HookListener`（通知）
- 保留现有 `HookRegistry::register` 方法（注册 `HookHandler`）
- 新增 `add_listener` 方法（注册 `HookListener`）

#### Task 4.2: 实现 `EventPersistListener`
- 创建 `hooks/listeners/event_persist.rs`
- 实现 `HookListener` trait
- 监听 `PostModel` 和 `PostTool` 事件，调用 `EventStore::append_message`
- 注意：这是追加式持久化，不需要处理 `PostCompact`（compaction 的 `replace` 由 `AgentLoop` 直接调用）

#### Task 4.3: 实现 `TuiEventListener`
- 创建 `hooks/listeners/tui_event.rs`
- 实现 `HookListener` trait
- 监听所有事件，转换为 `Event` enum 并通过 `EventBusHandle` 发送
- 需要映射表：
  - `LoopStart` → `AgentEvent::Lifecycle(Running)`
  - `PostModel` → `ModelEvent::Chunk` / `TokenUsage` / `Completed` / `End` / `Fallback` / `ToolCallDelta`
  - `PostTool` → `ToolEvent::Start` / `End`
  - `OnError` → `AgentEvent::Error`
  - `LoopEnd` → `AgentEvent::Lifecycle(Stopped)` / `SystemEvent::Shutdown`
- 注意：需要存储 `agent_id` 和 `session_id` 用于事件构造

#### Task 4.4: 实现 `UsageRecordListener`
- 创建 `hooks/listeners/usage_record.rs`
- 实现 `HookListener` trait
- 监听 `PostModel` 事件，提取 `token_usage`，调用 `UsageRecorder::record`
- 监听 `PostCompact` 事件，提取 compaction token usage，调用 `UsageRecorder::record`（`UsageType::Compactor`）

#### Task 4.5: 重构 `Agent::emit_user_message_event` 为 Hook
- 删除 `Agent::emit_user_message_event` 方法
- 在 `AgentLoop::run_turn` 的 `PreTurn` 阶段后，通过 `HookRegistry` 发送 `UserEvent::Message`
- `TuiEventListener` 监听 `PreTurn` 或 `PostTurn` 事件，发送 `UserEvent::Message`

#### Task 4.6: 重构 `Agent::emit_error` 为 `OnError` Hook
- 删除 `Agent::emit_error` 方法
- 在 `AgentLoop` 的错误处理点调用 `HookRegistry::run_on_error`
- `TuiEventListener` 监听 `OnError` 事件，发送 `AgentEvent::Error`

#### Task 4.7: 重构 `Agent::emit_retrying` 为 `OnError` Hook
- 删除 `Agent::emit_retrying` 方法
- 在 `AgentLoop::stream_with_retry` 的重试点调用 `HookRegistry::run_on_error`（`is_recoverable: true`）
- `TuiEventListener` 监听 `OnError` 事件，发送 `AgentEvent::Retrying`

#### Task 4.8: 重构 `Agent::emit_operation_cancelled` 为 `OnError` Hook
- 删除 `Agent::emit_operation_cancelled` 方法
- 在 `AgentLoop` 的取消处理点调用 `HookRegistry::run_on_error`
- `TuiEventListener` 监听 `OnError` 事件，发送 `AgentEvent::Lifecycle(Stopped(Cancelled))`

#### Task 4.9: 重构 `Agent::emit_stopped_completed` 为 `LoopEnd` Hook
- 删除 `Agent::emit_stopped_completed` 方法
- 在 `AgentLoop::run_turn` 完成时调用 `HookRegistry::run_loop_end`
- `TuiEventListener` 监听 `LoopEnd` 事件，发送 `AgentEvent::Lifecycle(Stopped(Completed))`

#### Task 4.10: 重构 `Agent::emit_compaction_event` 为 `PreCompact`/`PostCompact` Hook
- 删除 `Agent::emit_compaction_event` 方法
- 在 `AgentLoop::check_and_compact` 的开始和结束调用 `HookRegistry::run_pre_compact` / `run_post_compact`
- `TuiEventListener` 监听 `PreCompact`/`PostCompact` 事件，发送 `ModelEvent::Compacting`

#### Task 4.11: 重构 `Agent::fail_agent` 为 `OnError` + `LoopEnd` Hook
- 删除 `Agent::fail_agent` 方法
- 在致命错误点调用 `HookRegistry::run_on_error`（`is_recoverable: false`）+ `run_loop_end`
- `TuiEventListener` 监听这两个事件，发送 `AgentEvent::Error` + `AgentEvent::Lifecycle(Stopped(Failed))`

#### Task 4.12: 保留现有 `HookHandler` 兼容性
- 确保 `CommandHookHandler`, `SkillHookHandler`, `GoalPreStopHandler` 仍然可以注册到 `DefaultHookRegistry`
- 保留 `run_pre_tool_hooks` 和 `run_post_tool_hooks` 的辅助函数（改为调用 `HookRegistry::run_pre_tool` / `run_post_tool`）
- 保留 `HookContext`, `HookResult`, `PreToolDecision`, `PostToolDecision`, `PreStopDecision` 的序列化兼容性

---

### Phase 5: 重构 Agent 包装层

#### Task 5.1: 重构 `Agent` struct 为 `AgentLoop` 包装
- 修改 `Agent` struct：
  - 删除 `message_buffer`（由 `AgentLoop` 内部持有）
  - 删除 `tool_registry`（由 `AgentLoop` 内部持有）
  - 删除 `hook_registry`（由 `AgentLoop` 内部持有）
  - 删除 `current_turn`（由 `AgentLoop` 的 `TurnTracker` 管理）
  - 删除 `max_tool_output_length`（由 `AgentLoop` 的 `LoopConfig` 持有）
  - 删除 `skills`（由 `AgentLoop` 的 `LoopConfig` 持有）
  - 保留 `input_rx`, `steer_rx`, `context`, `cancel_token`, `input_stale_since`
  - 新增 `loop_engine: AgentLoop`
  - 新增 `mailbox: Box<dyn Mailbox>`

#### Task 5.2: 重构 `Agent::spawn` 为构建 `LoopConfig` + 创建 `AgentLoop`
- 在 `spawn` 中：
  1. 构建 `system_prompt`（复用 `SystemPromptBuilder`）
  2. 加载 `history`（从 `MessageStore` 或 `args.history`）
  3. 创建 `MemoryHistory` 并初始化
  4. 构建 `ToolRegistry`（复用 `ToolRegistryFactory`）
  5. 构建 `HookRegistry`（复用 `build_hook_registry_with_skills`）
  6. 创建 `DefaultHookRegistry` 并添加 listener：
     - `EventPersistListener`（绑定 `EventStore`）
     - `TuiEventListener`（绑定 `EventBusHandle`）
     - `UsageRecordListener`（绑定 `UsageStore`）
  7. 创建 `LoopConfig`（填充所有端口）
  8. 创建 `AgentLoop`
  9. 创建 `Agent`（包装层）
  10. Spawn 任务
  11. 返回 `AgentHandle`

#### Task 5.3: 重构 `Agent::start_loop` 为简化状态机
- 简化状态机：只保留 `Idle` → `Closed` 两个状态
- `Idle` 时：
  - 从 `Mailbox` 拉取消息
  - 处理 generation fencing（stale input 丢弃）
  - 处理 `AgentInput::Shutdown` → `Closed`
  - 处理 `AgentInput::Compact` → 调用 `AgentLoop` 的 compaction（或直接在 `AgentLoop` 中处理）
  - 处理 `AgentInput::Rewind` → 调用 `History::truncate_at` + 持久化
  - 处理 `AgentInput::Clear` → 调用 `History::clear` + 持久化
  - 处理 `AgentInput::Continue` → 构造 "continue" 消息，调用 `AgentLoop::run_turn`
  - 处理 `AgentInput::User` → 构造消息，调用 `AgentLoop::run_turn`
- 所有 `Streaming` / `ExecutingTool` / `Compacting` 状态在 `AgentLoop` 内部处理
- `Agent` 包装层只等待 `AgentLoop::run_turn` 返回，然后回到 `Idle`

#### Task 5.4: 重构 `AgentInput` 处理逻辑
- 保留 `AgentInput` enum（但考虑用 `Mail` 替代）
- 在 `Agent::start_loop` 中将 `AgentInput` 映射为 `Mail` 或直接调用 `AgentLoop`
- 处理 `AgentInput::Continue`：构造 `Message::user("continue")`，直接调用 `AgentLoop::run_turn`
- 处理 `AgentInput::Compact`：直接调用 `AgentLoop::check_and_compact`（需要暴露方法）
- 处理 `AgentInput::Rewind`：调用 `History::truncate_at`，然后 `EventStore::replace_messages`
- 处理 `AgentInput::Clear`：调用 `History::clear`，然后 `EventStore::replace_messages`，清空 file state / todo
- 处理 `AgentInput::Shutdown`：
  - 调用 `TurnTracker::cancel_turn`
  - 调用 `HookRegistry::run_loop_end`
  - 状态设为 `Closed`
- 处理 `AgentInput::TaskResult`：构造消息，调用 `AgentLoop::run_turn`

#### Task 5.5: 重构 `Agent::handle_clear` 为使用 `History` + `EventStore`
- 删除 `handle_clear` 方法，逻辑内联到 `AgentInput::Clear` 处理
- 使用 `History::clear` + `History::append(system_msg)`
- 使用 `EventStore::replace_messages`
- 清空 `file_state_store` 和 `todo_storage`（通过 `PostClear` hook 或直接在 `Agent` 中处理）

#### Task 5.6: 重构 `Agent::process_rewind` 为使用 `History` + `EventStore`
- 删除 `process_rewind` 方法，逻辑内联到 `AgentInput::Rewind` 处理
- 使用 `History::truncate_at`
- 调用 `Turn::rewind_to_checkpoint`
- 使用 `EventStore::replace_messages`
- 发送 `SystemEvent::Rewound`（通过 `HookRegistry` 的 `OnError` 或自定义 hook）

#### Task 5.7: 保留 `AgentHandle` 接口不变
- `AgentHandle` 的公共 API 完全不变：
  - `send_message`, `send_text`, `send_permission_response`, `send_ask_user_response`
  - `state`, `is_compacting`, `wait_for_state_change`
  - `cancel`, `close`, `force_compact`, `rewind`, `send_steer`, `send_continue`, `clear`
- 内部实现改为通过 `Mailbox` 或 `AgentLoop` 的暴露方法
- 注意：`is_compacting` 可能需要改为查询 `AgentLoop` 状态或总是返回 false（因为 compaction 在 `AgentLoop` 内部同步执行）

#### Task 5.8: 处理 `AgentInput::Compact` 和 `AgentInput::Continue` 的 compaction 语义
- `AgentInput::Compact`（force compact）：需要 `AgentLoop` 暴露 `force_compact` 方法
- 在 `Agent` 包装层中直接调用 `AgentLoop::check_and_compact`（强制 compaction）
- 或者：将 `Compact` 作为特殊 `Mail` 处理，`AgentLoop::run_turn` 检测到 compaction 消息时先执行 compaction

---

### Phase 6: 重构 SubagentTool

#### Task 6.1: 删除 `simple.rs`
- 删除 `crates/kernel/src/agent/simple.rs`
- 从 `agent/mod.rs` 中移除 `simple` 模块的导出

#### Task 6.2: 重构 `SubagentTool` 为使用 `AgentLoop`
- 修改 `tools/subagent.rs` 中的 `SubagentTool::exec`
- 不再创建 `SimpleAgent`
- 构建 `AgentLoop` 的 `LoopConfig`：
  - `system_prompt`：复用 `build_system_prompt`（从 `build_subagent_prompt` 迁移）
  - `history`：从 `inherit_context` 选项构建 `MemoryHistory`
  - `model`：复用 `ProviderModelClient`（使用 parent 的 provider）
  - `model_config`：复用 parent 的 `model_config`
  - `tools`：通过 `ToolRegistryFactory::for_subagent` 创建
  - `compactor`：复用 parent 的 compactor（或 `None`）
  - `hooks`：创建精简的 `DefaultHookRegistry`（不含 TUI 事件 listener，可选含 EventPersistListener）
  - `turn_tracker`：`NoOpTurnTracker`（子 agent 不需要 checkpoint）
  - `event_store`：`NoOpEventStore`（默认不持久化）
  - `usage_recorder`：`NoOpUsageRecorder`（默认不记录）
  - `max_steps`：复用 `max_iterations`
  - `max_tool_output_length`：复用 parent 的 `max_tool_output_length`
  - `permission_checker`：`None`（子 agent 默认不检查权限）或复用 parent 的
- 直接调用 `AgentLoop::run_turn(user_msg)`
- 从 `AgentLoop` 的 `history` 读取结果，格式化输出

#### Task 6.3: 实现 `SubagentTool` 的事件收集
- 在 `SubagentTool` 的 `LoopConfig` 中注册一个自定义 `HookListener`：
  - 监听 `PostModel` 事件，收集 assistant 的回复文本
  - 监听 `PostTool` 事件，收集工具执行结果
  - 监听 `OnError` 事件，收集错误信息
- 或者：直接从 `AgentLoop::history()` 读取最后几条消息来构建输出
- 参考 `SimpleAgent::build_result` 的格式化逻辑，构建 `ToolOutput::text(...)`

#### Task 6.4: 处理 `SubagentTool` 的 session 创建
- 保留 `SubagentTool` 通过 `session_store` 创建子 session 的逻辑
- 子 session 的 `session_id` 用于 `AgentLoop::LoopConfig` 的 `session_id`
- 子 session 的 `EventStore` 可以选择使用 `JsonlEventStore`（写入子 session 的 JSONL 文件）

#### Task 6.5: 处理 `SubagentTool` 的 `on_event` 回调（进度上报）
- 如果需要保持进度上报给 parent 的 TUI，需要在 `SubagentTool` 的 `HookRegistry` 中添加 listener：
  - 监听 `PostModel` 的 chunk 事件，通过 `ctx` 的 `event_bus` 发送 `ToolEvent::Progress`
- 或者：从 `AgentLoop` 的 `history` 中读取增量结果，在 `run_turn` 返回后一次性上报
- 注意：如果不需要实时进度，可以删除 `on_event` 回调

#### Task 6.6: 验证 `SubagentTool` 的并行工具执行
- `AgentLoop::execute_tools` 使用 `execute_tools_parallel`，子 agent 自动获得并行能力
- 对比 `SimpleAgent` 的串行执行，确认性能提升

#### Task 6.7: 验证 `SubagentTool` 的 hooks 生效
- 子 agent 的 `DefaultHookRegistry` 包含 `HookHandler`（如 skill-level hooks）
- 确认 `PreToolUse` / `PostToolUse` 在子 agent 中生效

---

### Phase 7: 清理和删除旧代码

#### Task 7.1: 删除 `MessageBuffer`
- 删除 `agent/message_buffer.rs`（或保留为 `MemoryHistory` 的内部实现）
- 将所有 `MessageBuffer` 引用替换为 `Box<dyn History>` 或 `MemoryHistory`
- 迁移 `MessageBuffer::sanitize` 到 `MemoryHistory::sanitize`
- 迁移 `MessageBuffer::from_arc_messages` 到 `MemoryHistory::from_arc_messages`

#### Task 7.2: 删除 `AgentInput`（可选）
- 如果 `Agent` 完全使用 `Mailbox` + `Mail`，可以删除 `AgentInput`
- 但 `AgentHandle` 的 API 使用 `AgentInput`，可能需要保留作为内部类型
- 或者将 `AgentInput` 改名为 `Mail` 或 `AgentCommand`

#### Task 7.3: 删除 `AgentShared` 中冗余的字段
- 分析 `AgentShared` 的每个字段，确认哪些已经通过 `LoopConfig` 注入：
  - `provider` → 通过 `ModelClient`
  - `compactor` → 通过 `Compactor`
  - `message_store` → 通过 `EventStore`
  - `usage_store` → 通过 `UsageRecorder`
  - `checkpoint_store` → 通过 `TurnTracker`
  - `event_bus` → 通过 `TuiEventListener`（间接）
- 保留 `AgentShared` 作为资源池，但 `Agent::spawn` 构建 `LoopConfig` 时选择性注入
- 或者：将 `AgentShared` 拆分为更小的资源组

#### Task 7.4: 删除 `Agent` 中已迁移的方法
- 删除 `Agent::handle_streaming`
- 删除 `Agent::handle_streaming_with_retry`
- 删除 `Agent::handle_execute_tool`
- 删除 `Agent::transition_after_streaming`
- 删除 `Agent::collect_stream_output`
- 删除 `Agent::maybe_compact_messages`
- 删除 `Agent::force_compact` / `force_full_compact`（或保留作为代理到 `AgentLoop`）
- 删除 `Agent::handle_compaction_result`
- 删除 `Agent::emit_user_message_event`
- 删除 `Agent::emit_error`
- 删除 `Agent::emit_retrying`
- 删除 `Agent::emit_operation_cancelled`
- 删除 `Agent::emit_stopped_completed`
- 删除 `Agent::emit_compaction_event`
- 删除 `Agent::fail_agent`
- 删除 `Agent::handle_clear`（逻辑内联）
- 删除 `Agent::process_rewind`（逻辑内联）
- 删除 `Agent::inject_user_message`（逻辑内联到 `AgentLoop::run_turn`）
- 删除 `Agent::truncate_at`（迁移到 `MemoryHistory`）
- 删除 `Agent::start_turn_if_needed`（迁移到 `TurnTracker`）
- 删除 `Agent::complete_turn_if_needed`（迁移到 `AgentLoop`）
- 删除 `Agent::persist_message`（迁移到 `AgentLoop`）
- 删除 `Agent::record_compactor_token_usage`（迁移到 `UsageRecordListener`）
- 删除 `Agent::apply_compacted_messages`（迁移到 `AgentLoop`）
- 删除 `Agent::extract_summary`（迁移为公共函数）
- 保留 `Agent::handle_cancel`（但简化）
- 保留 `Agent::create_runtime_token`（但简化）

#### Task 7.5: 删除 `AgentState` 中的冗余状态
- 保留 `Idle` 和 `Closed`
- 删除 `Streaming`？或者保留作为 `Agent` 包装层的观察状态？
- 删除 `ExecutingTool`？
- 删除 `Compacting`？
- 决策：如果 TUI 需要观察 streaming 状态，可以保留 `Streaming` 作为 `Agent` 包装层的状态，但 `AgentLoop` 执行期间 `Agent` 设为 `Streaming`，完成后回到 `Idle`
- 建议保留 `Streaming` 用于 TUI 观察，删除 `ExecutingTool` 和 `Compacting`（它们都在 `AgentLoop` 内部）

#### Task 7.6: 删除 `StreamingHandler` 或重构为 `StreamCollector`
- `StreamingHandler` 中的 `event_tx` 硬编码发送需要改为通过 `HookRegistry`
- 将 `StreamingHandler` 的 collect 逻辑迁移到 `AgentLoop::do_stream` 中的 `StreamCollector`
- 保留 `StreamingHandler` 的 `start_stream` 和 `build_message` 方法（作为辅助函数）
- 或完全删除 `StreamingHandler`，所有逻辑在 `AgentLoop` 中内联

---

### Phase 8: 编译和测试

#### Task 8.1: 确保所有模块编译通过
- 逐步编译 `kernel` crate
- 修复所有类型不匹配和 trait 未实现错误
- 确保 `cli` 和 `tui` crate 的依赖不变（它们只通过 `AgentHandle` 和 `EventBus` 与 `Agent` 交互）

#### Task 8.2: 运行 `cargo clippy`
- 修复所有 clippy 警告
- 特别注意 `async_trait` 的使用和 `dyn` 对象的性能影响

#### Task 8.3: 运行 `cargo fmt`
- 格式化所有修改的文件

#### Task 8.4: 运行单元测试
- 运行 `cargo test -p kernel --lib`
- 修复所有失败的测试
- 特别关注：
  - `tools::read` 的测试（使用 `ToolExecCtx`）
  - `utils::search` 的测试（使用 `ToolExecCtx`）
  - `hooks` 的测试
  - `compactor` 的测试
  - `storage` 的测试

#### Task 8.5: 运行集成测试
- 运行 `cargo test -p kernel`
- 如果有集成测试，验证它们通过

#### Task 8.6: 验证 `AgentHandle` 的 API 兼容性
- 确认 `Session`, `Coordinator`, `KernelServer` 对 `AgentHandle` 的使用不变
- 确认 TUI 通过 `EventBus` 接收事件的行为不变
- 确认 `SubagentTool` 的行为和输出格式不变

#### Task 8.7: 手动测试关键场景
- 主 agent 发送消息 → streaming → 工具执行 → 完成
- 主 agent 的 compaction 触发
- 主 agent 的 cancel 操作
- 主 agent 的 rewind 操作
- 主 agent 的 clear 操作
- 子 agent（`SubagentTool`）执行复杂任务
- 子 agent 的并行工具执行（多 grep）
- 子 agent 的 hooks 生效
- 权限检查流程
- 错误恢复和重试

---

### Phase 9: 文档和代码审查

#### Task 9.1: 更新架构文档
- 更新 `docs/design/` 中的架构文档
- 描述新的 `AgentLoop` + `Agent` 包装层架构
- 描述端口 trait 的职责和关系
- 描述主/子 agent 的一致性

#### Task 9.2: 添加模块级文档注释
- 为 `agent/loop/` 下的所有模块添加 `//!` 文档注释
- 为每个 trait 添加使用说明
- 为 `AgentLoop` 的核心方法添加详细文档

#### Task 9.3: 代码审查清单
- 检查所有 `unwrap` 和 `expect`，确保有合理的错误处理
- 检查所有 `async_trait` 的使用，避免不必要的 `Box` 分配
- 检查所有 `dyn` 对象的性能影响（`History`, `Mailbox`, `EventStore`, `HookRegistry`, `TurnTracker`, `Compactor`, `PermissionChecker`, `UsageRecorder`）
- 考虑是否某些 trait 可以用泛型替代 `dyn`（如 `History` 在 `AgentLoop` 中是否可以用泛型参数）
- 检查 `AgentLoop` 的 `LoopConfig` 构建是否过于复杂（考虑使用 builder pattern）
- 检查 `SubagentTool` 的 `LoopConfig` 构建是否重复了 `Agent::spawn` 的逻辑（考虑提取为 `LoopConfigBuilder`）

---

## 九、风险与缓解

| 风险 | 影响 | 缓解 |
|------|------|------|
| 重构范围极大（~1500 行 `Agent` 代码） | 高 | 分 Phase 执行，每个 Phase 都有编译检查点 |
| 事件流变化可能影响 TUI | 高 | `TuiEventListener` 精确映射所有事件，`AgentHandle` API 不变 |
| `dyn` 对象性能开销 | 中 | 大部分 `dyn` 在 `AgentLoop` 初始化时创建，运行时调用频率低；`History` 和 `HookRegistry` 调用最频繁，可考虑泛型优化 |
| `SubagentTool` 输出格式变化 | 中 | 保留 `SimpleAgent::build_result` 的格式化逻辑，只是数据来源改为 `AgentLoop::history` |
| 崩溃恢复/rewind 逻辑变化 | 高 | `TurnTracker` 保留 `CheckpointTurnTracker` 实现，`AgentLoop` 的 `run_turn` 中正确调用 `complete_turn` / `cancel_turn` |
| Compaction 的 file state clear 逻辑 | 中 | 通过 `PostCompact` hook 的 listener 实现，或 `AgentLoop` 内部直接调用 |
| 权限检查在子 agent 中的行为 | 低 | 子 agent 的 `PermissionChecker` 可配置为 `None`（自动通过）或复用 parent 的 |
| `AgentState::Streaming` 作为 TUI 观察状态 | 低 | 保留 `Agent` 包装层在 `AgentLoop::run_turn` 前后设置 `Streaming` / `Idle` 状态 |

---

## 十、依赖关系图（任务执行顺序）

```
Phase 1: 定义 Trait
  ├─ Task 1.1 (ports.rs)
  ├─ Task 1.2 (hook_payloads.rs)
  ├─ Task 1.3 (HookRegistry 扩展)
  ├─ Task 1.4 (loop/mod.rs)
  └─ Task 1.5 (error.rs)
       │
       ▼
Phase 2: 适配器实现
  ├─ Task 2.1 (MemoryHistory)
  ├─ Task 2.3 (ChannelMailbox)
  ├─ Task 2.4 (DirectMailbox)
  ├─ Task 2.5 (JsonlEventStore)
  ├─ Task 2.6 (NoOpEventStore)
  ├─ Task 2.7 (CheckpointTurnTracker)
  ├─ Task 2.8 (NoOpTurnTracker)
  ├─ Task 2.9 (ProviderModelClient)
  ├─ Task 2.10 (ConfigPermissionChecker)
  ├─ Task 2.11 (SqliteUsageRecorder)
  └─ Task 2.12 (NoOpUsageRecorder)
       │
       ▼
Phase 3: AgentLoop 提取
  ├─ Task 3.1 (AgentLoop struct)
  ├─ Task 3.2 (do_stream)
  ├─ Task 3.3 (StreamCollector)
  ├─ Task 3.4 (stream_with_retry)
  ├─ Task 3.5 (execute_tools)
  ├─ Task 3.6 (check_and_compact)
  ├─ Task 3.7 (run_turn 尾部)
  ├─ Task 3.8 (run_turn 入口)
  ├─ Task 3.9 (TurnTracker::start_turn)
  ├─ Task 3.10 (persist helpers)
  ├─ Task 3.11 (history accessor)
  └─ Task 3.12 (编译验证)
       │
       ▼
Phase 4: Hook 系统重构
  ├─ Task 4.1 (DefaultHookRegistry)
  ├─ Task 4.2 (EventPersistListener)
  ├─ Task 4.3 (TuiEventListener)
  ├─ Task 4.4 (UsageRecordListener)
  ├─ Task 4.5-4.11 (事件发送重构)
  └─ Task 4.12 (兼容性保留)
       │
       ▼
Phase 5: Agent 包装层重构
  ├─ Task 5.1 (Agent struct)
  ├─ Task 5.2 (Agent::spawn)
  ├─ Task 5.3 (start_loop 简化)
  ├─ Task 5.4 (AgentInput 处理)
  ├─ Task 5.5 (handle_clear)
  ├─ Task 5.6 (process_rewind)
  ├─ Task 5.7 (AgentHandle 保留)
  └─ Task 5.8 (Compact/Continue)
       │
       ▼
Phase 6: SubagentTool 重构
  ├─ Task 6.1 (删除 simple.rs)
  ├─ Task 6.2 (SubagentTool 使用 AgentLoop)
  ├─ Task 6.3 (事件收集)
  ├─ Task 6.4 (session 创建)
  ├─ Task 6.5 (on_event 回调)
  ├─ Task 6.6 (并行工具验证)
  └─ Task 6.7 (hooks 验证)
       │
       ▼
Phase 7: 清理
  ├─ Task 7.1 (删除 MessageBuffer)
  ├─ Task 7.2 (删除 AgentInput)
  ├─ Task 7.3 (AgentShared 清理)
  ├─ Task 7.4 (删除旧方法)
  ├─ Task 7.5 (AgentState 简化)
  └─ Task 7.6 (StreamingHandler 清理)
       │
       ▼
Phase 8: 编译和测试
  ├─ Task 8.1 (编译通过)
  ├─ Task 8.2 (clippy)
  ├─ Task 8.3 (fmt)
  ├─ Task 8.4 (单元测试)
  ├─ Task 8.5 (集成测试)
  ├─ Task 8.6 (API 兼容性)
  └─ Task 8.7 (手动测试)
       │
       ▼
Phase 9: 文档
  ├─ Task 9.1 (架构文档)
  ├─ Task 9.2 (模块文档)
  └─ Task 9.3 (代码审查)
```

---

**总计任务数：10 + 12 + 12 + 12 + 8 + 7 + 6 + 7 + 3 = 77 个具体任务**
