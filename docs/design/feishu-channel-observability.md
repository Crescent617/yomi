# 设计文档：Feishu Channel 可观测性 —— 状态卡片 + Reaction 状态机

**Status:** Draft（v3，按评审结论修订：卡片生命周期绑定 Agent Lifecycle；首个工具才发卡）
**Date:** 2026-07-23

---

## 1. 背景与问题

channel（Feishu/Telegram）中的 agent 是一个**黑箱**：用户 @ 机器人后，只能看到收到确认（`OneSecond` reaction）和最终回复，中间的模型调用、工具执行、重试、压缩等过程完全不可见。长任务（几分钟以上）用户无法区分"正在跑"还是"挂了"。

根因在 `crates/kernel/src/channels/hub.rs::start_event_forwarder`：事件总线里虽有丰富事件（`ToolEvent::Start/End`、`AgentEvent::Retrying`、`ModelEvent::TokenUsage/Compacting`、`AgentEvent::GoalUpdated` 等），forwarder 只转发了 3 个：

| 事件 | 当前行为 |
|------|---------|
| `ModelEvent::Request` | `send_typing`（Feishu 适配器未实现，trait 默认 no-op，**实际什么都没发生**） |
| `ModelEvent::End` | 发送最终回复卡片 |
| `ModelEvent::Error` | 发送 `Error: ...` 文本 |

此外 `feishu.rs::send_reaction` 硬编码 `OneSecond`、忽略 `emoji` 参数；`send_message` 发完即弃，拿不到 `message_id`，无法更新已发消息。

## 2. 目标与非目标

**目标**

- 用户在飞书中能实时看到任务的**阶段**（思考中 / 执行工具 / 重试 / 压缩上下文）、**工具执行摘要**、**耗时**、**token 用量**、**终态**（完成 / 失败 / 已停止）。
- 不刷屏：一段连续运行只产生**一条状态消息**（原地更新），最终回复仍是独立消息（现有行为不变）。
- 平台无关的 trait 扩展，Telegram 可后续跟进。

**非目标**

- **不做 token 级流式输出**（不引入 CardKit 流式模式、不做打字机效果）。最终回复仍一次性发送。
- 不展示 thinking 内容（`blocks_to_text` 已有的安全约束保持不变）。
- 不改 TUI/GUI 的任何渲染。

## 3. 任务边界的界定：绑定 Agent Lifecycle（评审结论）

channel 默认消息路径是 `kernel.send_steer`（`hub.rs::ChannelCommand::None`），steer 语义是"注入当前运行"，因此"一条用户消息 = 一个任务"不成立。经评审，状态卡生命周期**只与 Agent Lifecycle 挂钩**：

| 信号 | 语义（代码事实） | 角色 |
|------|----------------|------|
| `AgentEvent::Lifecycle(Running)` | 每次进入 `Streaming` 状态发射（`agent.rs` 主循环 `AgentState::Streaming` 分支） | **开始跟踪**：当前无活跃运行时首个 Running 触发（仅内存，不发卡） |
| `AgentEvent::Lifecycle(Stopped{reason})` | agent 回到 `Idle` 时发射一次（Completed/Failed/Cancelled/MaxIterations） | **结算**：终态样式按 reason 映射 |
| `UserEvent::Message / Steer` | 用户消息注入时发射（含运行中插队） | **忽略**：不影响生命周期 |
| `ModelEvent::End` | 每个 assistant message 完成都发（工具循环中间轮次也发） | **不作结算信号**；仅触发最终回复发送（现有逻辑） |
| `AgentEvent::Error` | 可能出现在重试中途（如 compaction 失败），运行随后可能恢复 | **仅展示**：更新阶段行，不作结算信号 |

**`Stopped` 的完备性**（已逐路径核实 `agent.rs`/`tool_exec.rs`）：正常完成（`emit_stopped_completed`）、流式失败（`handle_streaming_with_retry` 全部错误出口收敛到 `fail_agent`）、取消（主循环 `handle_cancel`）、最大迭代、`Shutdown` 均发射 `Stopped`；空闲 5 分钟超时发生在 Idle（无活跃运行）。主循环错误兜底分支（`emit_error` + 回 Idle、不发 `Stopped`）在实际代码中不可达——Streaming 分支的错误全部经 `fail_agent`/cancel 转换，Idle 分支的输入错误发生时尚无活跃运行。因此**结算信号只需要 `Stopped`**；watchdog 仅兜底进程崩溃/panic 等零事件场景。

