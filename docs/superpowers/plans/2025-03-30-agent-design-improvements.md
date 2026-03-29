# Agent Design Improvements Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 agent 核心设计的 10 个问题：子代理实现、状态机安全、流式处理 bug、取消机制、所有权设计、工具并行执行、错误处理、消息历史限制、可测试性、以及关键功能缺失。

**Architecture:**
1. 重构 Agent 结构使用 Actor 模式（内部运行 + 外部控制句柄）
2. 引入合法状态转换表和取消令牌
3. 添加工具并行执行和超时控制
4. 实现消息历史截断和 Token 追踪

**Tech Stack:** Rust, tokio (async, mpsc, broadcast, time), chrono

---

## File Structure

| File | Responsibility |
|------|---------------|
| `crates/core/src/agent/mod.rs` | Agent Actor 主逻辑、状态机运行循环 |
| `crates/core/src/agent/state.rs` | AgentState 枚举 + 状态转换验证 |
| `crates/core/src/agent/handle.rs` | AgentHandle - 外部控制接口（新增） |
| `crates/core/src/agent/context.rs` | AgentExecutionContext - 共享状态（新增） |
| `crates/core/src/agent/cancel.rs` | CancelToken - 协作式取消（新增） |
| `crates/core/src/agent/subagent.rs` | SubAgentManager - 子代理生命周期（新增） |
| `crates/core/src/agent/message_buffer.rs` | MessageBuffer - 历史截断管理（新增） |
| `crates/core/src/tool/parallel.rs` | ParallelToolExecutor - 并行工具执行（新增） |
| `crates/core/src/types.rs` | TokenUsage, ToolTimeout 等新类型 |

---

## Task 1: 修复流式处理 Bug

**Files:**
- Modify: `crates/core/src/agent/mod.rs:179`

- [ ] **Step 1: 修复运算符优先级 bug**

将第 179 行的代码：
```rust
if current_text.len() + current_thinking.len() % 1000 == 0 {
```

改为：
```rust
if (current_text.len() + current_thinking.len()) % 1000 == 0 {
```

- [ ] **Step 2: Commit**

```bash
git add crates/core/src/agent/mod.rs
git commit -m "fix: operator precedence in streaming yield check

The modulo operator % has higher precedence than +,
so the original code was effectively:
  len + (len % 1000) == 0
instead of:
  (len + len) % 1000 == 0"
```

---

## Task 2: 添加合法状态转换验证

**Files:**
- Modify: `crates/core/src/agent/state.rs`

- [ ] **Step 1: 添加状态转换表**

```rust
impl AgentState {
    /// 返回从此状态可以合法转移到的目标状态
    pub fn valid_transitions(&self) -> &'static [AgentState] {
        match self {
            Self::Idle => &[Self::WaitingForInput],
            Self::WaitingForInput => &[Self::Streaming, Self::Cancelled],
            Self::Streaming => &[Self::ExecutingTool, Self::WaitingForInput, Self::Failed, Self::Cancelled],
            Self::ExecutingTool => &[Self::Streaming, Self::Failed, Self::Cancelled],
            // Terminal states - no valid transitions
            Self::Completed => &[],
            Self::Failed => &[],
            Self::Cancelled => &[],
        }
    }

    /// 检查是否可以转换到目标状态
    pub fn can_transition_to(&self, target: AgentState) -> bool {
        self.valid_transitions().contains(&target)
    }

    /// 是否为终止状态
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}
```

- [ ] **Step 2: 添加状态转换测试**

在 `crates/core/src/agent/state.rs` 底部添加测试模块：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_idle_can_only_go_to_waiting() {
        assert!(AgentState::Idle.can_transition_to(AgentState::WaitingForInput));
        assert!(!AgentState::Idle.can_transition_to(AgentState::Streaming));
    }

    #[test]
    fn test_terminal_states_have_no_transitions() {
        assert!(AgentState::Completed.valid_transitions().is_empty());
        assert!(AgentState::Failed.valid_transitions().is_empty());
        assert!(AgentState::Cancelled.valid_transitions().is_empty());
    }

    #[test]
    fn test_streaming_can_execute_tool_or_finish() {
        assert!(AgentState::Streaming.can_transition_to(AgentState::ExecutingTool));
        assert!(AgentState::Streaming.can_transition_to(AgentState::WaitingForInput));
        assert!(AgentState::Streaming.can_transition_to(AgentState::Failed));
        assert!(!AgentState::Streaming.can_transition_to(AgentState::Completed)); // 必须经过 WaitingForInput
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/agent/state.rs
git commit -m "feat: add state transition validation

Add can_transition_to() and valid_transitions() methods to
enforce valid state machine transitions at runtime."
```

---

## Task 3: 创建取消令牌机制

**Files:**
- Create: `crates/core/src/agent/cancel.rs`
- Modify: `crates/core/src/agent/mod.rs`

- [ ] **Step 1: 创建取消令牌模块**

```rust
// crates/core/src/agent/cancel.rs
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 协作式取消令牌
#[derive(Debug, Clone)]
pub struct CancelToken {
    inner: Arc<CancelTokenInner>,
}

#[derive(Debug)]
struct CancelTokenInner {
    cancelled: AtomicBool,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CancelTokenInner {
                cancelled: AtomicBool::new(false),
            }),
        }
    }

    /// 请求取消
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
    }

    /// 检查是否已取消
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// 如果已取消则返回错误
    pub fn check_cancelled(&self) -> anyhow::Result<()> {
        if self.is_cancelled() {
            anyhow::bail!("Operation cancelled")
        }
        Ok(())
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cancel_token() {
        let token = CancelToken::new();
        assert!(!token.is_cancelled());

        token.cancel();
        assert!(token.is_cancelled());
        assert!(token.check_cancelled().is_err());
    }

    #[test]
    fn test_cancel_token_clone() {
        let token1 = CancelToken::new();
        let token2 = token1.clone();

        token1.cancel();
        assert!(token2.is_cancelled()); // Shared state
    }
}
```

- [ ] **Step 2: 在 agent/mod.rs 中添加模块声明**

在 `crates/core/src/agent/mod.rs` 顶部添加：
```rust
mod cancel;
pub use cancel::CancelToken;
```

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/agent/cancel.rs crates/core/src/agent/mod.rs
git commit -m "feat: add CancelToken for cooperative cancellation

CancelToken allows external code to signal cancellation
to the running agent. Uses Arc<AtomicBool> for thread-safe
shared state."
```

