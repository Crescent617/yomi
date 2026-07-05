# 无 Session 架构重构执行计划

## 阶段一：基础设施（无依赖，可先做）

### 1. 新建 `Mailbox`
- 新建 `crates/kernel/src/event_bus/mailbox.rs`
- 实现 `Mailbox`：
  - `steer: Mutex<VecDeque<ContentBlock>>`
  - `normal: Mutex<VecDeque<AgentInput>>`
  - `push(input)` / `push_steer(content)` / `try_pull(count)` / `try_pull_steer(count)` / `is_steer_empty()` / `clear()`
- 在 `event_bus/mod.rs` 导出

### 2. 改造 `InputBus`
- 将 `InputBus` 退化为极简 channel 包装：
  - 只保留 `tx: mpsc::Sender<(SessionId, AgentInput)>`
  - `new()` 返回 `(Arc<InputBus>, Receiver)`
  - `publish(sid, input)` 唯一方法
- 删除 `InputBusHandle`、`subscribe`、DashMap 路由表、`cancel` 方法、`publish_user`
- `cancel` 语义走 `AgentInput::Cancel` 消息

### 3. 改造 `AgentSpawnArgs`
- 删除 `ask_user_state`、`cancel_token` 字段（Agent 内部自己新建）
- 新增 `mailbox: Arc<Mailbox>` 字段

---

## 阶段二：Agent 改造（依赖 Mailbox）

### 4. 改造 `Agent`
- `spawn()` 改为 `async fn spawn(...)`，内部直接 await 异步初始化：
  ```rust
  pub async fn spawn(shared: &Arc<AgentShared>, args: AgentSpawnArgs) -> Self
  ```
- `start_loop(self)` 公开，由调用方 `tokio::spawn(agent.start_loop())`：
  ```rust
  pub async fn start_loop(mut self) -> Result<(), AgentError>
  ```
- `spawn()` 内部新建 `cancel_token`、`permission_state`、`ask_user_state`
- `start_loop` Idle 分支改为：
  - `is_steer_empty()` 为 false → `transition_to(Streaming)`
  - `try_pull(1)` 有消息 → `handle_input`
  - `try_pull(1)` 为空 → `break`（尾部 defer 发 Shutdown）
- `handle_streaming` 内部开头批量 `try_pull_steer(20)`，合并为一条用户消息注入 `message_buffer`
- 删除显式 `close()` / `Shutdown` 信号处理，Agent 空载自然退出

---

## 阶段三：Conductor（依赖 InputBus + Mailbox + Agent）

### 5. 新建 `Conductor`
- 新建 `crates/kernel/src/app/conductor.rs`
- 实现：
  ```rust
  pub struct Conductor {
      agent_shared: Arc<AgentShared>,
      active: DashMap<SessionId, ActiveAgent>,
      rx: mpsc::Receiver<(SessionId, AgentInput)>,
      event_bus: Arc<EventBus>,
  }
  ```
- `ActiveAgent` 包含 `mailbox: Arc<Mailbox>`、`handle: JoinHandle<()>`、`cancel_token: CancelToken`、`state: Atomic<AgentState>`
- `run()`：`select!` 同时监听：
  - `rx.recv()`：消息分发 / Cancel 处理 / lazy spawn
  - `EventBus`：`StateChanged` 更新 `active[sid].state`；`Shutdown` 执行 `active.remove(sid)`
- `get_state(sid)` 供 Coordinator 查询
- `spawn_agent()`：加载历史 → `Agent::spawn(args).await` → `tokio::spawn(agent.start_loop())` → 插入 `active`

---

## 阶段四：Coordinator 改造（依赖 Conductor）

### 6. 改造 `Coordinator`
- 删除 `sessions: DashMap<SessionId, Arc<RwLock<Session>>>`
- 删除 `require_session()` / `require_session_or_restore()` / `init_session()`
- 删除 `state_cache`
- 新增 `conductor: Arc<Conductor>`
- `send_message` 内联 title update（提取文本 → `session_store.update_title` → `EventBus::TitleUpdated`）
- `cancel` / `send_permission_response` / `send_ask_user_response` / `send_steer` 统一走 `input_bus.publish()`
  - `send_message` 调用 `input_bus.publish(sid, AgentInput::User { ... })`
- `get_session_status` 改为 `self.conductor.get_state(sid)`
- `create_session` 改为：创建 DB 记录 → `input_bus.publish(sid, AgentInput::User { ... })`（Conductor 会 lazy spawn）
- `restore_session` 改为：确认 DB 记录存在 → `input_bus.publish(sid, ...)` 或直接触发 spawn

---

## 阶段五：清理外围依赖

### 7. 删除 `Session`
- 删除 `app/session.rs`
- 删除 `SessionConfig`（字段扁平化到 `AgentConfig` / `AgentSpawnArgs`）

### 8. 适配 Subagent
- `tools/subagent.rs` 中本地创建 `Mailbox`
- 构造 `AgentSpawnArgs` 时 `with_mailbox(mailbox)`
- `Agent::spawn(args).await` + 自己 `tokio::spawn(agent.start_loop())`，不走 Conductor/InputBus
- Subagent 完成后通过 EventBus 收集结果

### 9. 适配 `lib.rs` 导出
- `pub use app::{Session, SessionConfig}` → `pub use app::{Coordinator, Conductor}`

### 10. 编译修复
- 修复所有因删除 `Session` 导致的编译错误
- 修复 CLI/GUI/Server 中对 `Coordinator` API 的调用（调用方式基本不变，内部实现变了）
- `cargo check` 全量通过
- `cargo clippy --all-targets --all-features` 通过

---

## 验证清单

| 验证项 | 方式 |
|--------|------|
| 创建 session + 发消息 | Coordinator API 调用，Agent 正常 streaming |
| Cancel | 发 Cancel 后 Agent 立即中断，发 Shutdown |
| 连续发多条消息 | Agent 处理完一轮后，Mailbox 有积压则继续处理，无积压则退出；下一条消息触发 Conductor 重新 spawn |
| Steer | 发 steer 后 Agent 当前/下一轮 Streaming 前注入 |
| Permission / AskUser | 响应通过 InputBus 走 Mailbox，Agent 收到后恢复 |
| Subagent | 正常 spawn + start_loop，完成，返回结果 |
| 关闭 session | Agent 自然退出，Conductor 清理，无内存泄漏 |
| EventBus 事件 | TitleUpdated / StateChanged / Shutdown 正常流转 |