推论：

- **steer 不切卡**：注入不中断运行、不发 Stopped，卡自然延续；`/queue` 消息等 agent 回 Idle 后才被取出（旧卡先结算、queue 消息触发的下一段运行开新卡），天然衔接。
- **goal 模式**：active goal 自动 continue 时不回 Idle、不发 Stopped，一张卡贯穿整个 goal 执行，语义正确。
- **多 session 并发**：不同 thread = 不同 session，`obs_states` 按 `session_id` 隔离。

## 4. 设计约束

| # | 约束 | 含义 |
|---|------|------|
| C1 | 不刷屏 | 一段连续运行只发一条状态卡片消息，过程全部原地 PATCH |
| C2 | 低频更新 | 状态卡片 PATCH 节流（默认最小间隔 1.5s），结算（终态）强制立即更新 |
| C3 | 失败不阻塞 | 状态卡片/reaction 的任何 API 失败只记 `warn!`，绝不影响 agent 主流程和最终回复 |
| C4 | 可关闭 | `ChannelConfig` 增加开关，默认开启；关闭后行为与现状一致 |
| C5 | 沿用线程模型 | 状态卡片与最终回复走相同的 `reply_anchor` 逻辑，落在同一线程 |
| C6 | 首个工具开卡 | `Lifecycle(Running)` 只开始内存跟踪；收到首个 `ToolEvent::Start` 才发送状态卡——纯问答等无工具短任务不产卡（v3 评审结论） |

## 5. 方案总览

两个互补机制：

1. **状态卡片（Progress Card）**：`Lifecycle(Running)` 开始跟踪，**首个 `ToolEvent::Start` 时发一张卡片消息**（无工具的短任务不产卡），随后随事件原地 PATCH；`Lifecycle(Stopped)` 时转为终态（绿=完成 / 红=失败 / 灰=已停止 / 灰=超时失联）。
2. **Reaction 状态机**：运行期间收到的每条用户消息打 `OneSecond`（收到，现有）；结算时这段运行处理过的所有消息统一换 `DONE`（完成）/ `CrossMark`（失败/停止），并给**最后一条内容回复消息**（如有）打上同样的 reaction。

```
用户消息 A ──► [OneSecond]（收到确认，现有）
                │
Lifecycle(Running) ──► 开始跟踪（仅内存，不发卡）
                │  首个 ToolEvent::Start ──► 发送状态卡（蓝）
                │  后续事件              ──► 原地 PATCH（标题=当前阶段，正文=统计行）
                │  用户消息 B（steer）   ──► [OneSecond]
                ▼
Lifecycle(Stopped) / watchdog 失联判定
                │
                ├─ Completed    ──► 卡变绿（完成）+ A、B 消息换 DONE
                ├─ Failed       ──► 卡变红（失败 + 错误摘要）+ A、B 消息换 CrossMark
                ├─ Cancelled    ──► 卡变灰（已停止）+ A、B 消息换 CrossMark
                └─ watchdog     ──► 卡变灰（超时失联）+ reaction 不动
                │
                ▼（与结算独立）
         最终回复消息照常发出（现有 ModelEvent::End 逻辑）
```

### 5.2 与内容卡（最终回复）的关系：完全独立

状态卡与最终回复是**两条独立消息、两条独立发送路径、两套独立生命周期**：

- 状态卡：`send_card`/`update_card`，一段运行 1 张，结算后冻结为终态摘要；
- 内容回复：现有 `send_message`（markdown 卡片），`ModelEvent::End` 触发，一段运行可能多条（工具循环中间轮次带文本也照发，行为不变）；
- 位置：两者走同一套 `reply_anchor` 逻辑落在同一线程（C5）。状态卡先发在上、内容回复随后在下，终态卡恰好成为这段回复的"运行摘要头"；
- **有意不合并**：最终内容不 PATCH 进状态卡——内容可能很长（沿用现有 30k 截断）、一段运行可能多条回复（steer 场景），且状态卡冻结后作为"这段运行干了什么"的持久凭据独立存在；
- 故障隔离：状态卡任何 API 失败只记 `warn!`（C3），内容回复不受影响。

### 5.3 为什么用 PATCH message 而不是 CardKit