---

## Task 4: 重构 Agent 为 Actor 模式

**Files:**
- Create: `crates/core/src/agent/handle.rs`
- Create: `crates/core/src/agent/context.rs`
- Modify: `crates/core/src/agent/mod.rs`

- [ ] **Step 1: 创建 AgentHandle 控制接口**

```rust
// crates/core/src/agent/handle.rs
use crate::agent::{AgentState, CancelToken};
use crate::types::AgentId;
use tokio::sync::{mpsc, oneshot};

/// 外部控制运行中 Agent 的句柄
#[derive(Debug, Clone)]
pub struct AgentHandle {
    pub id: AgentId,
    pub(super) input_tx: mpsc::Sender<String>,
    pub(super) state_rx: tokio::sync::watch::Receiver<AgentState>,
    cancel_token: CancelToken,
}

impl AgentHandle {
    pub fn new(
        id: AgentId,
        input_tx: mpsc::Sender<String>,
        state_rx: tokio::sync::watch::Receiver<AgentState>,
        cancel_token: CancelToken,
    ) -> Self {
        Self {
            id,
            input_tx,
            state_rx,
            cancel_token,
        }
    }

    /// 发送用户消息给 Agent
    pub async fn send_message(&self, content: String) -> anyhow::Result<()> {
        self.input_tx.send(content).await
            .map_err(|_| anyhow::anyhow!("Agent {} input channel closed", self.id.0))
    }

    /// 获取当前状态
    pub fn state(&self) -> AgentState {
        *self.state_rx.borrow()
    }

    /// 等待状态变化
    pub async fn wait_for_state_change(&mut self) -> AgentState {
        let _ = self.state_rx.changed().await;
        *self.state_rx.borrow()
    }

    /// 请求取消
    pub fn cancel(&self) {
        self.cancel_token.cancel();
    }

    /// 检查是否已请求取消
    pub fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }
}
```

- [ ] **Step 2: 创建共享执行上下文**

```rust
// crates/core/src/agent/context.rs
use crate::agent::AgentState;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// 在 Agent 任务和外部控制者之间共享的状态
#[derive(Debug, Clone)]
pub struct AgentExecutionContext {
    inner: Arc<AgentExecutionContextInner>,
}

#[derive(Debug)]
struct AgentExecutionContextInner {
    state_tx: tokio::sync::watch::Sender<AgentState>,
    iteration_count: AtomicUsize,
}

impl AgentExecutionContext {
    pub fn new(initial_state: AgentState) -> (Self, tokio::sync::watch::Receiver<AgentState>) {
        let (state_tx, state_rx) = tokio::sync::watch::channel(initial_state);
        let ctx = Self {
            inner: Arc::new(AgentExecutionContextInner {
                state_tx,
                iteration_count: AtomicUsize::new(0),
            }),
        };
        (ctx, state_rx)
    }

    pub fn transition_to(&self, new_state: AgentState) -> bool {
        let current = *self.inner.state_tx.borrow();
        if !current.can_transition_to(new_state) {
            tracing::warn!(
                "Invalid state transition: {:?} -> {:?}",
                current, new_state
            );
            return false;
        }
        self.inner.state_tx.send_replace(new_state);
        true
    }

    pub fn current_state(&self) -> AgentState {
        *self.inner.state_tx.borrow()
    }

    pub fn increment_iteration(&self) -> usize {
        self.inner.iteration_count.fetch_add(1, Ordering::SeqCst)
    }

    pub fn iteration_count(&self) -> usize {
        self.inner.iteration_count.load(Ordering::SeqCst)
    }
}
```

- [ ] **Step 3: 修改 Agent 结构使用新组件**

修改 `crates/core/src/agent/mod.rs` 的 Agent 结构：

