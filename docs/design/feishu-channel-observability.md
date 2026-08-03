# 设计文档：Channel 运行可观测性 —— 一段 run 一条消息

**Status:** Active（2026-07-24，按当前实现重写）

---

## 1. 背景与目标

channel（Feishu/Telegram）中的 agent 原本是黑箱：用户 @ 机器人后只能看到收到确认与最终回复，中间过程不可见，长任务刷屏（agent loop 每轮文本各一个气泡）。本设计让**一段连续运行在 IM 里恰好呈现为一条消息**：

- 运行中：一张紧凑状态卡，原地 PATCH 展示阶段、统计、last tool、whisper；
- 结束时：同一张卡 morph 为最终回复（正文 + 可折叠运行轨迹）；
- 例外：run 期间用户又发了消息时，状态卡冻结为终态凭据留在上方，回复作为新消息沉底（保证回复不出现在用户消息之上）。

目标：进度可见、不刷屏、内容不丢。非目标：token 级流式输出、thinking 内容外泄、改动 TUI/GUI 渲染。

## 2. Run 的界定

状态卡生命周期只与 Agent Lifecycle 挂钩（steer 注入不打断 run）：

| 信号 | 角色 |
|------|------|
| `Lifecycle(Running)` | 每次进入 Streaming 发射；首个 Running 开始内存跟踪并**立即开卡** |
| `Lifecycle(Stopped{reason})` | agent 回 Idle 时发一次（Completed/Failed/Cancelled/MaxIterations）→ **结算** |
| `ModelEvent::End` | 每轮 assistant message 完成都发 → 文本进回复缓冲，**不作结算信号** |
| `AgentEvent::Error` | 重试中途（可恢复）也发 → 仅更新阶段标题 |
| `UserEvent::*` | 不影响生命周期 |

`Stopped` 的完备性已逐路径核实（agent.rs 所有退出路径均发射）；watchdog 兜底崩溃/事件丢失（§7）。goal 模式自动 continue 不回 Idle，一张卡贯穿整个 goal。

## 3. 状态卡（运行中）

- **开卡**：首个 `Lifecycle(Running)` 即发——run 从一开始就可见（慢首请求、长 thinking、429 重试循环都有占位卡），占位文案（`Pondering…` 轮换）填充到首个 tool/文本到达；`Running` 每 turn 重发，每 run 只开一次卡。卡即回复——结算时同一张卡 morph 为最终回复。
- **卡面**（compact 400px、蓝 header、12px notation 正文、英文文案）：
  - header 标题 = 当前阶段（💭 Thinking / 🐾 Typing / 🐹 工具名 / 🔁 重试 / ⚠️ 错误 / 📦 压缩 / ↪️ 降级 / 🎯 Goal，取最新事件值）
  - 统计行：`⏱ 耗时 · N steps · N tools · ctx: x / y · out ~z（灰）`；ctx 为 provider 实报（响应结束才有），out 为本 run 累计输出预估（text/thinking ≈4 字节/token、工具参数 ≈2，单调增长）——首个长 thinking 也能看到 token 走动
  - `🔧 工具名 · 主参数摘要`（last tool，常驻）
  - `💬 灰色文本尾部`（whisper：chunk 累积、`Request` 清空、`End` 用完整文本自愈）
  - 动态文本行统一 ≤100 字符（含省略号，unicode 安全截断）
- **更新**：PATCH 全量替换，3s 节流（内存状态随到随改，下一个事件带出最新快照），结算强制立即更新；开卡发送失败每 run 只尝试一次（防 API 故障时的超时风暴）。

### 事件映射

| 事件 | 动作 |
|------|------|
| `ToolEvent::Start` | tool_count+1；标题=🐹 工具名；last_tool 更新 |
| `ToolEvent::End` | 标题回 Thinking；trace 标记工具完成/失败 |
| `ModelEvent::Request` | 标题=Thinking；清 whisper；清零当前响应的输出计数（重试重发 Request，失败 attempt 不重复计） |
| `ModelEvent::Chunk` | Text → 标题 Typing + whisper 累积；Thinking → 标题 Thinking；两者均计入输出预估 |
| `ModelEvent::ToolCallDelta` | 不计卡面状态，仅参数字节计入输出预估 |
| `ModelEvent::End` | whisper 用完整文本自愈；文本进回复缓冲（§5）；当前响应预估折入 run 累计 |
| `ModelEvent::TokenUsage` | 更新 token 统计（覆盖式） |
| `Retrying` / `Error` / `Compacting` / `Fallback` / `GoalUpdated` | 更新阶段标题（Retrying 含 `in Ns` 等待时长） |

## 4. 结算（deliver_reply）

`Stopped`（或 watchdog 超时）时由 hub 的 `deliver_reply` 统一交付：

| 条件 | 行为 |
|------|------|
| 有卡平台 + observability + 无 mid-run 消息 | **morph**：卡原地 PATCH 为最终回复卡（无 header：异常提示行? + 正文 + 轨迹面板） |
| 有卡平台 + observability + 有 mid-run 消息（见 §6）且 `mid_run_split: true` | **冻结 + 沉底**：卡原地 PATCH 为终态凭据（header ✅ Done / ❌ Failed / ⏹ Stopped / ⏰ Timed out + 统计行），回复卡（正文 + 轨迹面板）作为新消息发出（锚定 run 起点）。回复带不了轨迹时（无正文 / `tool_trace: false` / 无回复）冻结卡自己保留轨迹面板——轨迹不丢 |
| 有卡平台 + observability + 有 mid-run 消息且 `mid_run_split: false` | **morph**（同上，一 run 一消息；答案停在 run 起点，用户 mid-run 消息之下不沉底） |
| 无卡平台 或 `observability: false` | `flush_reply` 发新消息（无卡平台 obs 仅内存态结算） |
| settle 未落地（无 run 状态 / `send_card` 失败） | 回复交还并回退 `flush_reply`——单点故障只降级展示形式，不丢内容 |