| 方案 | 适配度 | 说明 |
|------|--------|------|
| `PATCH /open-apis/im/v1/messages/:message_id` | ✅ 选用 | 更新已发卡片全量内容。无流式需求时频率极低（秒级节流 + 事件驱动），频控无压力；实现简单，复用现有发送路径，天然支持 `reply_in_thread` |
| CardKit `cards/:card_id/batch_update` | ❌ 过度设计 | 面向高频局部更新/流式场景，需管理卡片实体（card_id）与 sequence，复杂度高 |
| 每次发新消息 | ❌ 违反 C1 | 刷屏 |

已核实的 API：

```
PATCH /open-apis/im/v1/messages/:message_id
Authorization: Bearer <tenant_access_token>
body: { "content": <card json string> }        // 全量替换卡片内容

POST   /open-apis/im/v1/messages/:message_id/reactions
body: { "reaction_type": { "emoji_type": "DONE" } }   // 响应 data.reaction_id 需留存

DELETE /open-apis/im/v1/messages/:message_id/reactions/:reaction_id
```

## 6. 核心设计

### 6.1 事件 → 状态卡片映射

| 事件 | 动作 |
|------|------|
| `Lifecycle(Running)`，且无活跃运行 | **开始跟踪**运行状态（`started_at`、阶段、统计），不发卡；卡未发出前一切更新仅写内存 |
| `ToolEvent::Start`，且卡未发出 | **发送**状态卡片（蓝）；anchor 取 `Running` 时固化的 routing（首条触发消息），首个工具前的事件（重试/token）已沉淀在状态里，首发渲染即带出 |
| `Lifecycle(Running)`，已有活跃运行 | 忽略（工具循环每轮都会重发 Running） |
| `ToolEvent::Start { tool_name, .. }` | `tool_count += 1`；标题=执行工具（附工具名） |
| `ToolEvent::End { .. }` | 忽略（不展示单工具耗时与失败数，不 PATCH） |
| `ModelEvent::Request { .. }` | 标题=思考中（每次模型调用重置；无卡平台此事件兼作 typing 指示触发，见 6.4） |
| `ModelEvent::Chunk { content }` | 标题=输出正文（Text chunk）/ 思考中（Thinking chunk） |
| `AgentEvent::Retrying { attempt, max_attempts, reason }` | 标题=重试（附 attempt/max 与 reason 摘要） |
| `AgentEvent::Error { error, .. }` | 标题=出错（附错误摘要；不结算——无论 `is_recoverable`，见 §3 完备性说明） |
| `ModelEvent::Compacting { active }` | 标题=压缩上下文 / 恢复思考中 |
| `ModelEvent::Fallback { from, to }` | 标题=模型降级（from → to） |
| `ModelEvent::TokenUsage { total_tokens, context_window, .. }` | 统计行附 token 用量（覆盖式，保留最新） |
| `AgentEvent::GoalUpdated { status, .. }` | 标题=goal 状态 |
| `UserEvent::Message / Steer { content, .. }` | 忽略；**不影响生命周期** |
| `Lifecycle(Stopped { reason })` | **结算**，终态样式按 reason 映射，见 6.3 |
| `ModelEvent::End` | 不结算；仅现有内联发送最终回复 |
| 其他（`ToolCallDelta` 等） | 忽略 |

标题文案取最近一次事件的**当前值**，不做历史流水。具体图标与措辞属于渲染细节，以 `obs.rs` 代码为准，本文不固定。

### 6.2 卡片结构（card schema 2.0，紧凑布局）

```json
{
  "schema": "2.0",
  "config": { "width_mode": "compact" },
  "header": {
    "title": { "tag": "plain_text", "content": "<阶段标题>" },
    "template": "blue",
    "padding": "4px 12px 4px 12px"
  },
  "body": {
    "padding": "8px 12px 8px 12px",
    "elements": [
      { "tag": "markdown", "text_size": "notation", "content": "<统计行>" }
    ]
  }
}
```

- 紧凑布局：`width_mode: compact`（400px，默认 600px）、header/body 缩小 padding、正文统一 `text_size: notation`（12px 辅助信息字号）、单一 markdown 元素。卡面文案一律英文。
- `header.template`：`blue`（运行中）/ `green`（完成）/ `red`（失败）/ `grey`（已停止、超时）。
- header title = 当前阶段（emoji + 短语）；body = 单行统计（耗时 · 工具总数 · token 用量，工具只记总数不区分种类）。
- 终态卡：标题为终态摘要（完成时附工具数与耗时），失败时 body 附错误摘要行（截断 200 字）。

