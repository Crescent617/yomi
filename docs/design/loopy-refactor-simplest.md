# Yomi Agent 简化重构 — 最终方案

## 核心洞察

用户说得很对：我之前的方案在创建新架构（`AgentLoop` + 一堆 trait + 双层 build），而 loopy 的本质是**一个 `Loop` 直接跑，没有包装层**。Yomi 的 `Agent` 本身就是 loop，只需要让它能被主/子 agent 直接复用。

## 问题根因

`Agent` 的 turn 执行逻辑（streaming → tools → continue）被锁在 `start_loop` 的 `Streaming`/`ExecutingTool` 状态机里：`SubagentTool` 没法直接调用 `handle_streaming` + `handle_execute_tool` 的循环，所以另写了 `SimpleAgent`。

## 方案：Agent 直接复用，无新架构

### 改动

1. 从 `Agent` 移除 `input_rx` 和 `steer_rx`（run loop 的 channel 不属于 Agent 核心）
2. 提取 `Agent::create` — 纯构造，不 spawn
3. 提取 `Agent::execute_turn` — 一个完整 turn 的核心循环（streaming + tools + continue）
4. 提取 `Agent::run_loop` — 从 channel 读取并调用 `execute_turn`（主 agent 用）
5. `handle_streaming` 中的 `steer_rx` 读取改为 `execute_turn` 的参数注入
6. `SubagentTool` 直接 `Agent::create` + `execute_turn`
7. 删除 `SimpleAgent`

### 代码