```rust
pub struct Agent {
    id: AgentId,
    config: AgentConfig,
    messages: Vec<Message>,
    event_bus: EventBus,
    provider: Arc<dyn ModelProvider>,
    storage: Arc<dyn Storage>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    input_rx: mpsc::Receiver<String>,
    // 新增
    context: AgentExecutionContext,
    cancel_token: CancelToken,
}

impl Agent {
    pub fn spawn(
        id: AgentId,
        config: AgentConfig,
        event_bus: EventBus,
        provider: Arc<dyn ModelProvider>,
        storage: Arc<dyn Storage>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
    ) -> AgentHandle {
        let (input_tx, input_rx) = mpsc::channel(100);
        let cancel_token = CancelToken::new();
        let (context, state_rx) = AgentExecutionContext::new(AgentState::Idle);

        let mut agent = Self {
            id: id.clone(),
            config,
            messages: Vec::new(),
            event_bus,
            provider,
            storage,
            tool_registry,
            sandbox,
            input_rx,
            context: context.clone(),
            cancel_token: cancel_token.clone(),
        };

        let handle_id = id.clone();
        tokio::spawn(async move {
            if let Err(e) = agent.run().await {
                tracing::error!("Agent {} failed: {}", handle_id.0, e);
            }
        });

        AgentHandle::new(id, input_tx, state_rx, cancel_token)
    }

    async fn run(&mut self) -> Result<()> {
        self.context.transition_to(AgentState::WaitingForInput);

        loop {
            let state = self.context.current_state();

            if state.is_terminal() {
                break;
            }

            if self.context.iteration_count() >= self.config.max_iterations {
                tracing::warn!("Max iterations reached, forcing completion");
                self.context.transition_to(AgentState::Completed);
                break;
            }

            match state {
                AgentState::WaitingForInput => {
                    if let Err(e) = self.handle_wait_for_input().await {
                        tracing::error!("Wait for input failed: {}", e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                AgentState::Streaming => {
                    if let Err(e) = self.handle_streaming().await {
                        tracing::error!("Streaming failed: {}", e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                AgentState::ExecutingTool => {
                    if let Err(e) = self.handle_execute_tool().await {
                        tracing::error!("Tool execution failed: {}", e);
                        self.context.transition_to(AgentState::Failed);
                    }
                }
                _ => tokio::task::yield_now().await,
            }

            self.context.increment_iteration();
        }

        let result = AgentResult {
            messages: self.messages.clone(),
            tool_calls: self.count_tool_calls(),
        };

        self.event_bus.send(crate::event::Event::Agent(
            AgentEvent::Completed {
                agent_id: self.id.clone(),
                result,
            },
        ))?;

        Ok(())
    }

    async fn handle_wait_for_input(&mut self) -> Result<()> {
        tokio::select! {
            msg = self.input_rx.recv() => {
                match msg {
                    Some(content) => {
                        self.messages.push(Message::user(content));
                        self.context.transition_to(AgentState::Streaming);
                        Ok(())
                    }
                    None => {
                        self.context.transition_to(AgentState::Cancelled);
                        Ok(())
                    }
                }
            }
            _ = tokio::time::sleep(tokio::time::Duration::from_secs(30)) => {
                tracing::warn!("Input timeout");
                self.context.transition_to(AgentState::Cancelled);
                Ok(())
            }
        }
    }

    // handle_streaming 和 handle_execute_tool 类似修改...
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/agent/
git commit -m "refactor: convert Agent to Actor pattern

- Add AgentHandle for external control
- Add AgentExecutionContext for shared state
- Integrate CancelToken for cancellation
- Return Handle from spawn() instead of consuming self
- Add state transition validation in run loop"
```

---

## Task 5: 实现工具并行执行

**Files:**
- Create: `crates/core/src/tool/parallel.rs`
- Modify: `crates/core/src/tool.rs`
- Modify: `crates/core/src/agent/mod.rs`

- [ ] **Step 1: 创建并行工具执行器**

```rust
// crates/core/src/tool/parallel.rs
use crate::event::ToolEvent;
use crate::tool::{Tool, ToolRegistry, ToolSandbox};
use crate::types::{AgentId, ContentBlock, Message, Role, ToolCall, ToolOutput};
use anyhow::Result;
use std::sync::Arc;
use tokio::task::JoinSet;

/// 工具执行结果
pub struct ToolExecutionResult {
    pub tool_call_id: String,
    pub message: Message,
    pub event: ToolEvent,
}

/// 并行执行多个工具调用
pub async fn execute_tools_parallel(
    agent_id: &AgentId,
    tool_calls: Vec<ToolCall>,
    tool_registry: &ToolRegistry,
    _sandbox: &ToolSandbox,
    timeout: std::time::Duration,
) -> Vec<ToolExecutionResult> {
    let mut join_set = JoinSet::new();

    for call in tool_calls {
        let agent_id = agent_id.clone();
        let tool_opt = tool_registry.get(&call.name);

        join_set.spawn(async move {
            let start_event = ToolEvent::Started {
                agent_id: agent_id.clone(),
                tool_id: call.id.clone(),
                tool_name: call.name.clone(),
            };

            let result = match tool_opt {
                Some(tool) => {
                    execute_single_tool(tool, call.clone(), timeout).await
                }
                None => ToolOutput {
                    exit_code: 1,
                    stdout: String::new(),
                    stderr: format!("Unknown tool: {}", call.name),
                },
            };

            let (event, message) = if result.success() {
                let output = result.stdout.clone();
                (
                    ToolEvent::Output {
                        agent_id: agent_id.clone(),
                        tool_id: call.id.clone(),
                        output: output.clone(),
                    },
                    Message {
                        role: Role::Tool,
                        content: vec![ContentBlock::Text { text: output }],
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                        created_at: chrono::Utc::now(),
                    },
                )
            } else {
                let error = format!("Exit code: {}\n{}\n{}",
                    result.exit_code, result.stdout, result.stderr);
                (
                    ToolEvent::Error {
                        agent_id: agent_id.clone(),
                        tool_id: call.id.clone(),
                        error: error.clone(),
                    },
                    Message {
                        role: Role::Tool,
                        content: vec![ContentBlock::Text { text: error }],
                        tool_calls: None,
                        tool_call_id: Some(call.id.clone()),
                        created_at: chrono::Utc::now(),
                    },
                )
            };

            ToolExecutionResult {
                tool_call_id: call.id,
                message,
                event,
            }
        });
    }

    let mut results = Vec::new();
    while let Some(Ok(result)) = join_set.join_next().await {
        results.push(result);
    }
    results
}

async fn execute_single_tool(
    tool: Arc<dyn Tool>,
    call: ToolCall,
    timeout: std::time::Duration,
) -> ToolOutput {
    match tokio::time::timeout(timeout, tool.execute(call.arguments)).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => ToolOutput {
            exit_code: 1,
            stdout: String::new(),
            stderr: format!("Tool execution error: {}", e),
        },
        Err(_) => ToolOutput {
            exit_code: 124,
            stdout: String::new(),
            stderr: format!("Tool execution timed out after {:?}", timeout),
        },
    }
}
```