### 6.3 结算规则（终态判定）

结算信号**只有 `Lifecycle(Stopped)`**（完备性见 §3），外加 watchdog 兜底零事件场景。结算后忽略后续一切非开卡事件，并从 `obs_states` 移除：

| 信号 | 终态样式 | reaction |
|------|---------|----------|
| `Stopped{Completed}` | 绿·完成（附工具数与耗时） | 本段运行所有消息换 `DONE` |
| `Stopped{Failed}` | 红·失败（附错误摘要）；**卡未发出时（无工具运行）直接发送终态卡**，保证失败有解释 | 换 `CrossMark` |
| `Stopped{Cancelled}` | 灰·已停止 | 换 `CrossMark` |
| `Stopped{MaxIterations}` | 红·达到最大迭代数 | 换 `CrossMark` |
| watchdog：session agent 不存活（task 结束/panic，或已回 Idle 但 `Stopped` 丢失） | 灰·超时失联（卡未发出则静默丢弃） | 不动（迟到的真实 `Stopped` 仍会兜底结算 receipt） |

`AgentEvent::Error` 一律只更新阶段行：它既出现在重试中途（可恢复），也出现在 compaction 失败等"致命但随后可能经 `fail_agent` 补发 `Stopped{Failed}`"的路径上，真正可靠的终态只有 `Stopped`。watchdog 覆盖进程崩溃/panic/事件丢失等一切无 `Stopped` 的残余路径。

### 6.4 hub：per-session 状态、节流与 watchdog

`ChannelHub` 新增：

```rust
struct ObsCardState {
    status_msg_id: String,              // 状态卡片 message_id（空 = 尚未发卡）
    chat_id: String,                    // Running 时固化的路由（发卡/失败补发用）
    reply_msg_id: Option<String>,       // Running 时固化的 reply anchor（首条触发消息）
    started_at: std::time::Instant,
    phase: String,                      // 当前阶段（标题由渲染层组装：emoji + 短语）
    tool_count: u32,                    // 工具执行总数（不区分种类，不记耗时/失败数）
    token_footer: Option<String>,
    last_patch_at: std::time::Instant,
}

/// 运行期间收到的消息回执（reaction 状态机目标）
struct RunReceipts {
    items: Vec<(String /*msg_id*/, String /*reaction_id*/)>,
}

obs_states: Arc<DashMap<SessionId, ObsCardState>>,
receipts:   Arc<DashMap<SessionId, RunReceipts>>,
```

**receipt 记录**：`ChannelMessage` 新增 `receipt_reaction_id: Option<String>`（feishu 适配器打完 `OneSecond` 后填充，见 6.6）。hub 处理循环在 `get_or_create_session` 之后把 `(external_message_id, receipt_reaction_id)` 追加到 `receipts[session_id]`。结算时按 6.3 统一切换并清空；无活跃运行时的残留（如 agent 未启动）由下一次结算自然清掉。

**PATCH 位置与顺序**：`start_event_forwarder` 的 rx loop 内**直接 await**（不 spawn），保证同一 session 的 PATCH 严格有序，避免乱序覆盖。单次 PATCH 由 adapter reqwest 30s timeout 兜底；失败仅 `warn!`（C3），后续 PATCH 以全量替换语义自愈。内容回复（`ModelEvent::End`/`Error`）同样**内联 await 发送**——结算时要在最后一条内容消息上打 reaction，必须保证它先于结算落位。

**节流（C2）**：非结算更新，距上次 PATCH < 1.5s 则只更新内存状态、跳过本次 PATCH（下一个事件自然带出最新快照）；结算强制 PATCH。

**watchdog**：forwarder 的 `select!` 增加 60s 周期分支——对 `obs_states` 里的每个 session 调 `Kernel::is_session_running`（conductor 判定：agent task 存活且非 Idle），不存活的按 6.3 结算为「超时失联」并移除。基于**活性查询**而非事件时间戳：长工具调用期间 session 处于 `ExecutingTool`（存活），天然不会假阳性；agent panic/事件丢失导致 `Stopped` 永远不到达时仍能兜底。已知竞态：`Stopped` 已入事件总线但未处理时 sweep 可能抢先结算（卡片提前一拍变灰，receipt 由真实 `Stopped` 到达时正常结算）。

