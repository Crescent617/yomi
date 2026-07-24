# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.7.0] - 2026-07-24

### Added
- channel 状态卡运行中新增两行实时信息（≤100 字符截断）：last tool（`🔧 工具名 · 主参数摘要`）与 whisper（`💬` 灰色的当前输出文本尾部，流式累积、新模型调用清空、`End` 用完整文本自愈，3s 节流 PATCH 下呈打字感）。
- 最终回复卡附可折叠运行轨迹面板（Feishu `collapsible_panel`，默认收起，需客户端 V7.9+；无卡平台为纯文本行）：工具成败图标、参数摘要（按工具取主参数；短参数内联，长/多行参数保留自带换行以 `↳` 续行，最多 3 行 × 100 字符）、耗时；agent loop 中间轮次文本降级为面板内灰色旁白。新增 `tool_trace` 配置开关（默认开启）。

### Changed
- channel 回复改为**自适应单/双消息**：run 期间用户未发新消息时，状态卡在 `Lifecycle(Stopped)` 原地 morph 为最终回复卡（无 header，正文 = 最后一个模型文本 + 轨迹面板），一段 run 一条消息；**run 期间用户发了新消息（平台消息，命令除外）时**，状态卡冻结为终态凭据留在上方（终态 header + 统计行），最终回复作为新消息沉底（锚定最新用户消息），避免答案出现在用户消息之上。异常结束（Failed/MaxIterations/watchdog 超时）以正文提示行呈现；结算发送失败或状态缺失时回退为普通消息，不丢内容。
- 状态卡开卡时机由"首个工具"提前到"首个工具**或首个模型输出 chunk（文本或 thinking）**"：模型一开始响应卡片就到位（reasoning 模型长 thinking 期间标题为 `💭 Thinking…`），纯问答 run 也有状态卡且中途输出全程可见。

### Removed
- 结算时不再发送任何 reaction：移除 0.6.21 引入的用户消息 `OneSecond → DONE/CrossMark` 切换与回复卡完成 reaction（`OneSecond` 收到确认保留）；连带删除已无调用方的 `remove_reaction`（trait 默认方法 + Feishu 实现）与 `ChannelMessage.receipt_reaction_id` 字段。

### Fixed
- channel forwarder 订阅改为过滤掉 `ToolCallDelta` 事件：大文件写入等场景会产生成百上千个参数 delta，打满 forwarder 的 256 事件缓冲（总线满即丢），导致文本 chunk/End 被静默丢弃（whisper 为空、回复文本丢失）。
- Telegram 适配器补充消息长度截断（4000 字符上限，unicode 安全 + 截断标记）：此前超过 4096 字符的消息整条发送失败丢失，追加运行轨迹后更易超限。

## [0.6.22] - 2026-07-24

### Added
- 流式工具调用参数摘要日志：单个 tool call 的流式参数每累积约 4k token（4 字节 ≈ 1 token 估算）输出一条 `tracing::warn!` 摘要，含工具名、调用 id、累计 token 估算及参数头/尾各 80 字符片段（unicode 安全、转义为单行），便于观测大文件写入等超长参数流；跟踪状态为单槽设计，新 tool call 的 delta 到达即替换旧状态，首个 delta 为空或缺工具名时由后续 delta 补全。

## [0.6.21] - 2026-07-24

### Added
- channel 运行可观测性（`observability` 配置开关，默认开启）：任务运行期间以一张紧凑卡片（400px 宽、12px 字号、英文文案）呈现进度，首个工具调用时才发卡（纯问答不产卡），标题实时区分 🧠 Thinking / ✍️ Typing / 🔧 工具 / 🔁 重试 / 🗜 压缩 / 🔀 降级等阶段，正文单行汇总耗时、工具总数与 token 用量（1.5s 节流原地更新）；`Lifecycle(Stopped)` 结算为终态（绿=✅ Done / 红=❌ Failed / 灰=⏹ Stopped / 灰=⏰ Timed out，watchdog 每分钟查询 session 活性兜底崩溃/panic/失联，长工具调用不误判），首个工具前失败的运行也会直接补发终态红卡。运行期间收到的用户消息 ack reaction（`OneSecond`）在结算时统一替换为 `DONE`/`CrossMark`，并给最后一条内容回复打上同样 reaction。Feishu 适配器新增 `send_card`/`update_card`/`remove_reaction`，`send_reaction` 修复为透传 emoji 并返回 reaction_id；无卡片能力的平台（Telegram）保留 typing 指示作为进度回退。