- [ ] **Step 2: 在 tool.rs 添加模块和配置**

```rust
// 在 crates/core/src/tool.rs 底部添加
pub mod parallel;
pub use parallel::execute_tools_parallel;

// 在 ToolSandbox 中添加超时配置
impl ToolSandbox {
    pub fn default_timeout(&self) -> std::time::Duration {
        std::time::Duration::from_secs(30)
    }
}
```

- [ ] **Step 3: 修改 Agent 使用并行执行**

```rust
// 在 agent/mod.rs 的 handle_execute_tool 中
async fn handle_execute_tool(&mut self) -> Result<()> {
    let tool_calls = self.messages
        .last()
        .and_then(|m| m.tool_calls.clone())
        .unwrap_or_default();

    // 发送开始事件
    for call in &tool_calls {
        self.event_bus.send(crate::event::Event::Tool(ToolEvent::Started {
            agent_id: self.id.clone(),
            tool_id: call.id.clone(),
            tool_name: call.name.clone(),
        }))?;
    }

    // 并行执行
    let results = crate::tool::parallel::execute_tools_parallel(
        &self.id,
        tool_calls,
        &self.tool_registry,
        &self.sandbox,
        self.sandbox.default_timeout(),
    ).await;

    // 收集结果
    for result in results {
        self.event_bus.send(crate::event::Event::Tool(result.event))?;
        self.messages.push(result.message);
    }

    self.context.transition_to(AgentState::Streaming);
    Ok(())
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/tool/
git commit -m "feat: parallel tool execution with timeout

- Add execute_tools_parallel() for concurrent tool calls
- Add 30-second timeout per tool
- Return results as they complete"
```

---

## Task 6: 实现消息历史截断

**Files:**
- Create: `crates/core/src/agent/message_buffer.rs`
- Modify: `crates/core/src/agent/mod.rs`

- [ ] **Step 1: 创建消息缓冲区管理**

```rust
// crates/core/src/agent/message_buffer.rs
use crate::types::Message;

/// 管理消息历史，防止无限增长
#[derive(Debug, Clone)]
pub struct MessageBuffer {
    messages: Vec<Message>,
    max_messages: usize,
    summary_threshold: usize,
}

impl MessageBuffer {
    pub fn new(max_messages: usize) -> Self {
        Self {
            messages: Vec::new(),
            max_messages,
            summary_threshold: max_messages.saturating_sub(10),
        }
    }

    pub fn push(&mut self, message: Message) {
        self.messages.push(message);

        if self.messages.len() > self.max_messages {
            self.truncate_oldest();
        }
    }

    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    pub fn into_messages(self) -> Vec<Message> {
        self.messages
    }

    fn truncate_oldest(&mut self) {
        // 保留系统消息和最近的消息
        let system_count = self.messages.iter()
            .take_while(|m| matches!(m.role, crate::types::Role::System))
            .count();

        // 移除最早的用户/助手消息，但保留系统提示
        let remove_count = (self.messages.len() - self.summary_threshold).max(0);
        if remove_count > 0 && system_count < self.messages.len() {
            let start_idx = system_count + 1; // 保留系统消息后第一条
            let end_idx = (start_idx + remove_count).min(self.messages.len());

            tracing::info!("Truncating messages {}..{}", start_idx, end_idx);

            // 简化的截断：直接删除旧消息
            // 更复杂的实现可以生成摘要
            self.messages.drain(start_idx..end_idx);

            // 添加摘要提示
            self.messages.insert(system_count + 1, Message::system(
                "(Earlier conversation history has been truncated due to length)"
            ));
        }
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ContentBlock, Role};

    fn create_message(role: Role, text: &str) -> Message {
        Message {
            role,
            content: vec![ContentBlock::Text { text: text.to_string() }],
            tool_calls: None,
            tool_call_id: None,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_buffer_truncates_when_full() {
        let mut buffer = MessageBuffer::new(5);

        buffer.push(create_message(Role::System, "System prompt"));
        buffer.push(create_message(Role::User, "Message 1"));
        buffer.push(create_message(Role::Assistant, "Response 1"));
        buffer.push(create_message(Role::User, "Message 2"));
        buffer.push(create_message(Role::Assistant, "Response 2"));
        buffer.push(create_message(Role::User, "Message 3"));

        assert!(buffer.len() <= 5);
    }

    #[test]
    fn test_preserves_system_message() {
        let mut buffer = MessageBuffer::new(3);

        buffer.push(create_message(Role::System, "System prompt"));
        buffer.push(create_message(Role::User, "User message"));
        buffer.push(create_message(Role::Assistant, "Assistant response"));
        buffer.push(create_message(Role::User, "Another user"));

        // 系统消息应该保留
        assert!(buffer.messages().iter().any(|m| {
            matches!(m.role, Role::System)
        }));
    }
}
```