**typing 回退**：`ModelEvent::Request → send_typing` 仅在**平台不支持状态卡**（`supports_status_card() == false`，如 Telegram）或 `observability` 关闭时调用；Feishu 等卡片平台的进度信号由状态卡承载，不再发 typing。

### 6.5 Reaction 状态机

| 时机 | 目标消息 | 动作 | emoji |
|------|---------|------|-------|
| 收到消息（现有） | 该用户消息 | 添加 | `OneSecond` |
| 结算（Completed） | 本段运行全部 receipt 用户消息 | 删 `OneSecond` + 添加 | `DONE` |
| 结算（Failed/Cancelled/MaxIterations） | 同上 | 删 `OneSecond` + 添加 | `CrossMark` |
| 结算（Completed） | **最后一条内容回复消息**（如有） | 添加 | `DONE` |
| 结算（Failed/Cancelled/MaxIterations） | 同上 | 添加 | `CrossMark` |
| watchdog 失联判定 | — | 不动 | — |

- 删除需要 `reaction_id`：`send_reaction` 改为返回 `Option<String>`（POST 响应的 `data.reaction_id`），随 `ChannelMessage.receipt_reaction_id` 传给 hub。
- 内容回复的 message_id 由 `send_message` 返回值记录；为保证"最终回复先于结算 reaction 落位"，forwarder 中内容回复改为**内联 await 发送**（不再 spawn，见 R3）。
- 当前 `feishu.rs::send_reaction` **忽略 `emoji` 参数、硬编码 `OneSecond`**，本次修复为透传。

### 6.6 PlatformAdapter trait 变更

```rust
#[async_trait]
pub trait PlatformAdapter: Send + Sync {
    // ... run_receiver / send_files 不变 ...

    /// 发送卡片消息，返回 message_id（用于后续 update_card）。
    /// 默认实现退化为 send_message 并返回 None（Telegram 现状）。
    async fn send_card(
        &self,
        external_chat_id: &str,
        card_json: &str,
        reply_msg_id: Option<&str>,
    ) -> Result<Option<String>, ChannelError> { ... }

    /// 原地更新已发送的卡片消息。默认 no-op（不支持的平台静默跳过）。
    async fn update_card(
        &self,
        message_id: &str,
        card_json: &str,
    ) -> Result<(), ChannelError> { Ok(()) }

    /// 透传 emoji_type；返回 reaction_id 供删除。默认 Ok(None)。
    async fn send_reaction(
        &self, external_chat_id: &str, message_id: &str, emoji: &str,
    ) -> Result<Option<String>, ChannelError> { Ok(None) }

    /// 删除 reaction。默认 no-op。
    async fn remove_reaction(
        &self, message_id: &str, reaction_id: &str,
    ) -> Result<(), ChannelError> { Ok(()) }
}
```

注意 `send_reaction` 返回值从 `()` 变为 `Option<String>` 是签名 breaking change，调用点仅 hub 与 feishu 内部各一处，迁移成本可控。Telegram 保持默认实现（后续可用 `editMessageText` 实现 `update_card`，不在本设计范围）。

### 6.7 配置

`ChannelConfig` 增加：

```rust
/// 状态卡片 + reaction 状态机（默认开启）
#[serde(default = "default_observability")]
pub observability: bool,
```

`docs/config-schema.json` 与 `docs/CONFIG.md` 同步更新。全项目沿用 `snake_case`。

## 7. 数据流时序

```
用户 @bot 发消息 A "帮我跑下测试"
  │
  ▼ feishu ws: im.message.receive_v1
parse_event_json ──► send_reaction(A, OneSecond) ──► ChannelMessage{ receipt_reaction_id, .. }
  │
  ▼ hub 处理循环
get_or_create_session ──► receipts[sid].push(A) ──► kernel.send_steer
  │
  ▼ event forwarder
Lifecycle(Running)          ──► obs_states.insert(sid, ...)（仅内存跟踪，不发卡）
ToolEvent::Start  bash      ──► 首个工具 → send_card(蓝, anchor=Running 时的路由)
                                ──► 后续更新内存；距上次>1.5s → PATCH
ToolEvent::End    bash ✓    ──► 更新内存（节流窗口内，跳过 PATCH）
用户发消息 B（steer 插队）   ──► receipts[sid].push(B)
AgentEvent::Retrying 1/5    ──► PATCH(标题=重试)
ModelEvent::TokenUsage      ──► 更新统计行（下轮 PATCH 带出）
ModelEvent::End             ──► 内联发送最终回复并记录 content msg_id（不结算）
Lifecycle(Stopped{Completed})──► PATCH 终态(绿) ──► A、B 换 DONE + 内容消息打 DONE
                              ──► obs_states.remove(sid)；receipts.remove(sid)
```