### Removed
- 删除从未有发射方的 `ModelEvent::Error` 死事件及其在 hub/TUI 的死消费分支（agent 错误统一走 `AgentEvent::Error`）。

## [0.6.20] - 2026-07-23

### Fixed
- channel `/info` 的 Subagents 计数改为仅统计正在运行的 subagent（标签同步为 `Subagents (running)`），此前统计的是该 session 全部历史 subagent 会话（含已结束），与 Background Shell 的 running-only 语义不一致。

## [0.6.19] - 2026-07-23

### Changed
- 后台 shell 任务完成通知直接携带截断后的输出内容（keep-edges 截断，预算同工具输出上限），无需再手动读取日志文件；日志文件路径并入首行状态行，输出为空时显式标注 `[No output]`。

### Fixed
- GUI steer 消息展开/收起按钮不再被文本遮挡：长消息时文本列为按钮预留底部空间，并移除收起态底部的渐变遮罩。

## [0.6.17] - 2026-07-22

### Changed
- `list_messages` API 的 `AssistantMsg` 不再携带 `tool_calls` 字段：`ToolMsg` 自包含（name/args/result），前端无需再按 `tool_call_id` 配对；TUI 历史加载改由 `ToolMsg` 直接构建工具条目（被中断回合中未执行的 tool call 不再以 running 占位出现）。
- GUI activity group 的 header 统计与展开列表统一为仅基于 thinking 与已物化的 tool 消息，不再展示 pending 工具占位卡；无文本 assistant 消息（纯工具调用/纯 thinking/空）始终归入 activity group，不再拆组或渲染为空气泡。

### Fixed
- GUI 修复流式期间 activity group header 显示 N 个"未知工具"、展开却只有 thinking 的统计/渲染不一致。
- GUI 修复 `args.replace is not a function` 崩溃：历史加载中 `AssistantMsg.tool_calls[].arguments` 以 JSON 对象上线（声明类型为 string），对象透传进 `compactArgs` 触发 TypeError；字段移除后该路径不复存在。

## [0.6.16] - 2026-07-22

### Fixed
- GUI `post_message` 工具图标修复：工具名改 snake_case 后匹配不到 Send 图标分支（旧判断为 `postmessage`），错误回退为 Wrench；判断条件同步改为 snake_case。

## [0.6.15] - 2026-07-22

### Changed
- 工具名统一为 snake_case：`webSearch`→`web_search`、`webFetch`→`web_fetch`、`askUser`→`ask_user`、`postMessage`→`post_message`、`updateGoal`→`update_goal`、`taskCreate/Get/List/Update`→`task_create/get/list/update`，goal/subagent prompt 中的工具名引用同步更新。TUI/GUI 展示层对旧 camelCase 名保持归一化识别，历史会话不受影响。
- TUI/GUI 工具名展示新增 humanize（snake_case→CamelCase）：未知工具 label 兜底及 streaming inline 状态（TUI info bar、GUI InlineStreamStatus）中的工具名统一显示为 `WebSearch` 风格。

## [0.6.14] - 2026-07-22

### Fixed
- 聊天 markdown 下划线转义扩展到 CJK 文本：`变量_名`、`hello_世界` 等中文/混排词内下划线同样不再误渲染为斜体（词内判定从 ASCII 放宽到 Unicode 字母/数字）。

## [0.6.13] - 2026-07-22

### Fixed
- GUI 聊天 markdown 渲染：`finish_reason` 等 snake_case 标识符中的词内下划线不再被误渲染为斜体（此前会斜体到段落结尾）。通过在渲染前转义词内下划线 run 解决，code block、inline code、链接 URL 不受影响，正常 `_斜体_` / `__粗体__` 语法保持可用。