```rust
impl Agent {
    /// 创建 Agent 实例（不 spawn），主/子 agent 共用
    pub fn create(id: AgentId, shared: &Arc<AgentShared>, args: AgentSpawnArgs) -> Self {
        // 构建所有字段，steer_rx 用空 channel
        let (_, steer_rx) = mpsc::channel(1);
        
        let event_bus = shared
            .event_bus
            .as_ref()
            .expect("event_bus must be configured")
            .handle(SessionId(args.session_id.clone()));
        
        let system_prompt = ...; // 同步构建或 block_on
        let mut messages = vec![Arc::new(Message::system(system_prompt))];
        messages.extend(args.history.into_iter().filter(|m| m.role != Role::System));
        let message_buffer = MessageBuffer::from_arc_messages(&messages);
        
        let tool_registry = ...;
        let hook_registry = ...;
        let (permission_checker, _) = ...;
        let checkpoint_store = ...;
        
        Self {
            id,
            shared: shared.clone(),
            message_buffer,
            event_bus,
            context: AgentExecutionContext::new(AgentState::Idle),
            cancel_token: args.cancel_token.clone().unwrap_or_default(),
            session_id: SessionId(args.session_id.clone()),
            max_iterations: args.max_iterations,
            tool_registry,
            permission_checker,
            working_dir: args.working_dir,
            input_stale_since: Arc::new(AtomicU64::new(0)),
            hook_registry,
            checkpoint_store,
            data_dir: shared.data_dir.clone(),
            current_turn: None,
            skills: args.skills.clone(),
            steer_rx, // 空 channel，execute_turn 不从这里读
            max_tool_output_length: args.max_tool_output_length,
        }
    }
    
    /// 执行一个完整 turn（streaming + tools + continue），核心方法
    /// steer: 在 user message 后注入的 steer 内容（由 run_loop 从 steer_rx 读取后传入）
    pub async fn execute_turn(&mut self, content: Vec<ContentBlock>, steer: Vec<ContentBlock>) -> Result<(), AgentError> {
        // 1. 注入用户消息
        self.inject_user_message(content).await?;
        
        // 2. 注入 steer 消息（从 run_loop 传入，不再从 self.steer_rx 读取）
        if !steer.is_empty() {
            let steer_msg = Message::with_blocks(Role::User, steer);
            self.emit_user_message_event(&steer_msg.id, &steer_msg.content);
            self.persist_message(&steer_msg).await;
            self.message_buffer.push(steer_msg);
        }
        
        // 3. 启动 turn 跟踪
        self.start_turn_if_needed().await;
        
        let mut iterations = 0;
        
        // 4. Streaming → Tool 循环
        loop {
            if iterations >= self.max_iterations {
                return Ok(());
            }
            iterations += 1;
            
            if self.cancel_token.is_cancelled() {
                return Err(AgentError::Cancelled("turn".into()));
            }
            
            // 4a. Compaction
            self.maybe_compact_messages().await;
            
            // 4b. Streaming
            self.handle_streaming_with_retry().await?;
            
            // 4c. handle_streaming 已调用 transition_after_streaming，状态现在是 Idle 或 ExecutingTool
            match self.context.current_state() {
                AgentState::ExecutingTool => {
                    self.handle_execute_tool().await?;
                    // handle_execute_tool 内部会 transition_to(Streaming) 或 (Idle)
                }
                AgentState::Idle => {
                    // Turn 完成
                    if let Some(turn) = self.current_turn.take() {
                        turn.complete().await.ok();
                    }
                    break;
                }
                _ => unreachable!(),
            }
        }
        
        Ok(())
    }
    
    /// 主 loop（从 channel 读取），只有主 agent 用
    pub async fn run_loop(mut self, mut input_rx: mpsc::Receiver<AgentInput>, mut steer_rx: mpsc::Receiver<Vec<ContentBlock>>) {
        loop {
            tokio::select! {
                biased;
                Some(input) = input_rx.recv() => {
                    match input {
                        AgentInput::User { content, generation } => {
                            let current = self.input_stale_since.load(Ordering::Relaxed);
                            if generation < current { continue; }
                            
                            // 读取 steer_rx 积压
                            let mut steer = Vec::new();
                            while let Ok(blocks) = steer_rx.try_recv() {
                                steer.extend(blocks);
                            }
                            
                            self.context.transition_to(AgentState::Streaming);
                            if let Err(e) = self.execute_turn(content, steer).await {
                                if e.is_cancelled() {
                                    self.handle_cancel("turn").await;
                                } else {
                                    self.emit_error(ErrorPhase::Streaming, &e.to_string(), false).await;
                                }
                            }
                            self.context.transition_to(AgentState::Idle);
                        }
                        AgentInput::Continue => {
                            self.context.transition_to(AgentState::Streaming);
                            let _ = self.execute_turn(vec![ContentBlock::Text { text: "continue".to_string() }], vec![]).await;
                            self.context.transition_to(AgentState::Idle);
                        }
                        AgentInput::TaskResult { content, .. } => {
                            self.context.transition_to(AgentState::Streaming);
                            let _ = self.execute_turn(content, vec![]).await;
                            self.context.transition_to(AgentState::Idle);
                        }
                        AgentInput::Shutdown => {
                            if let Some(turn) = self.current_turn.take() {
                                turn.cancel().await.ok();
                            }
                            self.context.transition_to(AgentState::Closed);
                            break;
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
                        _ => {}
                    }
                }
                Some(steer) = steer_rx.recv() => {
                    // Steer 作为独立 user 消息注入（当 Idle 时收到 steer）
                    self.context.transition_to(AgentState::Streaming);
                    let _ = self.execute_turn(steer, vec![]).await;
                    self.context.transition_to(AgentState::Idle);
                }
                else => break,
            }
        }
    }
    
    /// 原有 spawn：create + run_loop
    pub async fn spawn(id: AgentId, shared: &Arc<AgentShared>, args: AgentSpawnArgs) -> AgentHandle {
        let (input_tx, input_rx) = mpsc::channel(20);
        let (steer_tx, steer_rx) = mpsc::channel(20);
        let (context, state_rx) = AgentExecutionContext::new(AgentState::Idle);
        let cancel_token = args.cancel_token.clone().unwrap_or_default();
        let input_stale_since = Arc::new(AtomicU64::new(0));
        
        let mut agent = Self::create(id.clone(), shared, args).await;
        
        // 覆盖空 steer_rx 和 context
        agent.steer_rx = steer_rx;
        agent.context = context;
        agent.input_stale_since = input_stale_since.clone();
        
        tokio::spawn(async move {
            agent.run_loop(input_rx, steer_rx).await;
        });
        
        AgentHandle::new(id, input_tx, state_rx, cancel_token, input_stale_since, steer_tx)
    }
}
```