`/stop` 路径：`ChannelCommand::Stop` → `kernel.cancel` → `Stopped(Cancelled)` → 灰卡 + 换 `CrossMark` + 清理。
`/queue` 路径：queue 消息在 mailbox 排队 → agent 回 Idle（旧卡结算）→ 消息取出注入 → 新 `Running` → 开新卡。

## 8. 接口变化与影响范围

| 文件 | 变更 |
|------|------|
| `channels/mod.rs` | trait 新增 `send_card`/`update_card`/`remove_reaction`/`supports_status_card`；`send_reaction` 签名改返回 `Option<String>`；`ChannelMessage` 加 `receipt_reaction_id: Option<String>`；`ChannelConfig` 加 `observability: bool` |
| `channels/feishu.rs` | 实现新方法（`supports_status_card` 返回 true）；`send_reaction` 透传 emoji 并解析返回 `reaction_id`；`parse_event_json` 填充 `receipt_reaction_id` |
| `channels/telegram.rs` | 适配 trait 签名（默认实现即可；保留 typing 指示作为进度回退） |
| `channels/hub.rs` | 事件映射（6.1）；节流 PATCH；结算（6.3）；watchdog；receipt 记录；typing 回退（无卡平台/开关关闭时） |
| `event/mod.rs` + `tui` | 删除无发射方的 `ModelEvent::Error` 死事件及消费分支（agent 错误统一走 `AgentEvent::Error`） |
| `docs/CONFIG.md` / `docs/config-schema.json` | `observability` 字段 |

无数据库变更；TUI/GUI/server 不受影响（事件总线只读消费，新增消费方不改协议；死事件删除不影响在线事件）。

## 9. 实施计划

1. **Phase 1（trait + feishu 能力）**：trait 扩展、`send_reaction` 修复透传并返回 id、`send_card`/`update_card`/`remove_reaction` 的 feishu 实现、telegram 适配。单测覆盖 feishu 请求体构造（`feishu_test.rs`）。
2. **Phase 2（hub 状态卡片）**：`ObsCardState` 聚合 + 事件映射 + 节流 PATCH + 结算 + watchdog。`hub_test.rs` 用 mock adapter 断言：开卡时机、PATCH 次数、各 reason 的终态样式、steer 不切卡、watchdog 结算。
3. **Phase 3（reaction 状态机 + 配置开关）**：`receipt_reaction_id` 透传与 receipts 记录、结算 reaction 切换、`observability` 配置与文档。

## 10. 风险与未决问题

| # | 风险 | 缓解 |
|---|------|------|
| R1 | PATCH 频率超限（飞书单消息更新 QPS 限制） | 1.5s 节流 + 事件驱动，远低于限制；失败仅 warn 且后续 PATCH 会带出最新状态（全量替换语义，天然自愈） |
| R2 | 进程崩溃/panic/事件丢失导致无 `Stopped`，卡片永远"运行中" | watchdog 60s 周期查询 session 活性（agent task 存活且非 Idle），失效即结算「超时失联」；长工具调用 session 始终存活，无假阳性；其余路径已核实必有 `Stopped`（§3） |
| R3 | 多 session 并发时 forwarder 内 await PATCH 阻塞其他 session 的状态更新 | 单 PATCH 约百毫秒级、秒级节流；可接受。若实测成为瓶颈，再改为 per-session 串行队列 + spawn |
| R4 | 进程重启后 `obs_states`/`receipts` 丢失，运行中的任务状态卡片不再更新 | 可接受：任务本身也已中断；卡片停留在最后状态 |
| R5 | reaction 删除失败残留 `OneSecond` | 残留语义不误导（确实收到过）；仅 warn |
| R6 | 长运行卡横跨多条 steer 消息，卡锚定在首条消息线程位置 | 设计语义（lifecycle 绑定） |

## 11. 附录：emoji_type 取值

已用于本设计：`OneSecond`（收到）、`DONE`（完成）、`CrossMark`（失败/停止）。可选备选：`OnIt`（进行中）、`OK`。完整列表见飞书文档 `im-v1/message-reaction/emojis-introduce`。