## [0.6.12] - 2026-07-22

### Added
- Channel 新命令 `/info`：查看当前 session 基本信息（ID、模型、运行状态、创建/活跃时间、权限、subagent 数、后台 shell）。thread 内显示 thread 的 session；reply_in_thread 群聊顶层显示 chat 级 session。
- 支持 `repeat` finish_reason（如 Kimi 的重复检测停止）：按正常终止处理，不再产生 unknown 警告，且不会触发 auto-continue。

### Changed
- Channel 命令字符串字面量收敛为单一 `CMD_PREFIXES` 常量表，解析与命令前缀检测共用。

## [0.6.11] - 2026-07-22

### Added
- Feishu `reply_in_thread` 群聊中，在群里（非 thread）发送 `/model <key>` 切换整个群的所有 thread session，且新 thread 自动继承；thread 内发送仍只切换当前 thread。

## [0.6.10] - 2026-07-22

### Fixed
- Feishu `reply_in_thread` 群聊模式下每个话题真正拥有独立 session：thread 由 bot 回复创建，发起消息不带 `thread_id`，此前所有顶层消息汇入同一个 chat 级 session（上下文互相污染），而 thread 内追问反而开出无上下文的新 session。现在利用话题内消息 `root_id` 指向话题根消息的语义，按根消息 id 归并 session。私聊与非 thread 模式行为不变。

## [0.6.9] - 2026-07-22

### Fixed
- Channel（Telegram/Feishu）会话及其 subagent 子会话不再注册 `askUser` 工具：此前 channel 会话创建时传入的 per-session `tool_blocklist` 未被持久化也未生效，模型调用 ask_user 后会空等到 2 分钟超时。现在 Conductor 在 spawn 时通过 channel mapping（沿 parent 链回溯）实时判断并追加 block。

## [0.6.6] - 2026-07-20

### Added
- Assistant 消息持久化 `model_id`：存储实际请求使用的模型 id，随 JSONL 消息历史保存，并通过 `list_messages` API 暴露给前端；旧格式消息无该字段可正常读取。

## [0.6.5] - 2026-07-20

### Added
- GUI inline stream status 实时显示估算 token 数：thinking 流按 ~4 字节/token、tool 参数 delta 按 ~2 字节/token 估算，与 kernel 估算逻辑一致。

### Fixed
- Session title 生成的 max_tokens 从 64 提高到 1000：部分模型即使请求关闭 thinking 仍会输出推理内容，64 的预算被耗尽导致标题生成失败。

## [0.6.4] - 2026-07-20

### Fixed
- Token usage 每个 stream 只记录一次：OpenAI 兼容流可能在 choice 级和顶层 chunk 重复携带 usage，此前每个事件都写库导致单次调用产生 2~3 条重复记录（约 18% 数据冗余）。

## [0.6.3] - 2026-07-19

### Added
- Requests 表格加回 Type 列，徽标按类型着色区分 normal / subagent / compactor。

## [0.6.2] - 2026-07-19

### Added
- Usage 页面底部新增 Requests 记录表格，显示完整 request id、时间、模型、token 用量和 cache 率，支持表格内滚动加载更多。

### Changed
- 环境变量不再覆盖已有 model 配置，检测到 model 相关 env 时自动创建 `from_env` model；`default_model` 未配置时回退到 `models[0]`。
- Cache 率显示统一改为 1 位小数。

## [0.6.0] - 2026-07-19

### Added
- Desktop pet now renders from local Codex Pets V1/V2 spritesheet packs in `~/.yomi/pets`, selectable in Settings; pack format documented in `docs/DESKTOP_PETS.md`.
- Pet window supports status-driven animations, click jump, drag movement, and V2 gaze/ambient look with mixed-DPI handling.
- `svelte-check` added as a frontend type-check gate (`npm run check`).

### Changed
- Auto-triggered compaction routes through `auto_compact`, honoring `micro_compact_enabled`; manual `/compact` and overflow recovery still force full compaction.
- Request budgeting uses a fixed 2000-token estimate per image block.
- Pet window is opaque on Linux (WebKitGTK stale-frame workaround); phaser dependency removed.