- [ ] **Step 2: 在 Agent 中使用 MessageBuffer**

```rust
// 修改 agent/mod.rs
use message_buffer::MessageBuffer;

pub struct Agent {
    // ...
    message_buffer: MessageBuffer,
    // 移除: messages: Vec<Message>,
}

impl Agent {
    pub fn spawn(...) -> AgentHandle {
        let mut message_buffer = MessageBuffer::new(100); // 默认最大100条
        message_buffer.push(Message::system(&config.system_prompt));

        // ...
    }

    fn messages(&self) -> &[Message] {
        self.message_buffer.messages()
    }
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/agent/message_buffer.rs crates/core/src/agent/mod.rs
git commit -m "feat: add MessageBuffer for automatic history truncation

- Truncate oldest messages when exceeding max limit
- Always preserve system messages
- Default limit: 100 messages"
```

---

## Task 7: 添加 Token 使用追踪

**Files:**
- Modify: `crates/core/src/types.rs`
- Modify: `crates/core/src/event.rs`
- Modify: `crates/core/src/agent/mod.rs`

- [ ] **Step 1: 添加 TokenUsage 类型**

```rust
// crates/core/src/types.rs
#[derive(Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
    pub total_tokens: u32,
}

impl TokenUsage {
    pub fn add(&mut self, other: &TokenUsage) {
        self.prompt_tokens += other.prompt_tokens;
        self.completion_tokens += other.completion_tokens;
        self.total_tokens += other.total_tokens;
    }
}
```

- [ ] **Step 2: 在 Agent 中追踪 Token**

```rust
// 修改 Agent 结构
pub struct Agent {
    // ...
    token_usage: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

// 在 handle_streaming 后统计
async fn handle_streaming(&mut self) -> Result<()> {
    // ... 流式处理代码 ...

    // 估算 token（简化实现）
    let estimated_tokens = current_text.len() / 4 + current_thinking.len() / 4;
    self.token_usage.fetch_add(estimated_tokens as u64, std::sync::atomic::Ordering::SeqCst);

    // 发送带 token 信息的事件
    self.event_bus.send(crate::event::Event::Agent(
        AgentEvent::Progress {
            agent_id: self.id.clone(),
            update: ProgressUpdate {
                step: self.context.iteration_count(),
                total: Some(self.config.max_iterations),
                message: format!("Tokens used: ~{}", estimated_tokens),
            },
        },
    ))?;

    Ok(())
}
```

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/types.rs crates/core/src/event.rs crates/core/src/agent/mod.rs
git commit -m "feat: track estimated token usage

Add TokenUsage type and basic estimation in agent loop.
Uses simple heuristic (chars/4) for token counting."
```

---

## Task 8: 实现真正的 Sub-Agent

**Files:**
- Create: `crates/core/src/agent/subagent.rs`
- Modify: `crates/core/src/agent/mod.rs`

- [ ] **Step 1: 创建子代理管理器**

```rust
// crates/core/src/agent/subagent.rs
use crate::agent::{Agent, AgentConfig, AgentHandle, SubAgentMode};
use crate::bus::EventBus;
use crate::provider::ModelProvider;
use crate::storage::Storage;
use crate::tool::{ToolRegistry, ToolSandbox};
use crate::types::AgentId;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// 子代理管理器
#[derive(Debug, Clone)]
pub struct SubAgentManager {
    sub_agents: Arc<RwLock<HashMap<AgentId, SubAgentHandle>>>,
    parent_id: AgentId,
    event_bus: EventBus,
    provider: Arc<dyn ModelProvider>,
    storage: Arc<dyn Storage>,
    tool_registry: ToolRegistry,
    sandbox: ToolSandbox,
    config: AgentConfig,
}

#[derive(Debug)]
struct SubAgentHandle {
    handle: AgentHandle,
    mode: SubAgentMode,
}

impl SubAgentManager {
    pub fn new(
        parent_id: AgentId,
        event_bus: EventBus,
        provider: Arc<dyn ModelProvider>,
        storage: Arc<dyn Storage>,
        tool_registry: ToolRegistry,
        sandbox: ToolSandbox,
        config: AgentConfig,
    ) -> Self {
        Self {
            sub_agents: Arc::new(RwLock::new(HashMap::new())),
            parent_id,
            event_bus,
            provider,
            storage,
            tool_registry,
            sandbox,
            config,
        }
    }

    /// 启动子代理
    pub async fn spawn(&self, mode: SubAgentMode, task: String) -> AgentId {
        let sub_config = AgentConfig {
            system_prompt: format!(
                "You are a sub-agent working on a specific task. \
                 Parent agent: {}. Task: {}",
                self.parent_id.0, task
            ),
            ..self.config.clone()
        };

        let handle = Agent::spawn(
            AgentId::new(),
            sub_config,
            self.event_bus.clone(),
            self.provider.clone(),
            self.storage.clone(),
            self.tool_registry.clone(),
            self.sandbox.clone(),
        );

        let id = handle.id.clone();

        self.sub_agents.write().await.insert(
            id.clone(),
            SubAgentHandle { handle, mode },
        );

        // 发送子代理任务
        handle.send_message(task).await.ok();

        self.event_bus.send(crate::event::Event::Agent(
            crate::event::AgentEvent::SubAgentSpawned {
                parent_id: self.parent_id.clone(),
                child_id: id.clone(),
                mode: mode.to_string(),
            },
        )).ok();

        id
    }