### handle_streaming 修改

从 `handle_streaming` 中移除 `steer_rx` 读取（因为 steer 已经在 `execute_turn` 开头注入）：

```rust
async fn handle_streaming(&mut self) -> Result<(), AgentError> {
    // ... 移除 steer_rx 读取代码
    // 原来的：
    // let mut steer_blocks = Vec::new();
    // while let Ok(blocks) = self.steer_rx.try_recv() { ... }
    // 现在由 execute_turn 的 steer 参数处理
    
    // 其余逻辑不变
    self.maybe_compact_messages().await;
    let tools = self.tool_registry.definitions();
    // ...
}
```

### SubagentTool 修改

```rust
impl Tool for SubagentTool {
    async fn exec(&self, args: Value, ctx: ToolExecCtx<'_>) -> Result<ToolOutput> {
        let task = parse_task(&args)?;
        let preset = parse_preset(&args)?;
        
        // 1. 创建子 session
        let subsession_id = self.session_store.create_subsession(&ctx.session_id).await?;
        
        // 2. 构建子 agent 的 spawn args（和主 agent 同一路径）
        let mut spawn_args = AgentSpawnArgs::new(
            build_subagent_prompt(&preset, &ctx),
            subsession_id.clone(),
        );
        
        if self.inherit_context {
            spawn_args = spawn_args.with_history(parent_history(&ctx));
        }
        
        spawn_args = spawn_args
            .with_max_iterations(self.max_iterations)
            .with_subagent(false) // 子 agent 默认禁用子 agent 工具
            .with_tool_blocklist(self.disallowed_tools.clone())
            .with_skills(self.skills.clone())
            .with_working_dir(ctx.working_dir.clone())
            .with_max_tool_output_length(self.max_tool_output_length)
            .with_allow_command_hooks(false);
        
        // 3. 直接创建 Agent 实例（不 spawn）
        let mut agent = Agent::create(
            AgentId::new(),
            &self.shared,
            spawn_args,
        ).await;
        
        // 4. 直接执行 turn
        agent.execute_turn(
            vec![ContentBlock::Text { text: task }],
            vec![], // 无 steer
        ).await.map_err(|e| anyhow!("Subagent failed: {}", e))?;
        
        // 5. 从 message_buffer 收集结果
        let result = self.format_result(agent.message_buffer.messages());
        
        Ok(ToolOutput::text(result))
    }
    
    fn format_result(&self, messages: &[Arc<Message>]) -> String {
        let mut result_text = String::new();
        for msg in messages.iter().skip(1) { // skip system prompt
            if msg.role == Role::Assistant {
                for block in &msg.content {
                    match block {
                        ContentBlock::Text { text } => result_text.push_str(text),
                        ContentBlock::Thinking { thinking } => result_text.push_str(thinking),
                        _ => {}
                    }
                }
            }
        }
        result_text
    }
}
```

### 删除 SimpleAgent

```bash
rm crates/kernel/src/agent/simple.rs
```

从 `agent/mod.rs` 移除 `pub mod simple;`

### Agent 字段变化

```diff
  pub struct Agent {
      id: AgentId,
      shared: Arc<AgentShared>,
      message_buffer: MessageBuffer,
      event_bus: EventBusHandle,
-     input_rx: mpsc::Receiver<AgentInput>,
      context: AgentExecutionContext,
      cancel_token: CancelToken,
      session_id: SessionId,
      max_iterations: usize,
      tool_registry: ToolRegistry,
      permission_checker: Option<Arc<Checker>>,
      working_dir: PathBuf,
      input_stale_since: Arc<AtomicU64>,
      hook_registry: HookRegistry,
      checkpoint_store: Arc<dyn CheckpointStore>,
      data_dir: PathBuf,
      current_turn: Option<Arc<Turn>>,
      skills: Vec<Arc<Skill>>,
-     steer_rx: mpsc::Receiver<Vec<ContentBlock>>,
      max_tool_output_length: usize,
  }
```