### Fixed
- Compaction overflow retries shed tool definitions to leave more room for history.
- Stale compaction threshold test expectation after the remaining-context reserve changed to 25.6k.

## [0.5.28] - 2026-07-18

### Changed
- Compaction now triggers based on remaining context capacity, reserving 33k tokens by default for a 200k context window.
- Shared request token estimation moved into `utils::tokens` and compaction threshold diagnostics were added.

### Fixed
- Avoided triggering compaction prematurely at a fixed 110k-token limit on larger context windows.

## [0.5.27] - 2026-07-17

### Added
- 增加 compactor 的 `micro_compact_enabled` 配置，默认关闭。

### Changed
- 上下文超限时自动裁减最旧对话轮次并重试 full compact。
- 完善 OpenAI Responses API 包装错误解析和上下文溢出恢复。
- 新增 Pet 运行时、后台任务状态恢复和相关 GUI 交互。

### Fixed
- 修复 OpenAI Responses API 返回 `context_length_exceeded` 时被误判为 SSE JSON 解析错误的问题。



### Added
- Session 菜单支持 `Create from`，复制 Project、Model Key 和 Approval Level 创建空白 Session。
- GUI 增加 Session 完成通知中心，支持未读状态、跳转和批量已读。

### Changed
- 纯 Session List 仅显示顶层 Project Session，并移除 Project Dot。
- Session 菜单使用更明确的 Hover / Focus 状态。
- Notification Center 使用更紧凑的单行 Header 和未读 Dot 列表。

### Fixed
- Activity Group Header 在 Agent metadata 尚未到达时也能正确显示 Agent。
- 修复重连后已运行 Session 可能不产生完成通知的问题。
- 修复删除 Project 后可能残留通知，以及跳转失败时通知被提前标记已读的问题。

## [0.5.23] - 2026-07-15

### Added
- Session 侧边栏菜单支持 Fork，并在创建后直接打开新 Session。
- Status Bar 汇总运行中的 Session 与后台 Shell，可快速跳转或复制日志路径。
- Project 标识色统一为主题感知的组件，Thinking 与 Steer 消息提供更清晰的紧凑预览和展开交互。

### Changed
- 普通 Fork Session 作为独立顶层 Session 保存，不再复用 Subagent 的父子关系。
- 后台 Agent 消息发送提示统一要求等待异步结果，避免重复轮询。

### Fixed
- 后台 Shell 完成通知明确区分 completed、failed、cancelled 与 timed_out，并避免重复 Task ID。
- Fork 前端状态会继承权限级别，并在加载失败时清理临时 Session。

## [0.5.20] - 2026-07-14

### Added
- 后台 Shell Task 的 Steer 消息使用 `[From Shell: <task_id>]` 标识来源。
- Status Bar 的后台 Shell 列表支持复制任务日志文件路径。

### Changed
- Agent 消息统一使用 `[From Agent: <session_id>]`，并继续兼容历史 `agent_id` 格式。
- Recent Sessions 标题布局更稳定，长标题会正确截断。

### Fixed
- 修复 Shell 日志复制失败通知的参数顺序，并将成功反馈保留在按钮局部。

## [0.5.19] - 2026-07-14

### Changed
- Steer Message 使用更紧凑的字号，并默认显示两行内容。

### Added
- 解析 Steer Message 开头的 `agent_id`，支持点击跳转到对应 Agent Session。

## [0.5.18] - 2026-07-14

### Added
- Diff 代码按文件语言进行语法高亮，支持 Unified / Split 和亮暗主题。

### Changed
- Diff 文件导航改为横向 Tab，并保留溢出文件列表和前后切换。
- Diff 加载状态统一使用内容区的 Loading Placeholder。
- Query Navigator 使用固定宽度的横向滑入动画。

### Fixed
- 修复切换 Diff 文件后仅首个文件显示语法高亮的问题。

## [0.5.17] - 2026-07-14

### Changed
- 收紧纯 Session 列表中的时间分割线高度。

### Fixed
- 超长单行 User Message 和 Steer Message 现在会在消息区域内自动换行。