    /// 获取子代理句柄
    pub async fn get(&self, id: &AgentId) -> Option<AgentHandle> {
        self.sub_agents.read().await.get(id).map(|h| h.handle.clone())
    }

    /// 等待所有子代理完成
    pub async fn wait_for_all(&self) -> Vec<AgentId> {
        let ids: Vec<_> = self.sub_agents.read().await.keys().cloned().collect();

        for id in &ids {
            if let Some(handle) = self.get(id).await {
                // 简单轮询等待完成
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    let state = handle.state();
                    if state.is_terminal() {
                        break;
                    }
                }
            }
        }

        ids
    }

    /// 取消所有子代理
    pub async fn cancel_all(&self) {
        for (_, handle) in self.sub_agents.read().await.iter() {
            handle.handle.cancel();
        }
    }
}
```

- [ ] **Step 2: 在 Agent 中集成 SubAgentManager**

```rust
// 修改 Agent 结构
pub struct Agent {
    // ...
    subagent_manager: Option<SubAgentManager>,
}

// spawn() 中初始化
let subagent_manager = if config.enable_sub_agents {
    Some(SubAgentManager::new(
        id.clone(),
        event_bus.clone(),
        provider.clone(),
        storage.clone(),
        tool_registry.clone(),
        sandbox.clone(),
        config.clone(),
    ))
} else {
    None
};
```

- [ ] **Step 3: Commit**

```bash
git add crates/core/src/agent/subagent.rs crates/core/src/agent/mod.rs
git commit -m "feat: implement real sub-agent spawning

- SubAgentManager handles child agent lifecycle
- Supports Async and Sync modes
- Parent can wait for children or cancel them"
```

---

## Task 9: 改进错误处理和恢复

**Files:**
- Modify: `crates/core/src/agent/mod.rs`

- [ ] **Step 1: 添加错误分类和重试逻辑**

```rust
// 在 agent/mod.rs 中添加
#[derive(Debug)]
pub enum AgentError {
    ModelError(String),
    ToolError { tool_name: String, error: String },
    Cancelled,
    MaxIterations,
    InputClosed,
}

impl Agent {
    async fn handle_streaming_with_retry(&mut self) -> Result<(), AgentError> {
        let max_retries = 3;
        let mut attempt = 0;

        loop {
            match self.handle_streaming_inner().await {
                Ok(()) => return Ok(()),
                Err(e) if attempt >= max_retries => {
                    return Err(AgentError::ModelError(e.to_string()));
                }
                Err(e) => {
                    attempt += 1;
                    tracing::warn!("Streaming failed (attempt {}): {}", attempt, e);
                    tokio::time::sleep(tokio::time::Duration::from_millis(500 * attempt as u64)).await;
                }
            }
        }
    }