- **异常提示行**（morph 卡正文首行）：`❌ **Error** …`（Failed，截断 200 字）/ `❌ Max iterations reached (n)` / `⏰ Session lost (timed out)`；Completed/Cancelled 无。
- **失败必有解释**：run 无文本且无卡时，Failed 也发一张 notice 卡；其余完全无内容的 run 不发消息。
- **无回复可 morph**（crash/事件丢失的兜底路径）退回终态 header 卡。
- **无文本**：有卡则 morph 为纯轨迹卡；flush 路径无文本不发（与旧行为一致）。

## 5. 回复缓冲与轨迹面板

forwarder 维护 per-session `RunReplyBuffer`（cap 100 条防 goal 长 run 膨胀）：`ModelEvent::End` 文本与 `ToolEvent::Start/End` 按时间交错记录，首个 `Running` 后保留（steer 不清），`Stopped`/watchdog 时排空。**只有最后一个文本成为正文**，中间文本降级为轨迹中的灰色 `💬` 旁白（≤80 字）。

轨迹面板（Feishu card JSON 2.0 `collapsible_panel`，默认收起，需客户端 V7.9+，旧客户端面板区显示升级占位图；面板标题 `🐾 Run trace · N tools · elapsed`）：

- 工具条目：`✅/❌/⏳ 工具名 · 参数摘要 · 耗时`（⏳ = 取消时仍在执行）
- 参数摘要按工具取主参数（`shell→command`、`read/edit→path`、`write→file_path`、`glob/grep→pattern`、`web_fetch→url`、`web_search→query`、`agent→description`；未收录工具按常见 key 兜底，非 JSON 用原始串）：短参数（≤60 字符单行）内联，长/多行参数保留自带换行以 `↳` 续行（最多 3 行 × 100 字符，超限 `…` 标记）
- 超 20 条省略最早条目；回复正文 28k 截断（飞书卡片约 30KB 上限）

`tool_trace: false` 时轨迹整体省略（morph/flush 只发正文）。

## 6. receipts 与门禁 reaction

- 消息门禁（hub `gate_message`）统一发放 reaction（best-effort，失败仅 warn）：通过访问控制且被 @（或无需 @）的消息打 ack（Feishu `OneSecond` / Telegram 👀）；allowlist 未命中（`allowed_chats`/`allowed_users` 之外）且被 @ 的消息打 🙏 婉拒（Feishu `THANKS`）；blocklist 命中、通道禁用、未 @ 的群消息一律静默。**结算时不发送任何 reaction**。
- receipts = 逐 run 记录的用户消息 ID（消息处理循环在 Steer/Queue/None 路由时记录，命令不记）；唯一用途是 **mid-run 判定**——仅当 session 正在运行时到达的消息才记录（空闲时到达的是触发消息），故任何 receipt 即代表 run 期间用户发了消息；任何结算路径（Stopped/Timeout/sweep）结束时清空。

## 7. watchdog（超时兜底）

forwarder 每 60s 扫一次：对有回复缓冲的 session 查 `conductor.is_running`（agent task 存活且非 Idle），失活即按 Timeout 走 `deliver_reply`（先查路由/实例再取缓冲，避免查询失败丢内容）。基于**活性查询**而非事件间隔——长工具调用期间 session 处于 `ExecutingTool`，不会误判。已知竞态：`Stopped` 已入队未处理时可能提前一拍按超时结算（真实 `Stopped` 到达后幂等 no-op；卡面可能残留 ⏰ 提示）。

## 8. 平台回退与防护

- **无卡平台（Telegram）**：进度用 `ModelEvent::Request → send_typing`（无卡或 observability 关闭时）；回复 `send_message` 纯文本（轨迹以纯文本行附后）；单条 4000 字符截断（Telegram 上限 4096，unicode 安全）。
- **事件洪水**：forwarder 订阅用 `subscribe_all_filtered` 过滤 `ToolCallDelta`——大文件写入等场景可产生上千个参数 delta，打满 listener 的 256 缓冲（总线 try_send 满即丢）会静默丢弃文本 `Chunk`/`End`。
- **PATCH 顺序与容错**：forwarder 单任务顺序处理、内联 await（同 session PATCH 严格有序）；失败仅 `warn!`（全量替换语义，下次 PATCH 自愈），绝不影响 agent 主流程。

## 9. 配置（ChannelConfig）

| 字段 | 默认 | 说明 |
|------|------|------|
| `observability` | `true` | 状态卡 + receipts；关闭后退回"ack reaction + 缓冲到 run 结束发一条气泡" |
| `tool_trace` | `true` | 最终回复附运行轨迹（折叠面板 / 纯文本行） |
| `mid_run_split` | `true` | run 期间用户发消息时：状态卡原地冻结为终态凭据，回复卡（含轨迹面板）沉底发新消息；关闭后总是原地 morph（一 run 一消息） |

## 10. 已知代价

- PATCH 不触发新消息推送——完成无通知（纯被动展示）。
- morph 的答案位置在 run 起点：无 mid-run 消息时卡本来就是会话最后一条，无感；有 mid-run 消息时已通过"冻结 + 沉底"解决。
- 进程重启丢失内存态（卡片停在最后状态）。