## [0.5.16] - 2026-07-14

### Changed
- 为 Session / Project 视图切换增加滑动指示器和过渡动画。
- 所有流式错误至少重试一次；可重试错误继续使用完整重试预算。

### Fixed
- 避免兼容代理返回畸形流式错误时立即终止 Agent。

## [0.5.14] - 2026-07-13

### Changed
- 优化 Session 时间分组标题为紧凑的居中分隔线样式。
- 收紧 Session 操作菜单右侧间距，并隐藏侧边栏列表滚动条。

### Fixed
- 修复时间分组标题吸顶时与列表顶部之间的缝隙。

## [0.5.13] - 2026-07-13

### Added
- Chat 侧边栏新增全部 Session 视图，并按从 30 分钟到更早月份的人性化时间窗口分组。
- `postMessage` 工具调用支持直接跳转到目标 Session。

### Changed
- Session / Project 视图改为图标切换，并持久化侧边栏可见状态、宽度与所选视图。
- Session 状态、未读提示与操作菜单共用紧凑槽位，Pinned 和 Project 列表交互保持一致。

### Fixed
- 修复父 Session 尚未加载时的面包屑导航，并避免切换父会话后残留子会话路径。
- 修复全部 Session 列表重复加载、失败重试与嵌套交互控件问题。

## [0.5.12] - 2026-07-13

### Added
- 新增独立的 User Query Navigator，以紧凑短横线快速浏览并定位历史提问。

### Changed
- Session title 在每条非空用户文本后更新，并结合当前标题与最新意图生成新标题。
- 精简聊天面包屑与部分列表、Toast 布局细节。

### Fixed
- 标题模型关闭 thinking、提高输出额度，并在生成失败时回退到最新用户输入。
- 串行化自动标题与手动改名，避免并发更新相互覆盖。
- 修复 Query Navigator 跳转后被自动滚回最新消息的问题。

## [0.5.11] - 2026-07-13

### Fixed
- 修复 Application Config 保存成功后因克隆 Svelte 状态代理而误报失败的问题。

## [0.5.10] - 2026-07-13

### Added
- 状态栏展示运行中的主会话与子 Agent，并支持快速跳转。
- 为后台完成的会话显示未读提示。

## [0.5.9] - 2026-07-13

### Added
- 配置通用环境变量注入、功能开关和轻量任务模型。
- 自动生成会话标题，并协调后台任务状态。

### Changed
- GUI 配置编辑器与文档结构精简。
- GUI 链接统一使用系统默认应用打开。

### Fixed
- 工具描述和链接打开错误处理。

## [0.5.8] - 2025-07-13

### Added
- `UI config` 支持：应用配置面板，可调整各类设置。
- 自动生成对话标题。

## [0.5.7] - 2025-07-12

### Added
- `postMessage` 增强。
- 文档更新。

## [0.5.6] - 2025-07-12

### Added
- UI 配置面板（`app config panel`）。
- 自动对话标题生成。

## [0.5.5] - 2025-07-10

### Fixed
- 稳定项目侧边栏展开状态。

## [0.5.4] - 2025-07-10

### Added
- 支持 Agent 消息工具（`agent messaging tool`）。
- 在任务面板中展示运行中的子 Agent（`show running subagents in task dock`）。

### Changed
- 工具消息不再清除缓冲区。

## [0.5.3] - 2025-07-09

### Added
- 集成 Serper 搜索（`serper search`）。
- 代码高亮（`code highlight`）。

### Changed
- 通知优化。

## [0.5.2] - 2025-07-08

### Added
- Mermaid 流式渲染。
- 进度条。
- 默认按 channels 分组消息。
- 在 Markdown 中渲染 Mermaid 图表。

### Fixed
- Mermaid 流式闪烁修复。
- 将子 Agent 活动保持在独立组中。

## [0.5.1] - 2025-07-07

### Fixed
- 飞书模型适配。
- 时间戳问题。

## [0.5.0] - 2025-07-07

### Added
- 全新 UI 界面。
- 优化桌面端体验（GUI 精修）。
- 针对 GPT 模型的子 Agent 修复。

## [0.4.8] - 2025-07-06