### 为什么不需要新架构

| 我的旧方案 | 现在方案 |
|-----------|---------|
| 创建 `AgentLoop` struct | 直接用 `Agent` |
| 创建 `History`/`Mailbox`/`TurnTracker`/`EventStore` trait | 不需要，`Agent` 字段就是状态 |
| 创建 `HookRegistry` 新接口 + 10+ payload 类型 | 保留现有 `HookRegistry`，事件通过 `event_bus` 发送 |
| 创建 `TuiEventListener`/`EventPersistListener`/`UsageRecordListener` | 不需要，事件发送还在 `Agent` 内部 |
| 两层：Agent 包装 AgentLoop | 一层：Agent 自己就是 loop |
| 子 agent 创建 LoopConfig + 构建 10+ 端口 | 子 agent 直接 `Agent::create` + `execute_turn` |

Loopy 的 `Loop` 就是一个 struct，所有依赖直接是字段。Yomi 的 `Agent` 也是。不需要再抽象。

### 子 agent 获得的能力

- ✅ 并行工具执行（`execute_tools_parallel`）
- ✅ Pre/Post Tool hooks（`HookRegistry`）
- ✅ Compaction（`Compactor`）
- ✅ 权限检查（如果配置了 `Checker`）
- ✅ Token usage 记录（`UsageStore`）
- ✅ 所有事件发送（`EventBus`）——发送到子 session，不影响父 agent

### 风险

1. **子 agent 的事件发送**：子 agent 的 `event_bus` 绑定到子 session，事件发送到子 session 的 bus。如果没人订阅，事件被丢弃。不影响功能。
2. **子 agent 的 checkpoint**：`Agent::create` 默认创建 `FilesystemCheckpointStore`，子 agent 的 checkpoint 写入 `data_dir/checkpoints/{subsession_id}/`。不影响父 agent。
3. **子 agent 的 `message_store` 持久化**：如果 `shared.message_store` 存在，子 agent 的消息会被持久化到子 session 的 JSONL 文件。这是可选功能，不影响。

### 执行步骤（8 个任务）

| # | 任务 | 文件 | 改动 |
|---|------|------|------|
| 1 | 提取 `Agent::create` | `agent/agent.rs` | 从 `spawn` 提取构造逻辑，不创建 channel |
| 2 | 移除 `input_rx`/`steer_rx` 字段 | `agent/agent.rs` | 从 struct 移除，改为 `run_loop` 参数 |
| 3 | 提取 `Agent::execute_turn` | `agent/agent.rs` | 从 `start_loop` 提取 Streaming+Tool 循环，接收 steer 参数 |
| 4 | 提取 `Agent::run_loop` | `agent/agent.rs` | 从 `start_loop` 提取 Idle 等待 + channel 读取 + `execute_turn` 调用 |
| 5 | 修改 `handle_streaming` | `agent/agent.rs` | 移除 `steer_rx` 读取逻辑 |
| 6 | 修改 `Agent::spawn` | `agent/agent.rs` | 调用 `create` + 覆盖 `context`/`steer_rx` + `run_loop` |
| 7 | 重构 `SubagentTool` | `tools/subagent.rs` | 使用 `Agent::create` + `execute_turn` + 从 `message_buffer` 收集结果 |
| 8 | 删除 `SimpleAgent` | `agent/simple.rs` | 删除文件，移除 `mod.rs` 导出 |

编译验证：`cargo build -p kernel` → `cargo clippy -p kernel -p cli -p tui` → `cargo test -p kernel --lib`
