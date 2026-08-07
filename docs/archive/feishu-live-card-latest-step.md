# Feishu 状态卡精简：只显最新 step + 模型名

## 1. 背景

运行中状态卡目前把 run trace 的最近 10 条（`STATUS_TRACE_MAX_ENTRIES`）全部平铺在卡面上，长 run 时卡片越拉越长、快速滚动，与用户"安静、密集、直接"的预期相悖（DESIGN.md：progress 要可见但不刷屏）。

两个改动：

1. **活卡只显示最新一条 step**（最后一个 trace entry），历史不再平铺；历史在结算卡的折叠 trace 面板里完整保留（已有能力，见 feishu-channel-observability §5）。
2. **stats 行显示当前模型**（session 生效的 model key）。

非目标：改动结算卡（morph/freeze）的任何行为；改动 TUI/GUI 渲染；引入新的配置项。

## 2. 现状

活卡渲染在 `obs.rs::render_running`：

```
[phase title]                          ← 标题（轮转 fun title / 工具名 / 阶段文本）
⏱ elapsed · N steps · M tools · <grey>ctx: x / y · out ~z</grey>
────────────────
{trace 最近 ≤10 条}                     ← s.trace.trace_preview_lines(10)
💬 whisper 尾部（灰）                   ← 进行中文本，≤100 字符
```

- trace entry = 工具调用（`✅/❌/⏳ 工具名 · 参数摘要 · 耗时`）或旁白文本（`💬 …`），按时间交错（reply.rs `RunReplyBuffer`）。
- 全量 PATCH 替换、3s 节流；失败下一个 PATCH 自愈。
- 结算时完整 trace 已进入回复卡/冻结卡的 `collapsible_panel`（默认收起）——历史在"静止的卡"上始终可展开查阅。

## 3. 设计

### 3.1 活卡布局（render_running 重写为三段）

```
[phase title]                                                   （不变）
⏱ 2m10s · 5 steps · 8 tools · <grey>k2 · ctx: 45k / 200k · out ~2k</grey>
────────────────
{最新一条 trace entry}            ← 新增：仅一条；无 trace 时整段省略
💬 whisper 尾部（灰）             （不变）
```

- 新增 `RunReplyBuffer::latest_entry_line() -> Option<String>`：取 `entries` 最后一条，复用现有单行渲染（`trace_lines(&entries[last..], true)`）。进行中的工具自然呈现 `⏳`，即"当前 step"。
- 新鲜卡占位（无 trace 无文本时的随机 placeholder）逻辑不变。
- `STATUS_TRACE_MAX_ENTRIES` 删除（不再需要窗口）；`trace_preview_lines` 若无其他调用方则一并删除。
- 不加"··· N earlier"提示行——steps/tools 计数已在 stats 行，历史去向（结算卡折叠面板）是既有约定。

### 3.2 为什么活卡上不放"可展开的历史面板"

直觉方案是活卡也挂一个 `collapsible_panel`（`expanded: false`）装历史。**否决**：

- Feishu 卡 PATCH（`im/v1/messages` patch）是全量替换（feishu.rs `update_card`），客户端按新 JSON 重渲染；面板展开态由 JSON 的 `expanded` 字段表达。
- 活卡每 3s 一次 PATCH，用户展开后面板会在下一个 PATCH 被收回去，基本不可用；平台又不会为折叠/展开发回调，daemon 侧无法记忆展开态。

历史平铺的诉求已由结算卡的折叠面板覆盖（run 结束后随时展开）。实施时留一个 5 分钟人工验证点：若实测客户端在 PATCH 后保留面板展开态，可作为后续增强把历史面板加回活卡（默认收起）。

> **后续变更（已实施又回退）**：活卡曾加回折叠历史面板（最近 20 条），但"展开态被下一帧 PATCH 收回"的取舍实测不可接受，已回退为平铺最近 10 条（`STATUS_TRACE_MAX_ENTRIES`，§2 旧布局）。模型名显示（§3.3）保留。

### 3.3 模型名显示

**渲染**：stats 行灰色技术区段的首位（模型是相对静态的元数据，与时钟/计数同属一行）：

```
⏱ 12s · 3 steps · 2 tools · <grey>nova-2 · ctx: 5k / 200k · out ~1k</grey>
```

`token_footer` 尚未到达（首个响应未结束）时同样带模型：`<grey>nova-2 · out ~500</grey>`；模型未知时省略该段，行为同今。

**数据来源**：事件流不携带模型名（`ModelEvent::Request` 只有 message_id/message_count），但 forwarder 持有 `Weak<Kernel>`。在 `Lifecycle(Running)` 分支：

1. `obs` 尚无该 session 状态（即 run 的首个 Running）且 kernel 可 upgrade 时，`kernel.get_session_model(&sid)`（一次 sqlite 读，每 run 仅一次）；
2. 调新增的 `ObsTracker::set_model(&sid, model)`：状态已存在则直接写字段；不存在则暂存 `pending_models`，`ObsCardState::new` 物化时取走；
3. 随后 `handle_event` 物化卡片，首帧即带模型。

- `ObsCardState` 新增 `model: Option<String>`，随结算随状态一并清除。
- kernel 已退出（upgrade 失败）则静默省略模型段。
- run 中途 `/model` 切换：本 run 卡片仍显示启动时模型，下一个 run 更新——可接受（与 token_footer 的"响应末才更新"语义一致）。
- 备选方案（不采纳）：给 `ModelEvent::Request` 加 `model` 字段——改动核心事件 schema（kernel/GUI/TUI 都消费），收益相同但波及面大。

## 4. 变更点

| 文件 | 改动 |
|------|------|
| `channels/reply.rs` | 新增 `RunReplyBuffer::latest_entry_line()`（后随布局回退移除，恢复 `trace_preview_lines` 平铺最近 10 条） |
| `channels/obs.rs` | `render_running` 保持 stats + 平铺最近 10 条 trace + whisper（"只显最新一步"已回退）；`ObsCardState.model` 字段；`ObsTracker::set_model` + `pending_models`；`stats_line` 渲染模型段 |
| `channels/hub.rs` | forwarder `Running` 分支：首个 Running 查模型并 `set_model` |

## 5. 测试（obs_test.rs / reply 单测）

- 多条 trace entry 时活卡只含最后一条（含进行中的 `⏳` 工具），whisper 仍在；
- 无 trace 时省略 entry 段，placeholder 行为不变；
- 有/无模型时 stats 行渲染（含无 token_footer、无 out estimate 的组合）；
- `set_model` 先于首个 `Running`：物化卡首帧带模型；状态已存在时 `set_model` 更新字段；
- 结算卡（morph/freeze）渲染不变——完整折叠 trace 面板保留（回归保护）。

## 6. 已知代价

- 运行中无法回看较早 step（需等结算卡的折叠面板）。缓解：面板验证点（§3.2）若通过可加回。