### Added
- 重启守护进程（`restart daemon`）。
- Toast 增强。

## [0.4.7] - 2025-07-06

### Added
- 新主题（`new theme`）。

## [0.4.6] - 2025-07-05

### Added
- 侧边栏工具按 Action Group 分组（`read-only lookup tools`）。
- Shell Action Group 徽标。
- 首页增强。
- 基于广播的事件订阅与统一关闭（`broadcast-based event subscriptions`）。

## [0.4.5] - 2025-07-05

### Added
- 今日用量（`today usage`）。
- 待办栏（`todo bar`）。

## [0.4.4] - 2025-07-04

### Fixed
- 流式恢复（`resume streaming`）。

## [0.4.3] - 2025-07-04

### Added
- `OpenAI Response` 协议与事件缓冲区合并。
- TUI 模型切换与 GC 命令。

## [0.4.2] - 2025-07-03

### Added
- 模型切换（`model switch`）。

### Fixed
- 重绕刷新（`rewound refresh`）。

## [0.4.1] - 2025-07-03

### Added
- 子 Agent 增强（`enhance subagent`）。
- 工具调用状态（`calling status`）。
- 工具显示优化。
- 更新 Nix flake。

## [0.4.0] - 2025-07-02

### Added
- 支持 Claude 缓存 token 与 effort 设置。
- 新消息 API（`new message api`）。

## [0.3.4] - 2025-06-30

### Fixed
- GUI 错误处理。

### Changed
- 工具执行改为 eager 模式。

## [0.3.3] - 2025-06-30

### Added
- 新消息 API。

## [0.3.2] - 2025-06-29

### Added
- Claude 缓存 token 与 effort 设置。
- 子 Agent 增强。
- 工具显示优化。
- 工具调用状态。

## [0.3.0] - 2025-06-28

### Added
- 消息队列与文件追加。
- 流式滚动优化。
- 会话清理命令。
- `send_message` API 统一使用 `ContentBlock`。
- 项目记忆加载移至 `SystemPromptBuilder`。
- 工具模块重组。
- JSON 解析与文件状态追踪。
- 文件状态 vacuum 与 builder 模式。
- API 响应元数据追踪。
- 基于广播的事件转发与 Agent 生命周期事件。
- 简化 `Coordinator` / `Session` 初始化。
- 用量表（`usage table`）。
- 存储层按功能域重组。
- 用量命令（`usage command`）。
- Token 用量追踪。
- 主题环境变量与对话框布局改进。
- 取消时清除状态。
- 简化 Agent 循环。
- 配置拆分与工具提取。
- 桌面通知（`desktop notify`）。
- 会话状态存储。
- Fork 会话。
- Skill 使用优化与历史优化。
- 配置精修。
- 新 Banner。
- 消息队列与文件追加。
- 流式滚动保持偏移。
- Shell 模式支持。
- 待办列表隐藏（完成时）。
- 新系统提示。
- 提醒与增强子 Agent。
- 会话命令（`sessions command`）。
- SQLite 并发优化。
- 会话 CWD（`session cwd`）。
- 工具 CWD 与 Glob 增强。
- 工具调用状态。
- Anthropic token 用量修复。
- Windows 支持。
- 命令与工具输出截断。
- 纯文本粘贴（小于 1k 时）。
- 会话元数据数据库。
- 单行清理。
- 自动删除空会话。
- 紧凑链逻辑优化。
- 帮助命令。
- 通知优化。
- 历史选择器（`history picker`）。
- 子命令重构。
- 版本命令。
- 文件状态持久化。
- 工具模块重组。
- 升级 tuirealm 4.0。
- 支持 Unix 上 C-z 挂起。

## [0.2.0] - 2025-06-25