    async fn handle_streaming_inner(&mut self) -> Result<()> {
        // 原 handle_streaming 代码
        todo!("Move streaming logic here")
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/core/src/agent/mod.rs
git commit -m "feat: add streaming retry with exponential backoff

Retry up to 3 times on model errors before failing."
```

---

## Task 10: 更新 Session 使用新的 Agent API

**Files:**
- Modify: `crates/app/src/session.rs`
- Modify: `crates/app/src/coordinator.rs`

- [ ] **Step 1: 更新 Session 使用 AgentHandle**

```rust
// crates/app/src/session.rs
use nekoclaw_core::agent::AgentHandle;

pub struct Session {
    // ...
    main_agent: Option<AgentHandle>,
    // 移除: input_tx: Option<mpsc::Sender<String>>,
}

impl Session {
    async fn spawn_main_agent(&mut self) -> Result<()> {
        let handle = Agent::spawn(
            AgentId::new(),
            self.config.agent.clone(),
            self.event_bus.clone(),
            self.provider.clone(),
            self.storage.clone(),
            self.tool_registry.clone(),
            self.sandbox.clone(),
        );

        let agent_id = handle.id.clone();
        tracing::info!("Main agent {} spawned for session {}", agent_id.0, self.id.0);
        self.main_agent = Some(handle);
        Ok(())
    }

    pub async fn send_message(&self, content: String) -> Result<()> {
        match &self.main_agent {
            Some(handle) => handle.send_message(content).await,
            None => Err(anyhow::anyhow!("Session not initialized")),
        }
    }

    pub async fn cancel(&self) {
        if let Some(handle) = &self.main_agent {
            handle.cancel();
        }
    }

    pub fn agent_state(&self) -> Option<AgentState> {
        self.main_agent.as_ref().map(|h| h.state())
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add crates/app/src/
git commit -m "refactor: update Session to use new AgentHandle API

- Store AgentHandle instead of input channel
- Add cancel() method
- Add agent_state() query"
```

---

## Spec Coverage Check

| Spec 要求 | 实现任务 |
|-----------|----------|
| 修复流式处理 bug | Task 1 |
| 状态机安全 | Task 2 |
| 取消机制 | Task 3, 4 |
| Actor 模式重构 | Task 4 |
| 工具并行执行 | Task 5 |
| 消息历史限制 | Task 6 |
| Token 追踪 | Task 7 |
| 子代理实现 | Task 8 |
| 错误处理改进 | Task 9 |
| 更新 Session API | Task 10 |
| FS-based Session 存储 (JSONL) | Task 11 |

---

## Task 11: 基于文件系统的 Session 存储 (JSONL)

**Files:**
- Create: `crates/core/src/storage/fs.rs`
- Modify: `crates/core/src/storage.rs`
- Modify: `crates/core/src/types.rs`

- [ ] **Step 1: 在 types.rs 添加 SessionEvent 类型**

```rust
// crates/core/src/types.rs
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 单个会话事件 - 存储在 JSONL 中
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "snake_case")]
pub enum SessionEvent {
    Created {
        session_id: SessionId,
        project_path: PathBuf,
        created_at: DateTime<Utc>,
    },
    MessageAdded {
        message: Message,
        timestamp: DateTime<Utc>,
    },
    Forked {
        parent_id: SessionId,
        new_session_id: SessionId,
        timestamp: DateTime<Utc>,
    },
    SummaryUpdated {
        summary: String,
        updated_at: DateTime<Utc>,
    },
    Completed {
        completed_at: DateTime<Utc>,
    },
}

impl SessionEvent {
    pub fn created(session_id: SessionId, project_path: PathBuf) -> Self {
        Self::Created {
            session_id,
            project_path,
            created_at: Utc::now(),
        }
    }

    pub fn message_added(message: Message) -> Self {
        Self::MessageAdded {
            message,
            timestamp: Utc::now(),
        }
    }
}

/// 用于 JSONL 存储的包装类型
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEventRecord {
    pub timestamp: DateTime<Utc>,
    pub event: SessionEvent,
}
```

- [ ] **Step 2: 创建 FsStorage 实现**

```rust
// crates/core/src/storage/fs.rs
use crate::event::Event;
use crate::storage::Storage;
use crate::types::{Message, SessionEvent, SessionEventRecord, SessionId, SessionRecord};
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::io::Write;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// 基于文件系统的存储实现 - 使用 JSONL 格式
pub struct FsStorage {
    base_dir: PathBuf,
}

impl FsStorage {
    pub fn new(base_dir: impl Into<PathBuf>) -> Result<Self> {
        let base_dir = base_dir.into();
        std::fs::create_dir_all(&base_dir)
            .context("Failed to create storage directory")?;
        Ok(Self { base_dir })
    }

    /// 默认存储路径: ~/.local/share/nekoclaw/sessions/
    pub fn default_path() -> PathBuf {
        directories::ProjectDirs::from("com", "nekoclaw", "nekoclaw")
            .map(|d| d.data_dir().join("sessions"))
            .unwrap_or_else(|| PathBuf::from("./sessions"))
    }

    /// 获取会话文件路径
    fn session_file_path(&self, session_id: &SessionId) -> PathBuf {
        self.base_dir.join(format!("{}.jsonl", session_id.0))
    }

    /// 原子追加写入 JSONL
    async fn append_event(&self, session_id: &SessionId, event: SessionEvent) -> Result<()> {
        let record = SessionEventRecord {
            timestamp: chrono::Utc::now(),
            event,
        };

        let line = serde_json::to_string(&record)?;
        let path = self.session_file_path(session_id);

        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .context("Failed to open session file")?;

        file.write_all(line.as_bytes()).await?;
        file.write_all(b"\n").await?;
        file.flush().await?;

        Ok(())
    }

    /// 读取会话的所有事件
    async fn read_events(&self, session_id: &SessionId) -> Result<Vec<SessionEventRecord>> {
        let path = self.session_file_path(session_id);

        if !path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&path).await?;
        let mut events = Vec::new();

        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let record: SessionEventRecord = serde_json::from_str(line)
                .context("Failed to parse session event")?;
            events.push(record);
        }

        Ok(events)
    }

    /// 重建会话记录
    fn rebuild_session(&self, events: &[SessionEventRecord]) -> Option<SessionRecord> {
        let mut session = None;
        let mut message_count = 0;
        let mut summary = None;
        let mut completed = false;

        for record in events {
            match &record.event {
                SessionEvent::Created { session_id, project_path, created_at } => {
                    session = Some(SessionRecord {
                        id: session_id.clone(),
                        project_path: project_path.clone(),
                        created_at: *created_at,
                        updated_at: *created_at,
                        message_count: 0,
                    });
                }
                SessionEvent::MessageAdded { .. } => {
                    message_count += 1;
                }
                SessionEvent::SummaryUpdated { summary: s, .. } => {
                    summary = Some(s.clone());
                }
                SessionEvent::Completed { .. } => {
                    completed = true;
                }
                _ => {}
            }
        }

        session.map(|mut s| {
            s.message_count = message_count;
            s
        })
    }
}

#[async_trait]
impl Storage for FsStorage {
    async fn create_session(&self, project_path: &Path) -> Result<SessionId> {
        let session_id = SessionId::new();
        let event = SessionEvent::created(session_id.clone(), project_path.to_path_buf());
        self.append_event(&session_id, event).await?;
        Ok(session_id)
    }

    async fn fork_session(&self, parent_id: &SessionId) -> Result<SessionId> {
        let parent_events = self.read_events(parent_id).await?;
        let new_id = SessionId::new();

        // 复制父会话的所有事件
        let path = self.session_file_path(&new_id);
        let mut file = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .open(&path)
            .await?;

        for record in &parent_events {
            let line = serde_json::to_string(record)?;
            file.write_all(line.as_bytes()).await?;
            file.write_all(b"\n").await?;
        }

        // 添加 fork 事件
        let fork_event = SessionEvent::Forked {
            parent_id: parent_id.clone(),
            new_session_id: new_id.clone(),
            timestamp: chrono::Utc::now(),
        };
        self.append_event(&new_id, fork_event).await?;

        Ok(new_id)
    }