### Added
- 输入总线（`input bus`）与子 Agent 重新设计。
- 通道命令（`chan cmd`）。
- 配置工具最大输出。
- 图像使用 asset 协议。
- 自动恢复。
- 背景消息支持（`bg msg support`）。
- Python 客户端脚本。
- 事件总线（`event bus`）。
- 扩展设计文档。
- 重绕增强（`enhance rewind`）。
- 飞书改进与会话分页游标。
- 飞书线程（`feishu thread`）。
- TG Markdown 支持。
- 飞书 Markdown 支持。
- 工作流增强（`enhance wkfl`）。
- 工具展开 tilde (`~`) 到 home 目录。
- 工作流支持（`wkfl`）。
- 频道（`channels`）。
- 目标 MD 精修。
- 侧边栏置顶会话（`pin sessions`）。
- 将 Compacting 提升为一级 `AgentState`。
- 在嵌入式浏览器中打开链接。
- Cron 修复。
- 加载 `~/.env`；GUI IME 修复。
- 用量 RPC 通过 wire 协议。
- 当 `finish_reason` 为空或 max tokens 时自动继续。
- 简化 tracing 与初始化。
- 统一信号处理、守护进程干净关闭、待办格式修复。
- TUI 目标显示增强。
- 工具 blocklist 不区分大小写。
- 目标 TUI 增强。
- GUI 通过 IPC 连接守护进程，统一 cron 通过 `CoordinatorApi`。
- 守护进程支持 cron 参数。
- 内核 bug 修复、死代码移除、子 Agent 清理。
- 网络搜索工具（`websearch`）重构与多引擎支持。
- 环境变量名集中至 `config.rs`。
- 追踪优化。
- Git 差异面板修复。
- Shell 工具新截断逻辑。
- 统一 IPC 协议。
- 状态栏（`status bar`）。
- 差异 UX 修复。
- 信息栏（`infobar`）修复。
- GUI Git 差异侧边栏、Action Group 修复、Shell 取消 token、移除终端。
- Action Group 与编辑工具增强。
- 目标栏 UX。
- 继续与分叉（`continue & fork`）。
- 目标实现（`goal implement`）。
- GUI 操作栏（`operation bar`）。
- 聊天布局重构、Git 信息、Action Groups、bug 修复。
- 背景会话修复。
- 网络搜索传输（`ws transport`）。
- 对话框选择输入修复。
- 询问用户工具（`ask user tool`）增强。
- 子 Agent 预设、Bing 搜索、UX 精修。
- 会话 GC（`sessions gc`）。
- 新选择器样式（`new picker style`）。
- 压缩器配置改为比例（`ratio`）。
- 守护进程增强（`daemon`）。
- 浏览时帮助通知。
- 状态栏清理。
- Banner 性能增强。
- 性能优化（`enhance perf`）。
- 完整消息生命周期追踪（`MessageId`）。
- CLI 支持 stdin。
- 桌面通知改用 OSC 转义序列。
- 工具事件重构。
- 生命周期钩子（`hooks`）可配置，带 feature flag 与改进的 TUI 集成。
- TUI 重绘优化（终端调整大小与光标移动）。
- 目标消息格式改进与反斜杠转义换行输入。
- 目标（`goal`）支持。
- TUI 动画环境切换。
- 待办工具合并。
- 系统提醒（`system reminder`）。
- `@` 搜索文件增强。
- 按模型查询用量。
- TUI 文件补全导航（C-n/C-p/Up/Down）。
- Grep 显示模式增强。
- TUI 输入重构。
- 上下文菜单复制。
- 信息栏通知图标。
- 复制代码按钮。
- 文件锁（`file lock`）。
- Windows 鼠标捕获变通方案；PageUp/PageDown 移至 `ChatViewComponent`。
- 存储层懒初始化（`JsonlStore`）。
- 待办更新工具（`todoUpdate`）与 `ToolExecCtx` 传递 `session_id`。
- 代码质量改进与清理。
- Agent 工具参数显示。

## [0.1.0] - 2025-06-20

### Added
- 初始发布：支持多 Agent 的 CLI / TUI 界面。
- 内核支持 SQLite 持久化、工具调用、网络搜索、文件操作。
- TUI 支持流式响应、Markdown 渲染、代码复制、选择复制。
- 支持多种模型（OpenAI、Anthropic 等）。
- 支持 Skills 加载与自定义工具。
- 支持会话管理、历史查看、配置管理。
- 支持多平台（macOS、Linux、Windows）。