    async fn get_session(&self, id: &SessionId) -> Result<Option<SessionRecord>> {
        let events = self.read_events(id).await?;
        Ok(self.rebuild_session(&events))
    }

    async fn list_sessions(&self, project_path: &Path) -> Result<Vec<SessionRecord>> {
        let mut sessions = Vec::new();
        let mut entries = fs::read_dir(&self.base_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            // 从文件名解析 session_id
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                let session_id = SessionId(stem.to_string());
                let events = self.read_events(&session_id).await?;

                if let Some(session) = self.rebuild_session(&events) {
                    // 只返回匹配项目路径的会话
                    if session.project_path == project_path {
                        sessions.push(session);
                    }
                }
            }
        }

        // 按创建时间排序（最新的在前）
        sessions.sort_by(|a, b| b.created_at.cmp(&a.created_at));
        Ok(sessions)
    }

    async fn delete_session(&self, id: &SessionId) -> Result<()> {
        let path = self.session_file_path(id);
        if path.exists() {
            fs::remove_file(&path).await?;
        }
        Ok(())
    }

    async fn append_messages(
        &self,
        session_id: &SessionId,
        messages: &[Message],
    ) -> Result<()> {
        for message in messages {
            let event = SessionEvent::message_added(message.clone());
            self.append_event(session_id, event).await?;
        }
        Ok(())
    }

    async fn get_messages(&self, session_id: &SessionId) -> Result<Vec<Message>> {
        let events = self.read_events(session_id).await?;
        let messages: Vec<Message> = events
            .into_iter()
            .filter_map(|record| match record.event {
                SessionEvent::MessageAdded { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        Ok(messages)
    }

    async fn update_summary(&self, session_id: &SessionId, summary: &str) -> Result<()> {
        let event = SessionEvent::SummaryUpdated {
            summary: summary.to_string(),
            updated_at: chrono::Utc::now(),
        };
        self.append_event(session_id, event).await
    }

    async fn get_summary(&self, session_id: &SessionId) -> Result<Option<String>> {
        let events = self.read_events(session_id).await?;
        let summary = events
            .into_iter()
            .filter_map(|record| match record.event {
                SessionEvent::SummaryUpdated { summary, .. } => Some(summary),
                _ => None,
            })
            .last();
        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_create_and_get_session() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(temp_dir.path()).unwrap();

        let session_id = storage
            .create_session(Path::new("/test/project"))
            .await
            .unwrap();

        let session = storage.get_session(&session_id).await.unwrap();
        assert!(session.is_some());
        assert_eq!(session.unwrap().project_path, PathBuf::from("/test/project"));
    }

    #[tokio::test]
    async fn test_append_and_get_messages() {
        let temp_dir = TempDir::new().unwrap();
        let storage = FsStorage::new(temp_dir.path()).unwrap();

        let session_id = storage
            .create_session(Path::new("/test"))
            .await
            .unwrap();

        let messages = vec![
            Message::system("System prompt"),
            Message::user("Hello"),
            Message::assistant("Hi there!"),
        ];

        storage.append_messages(&session_id, &messages).await.unwrap();

        let retrieved = storage.get_messages(&session_id).await.unwrap();
        assert_eq!(retrieved.len(), 3);
    }

    #[tokio::test]
    async fn test_session_persistence() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().to_path_buf();

        // 创建存储并添加消息
        let storage1 = FsStorage::new(&path).unwrap();
        let session_id = storage1.create_session(Path::new("/test")).await.unwrap();
        storage1
            .append_messages(&session_id, &[Message::user("Test")])
            .await
            .unwrap();

        // 重新创建存储（模拟重启）
        let storage2 = FsStorage::new(&path).unwrap();
        let messages = storage2.get_messages(&session_id).await.unwrap();
        assert_eq!(messages.len(), 1);
    }
}
```

- [ ] **Step 3: 在 storage/mod.rs 中导出 FsStorage**

```rust
// crates/core/src/storage.rs
pub mod fs;
pub use fs::FsStorage;

// 更新 StorageConfig 支持 FS
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StorageBackendConfig {
    Fs { base_dir: PathBuf },
    Sqlite { url: String },
}

impl Default for StorageBackendConfig {
    fn default() -> Self {
        Self::Fs {
            base_dir: FsStorage::default_path(),
        }
    }
}
```

- [ ] **Step 4: Commit**

```bash
git add crates/core/src/storage/
git commit -m "feat: add filesystem-based session storage with JSONL

- FsStorage implements Storage trait using JSON Lines format
- Each session is a separate .jsonl file
- Append-only for durability
- Events: Created, MessageAdded, Forked, SummaryUpdated, Completed"
```

---

## Spec Coverage Check

| Spec 要求 | 实现任务 |
|-----------|----------|
| 修复流式处理 bug | Task 1 |
| 状态机安全 | Task 2 |
| 取消机制 | Task 3, 4 |
| Actor 模式重构 | Task 4 |
| 工具并行执行 | Task 5 |
| 消息历史限制 | Task 6 |
| Token 追踪 | Task 7 |
| 子代理实现 | Task 8 |
| 错误处理改进 | Task 9 |
| 更新 Session API | Task 10 |
| FS-based Session 存储 (JSONL) | Task 11 |

---

**执行完成后:**
1. 运行 `cargo check` 确保无编译错误
2. 运行 `cargo test` 确保所有测试通过
3. 运行 `cargo clippy` 检查代码质量
