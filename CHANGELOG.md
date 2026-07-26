# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## 编写要求

- **写给用户**：只记录使用者能感知的变化（行为、配置、命令、界面表现）；不写内部实现（数据结构、锁、trait、重构过程本身）。
- **一条一行一句话**：说清"什么变了、对用户意味着什么"；写不下就拆成多条，不堆从句。
- **不写代码符号**：避免函数名、字段名、文件名和内部术语（buffer、游标、PATCH 等）；配置项与命令名除外。
- **分类**：`Added` 新能力 / `Changed` 行为变化 / `Fixed` 问题修复 / `Removed` 移除能力。
- **配置与命令必须点名**：新增或变更配置项、命令时，写出名称与默认值。

## [Unreleased]

## [0.7.5] - 2026-07-25

### Changed
- 运行状态指示全新设计（GUI）：会话列表的活动标记改为"正在输入"三点动画；聊天页底部的流式状态去掉扫线动画，改为状态词上的流光效果。
- 流式状态直接显示正在做什么（GUI）：编辑哪个文件、运行哪条命令、搜索什么关键词，并附带本次运行的已用时间。
- TUI 底部状态栏与 GUI 对齐：斜体流光状态词（Thinking / Writing / Running 等），去掉旋转图标与工具目标；耗时按整秒显示，不再跳小数。
- 工具名称统一显示为连写格式（如 WebSearch、PostMessage），覆盖工具卡片、权限请求条与子代理活动日志（GUI 与 TUI）。

### Fixed
- GUI 流式状态的已用时间不再沿用上一次运行的计时，每轮运行从零开始。
- GUI 连续调用同类工具（如连续搜索）时，流式状态词不再因重建而闪烁。

## [0.7.4] - 2026-07-25

### Changed
- 收紧子代理（sub-agent）能力：不再提供 cron、goal、ask_user 工具，不再注入通道回复协议；避免子代理创建定时任务、被 goal 续跑带偏任务、或绕过主会话直接向用户提问（疑问改由父代理转达）。

## [0.7.3] - 2026-07-25

### Added
- 通道（Telegram / 飞书）会话支持回复附件：agent 在回复中以 `<yomi_attachments>` 块声明文件，随回复自动发送；相对路径限制在会话工作区内，失败会在回复中注明。
- GUI 新增主题系统（Config → Themes）：5 套内置主题（Zed One、GitHub、Solarized、Nord、Dracula），可克隆为多个自定义主题并以 JSON 编辑、实时预览。
- GUI 工具结果与用户消息中的图片渲染为缩略图，点击全屏预览（Esc / 点击背景关闭）。
- TUI 折叠的 edit 工具改为预览紧凑 diff（最多 10 行，附展开提示）。
- 运行中排队的消息支持 steer：输入框为空时按 Enter 将排队消息转为立即插话（TUI 与 GUI）；TUI 中 Up / Esc 可将排队消息召回输入框编辑。

### Changed
- todo 工具改为实验特性且默认关闭：`[features] todo_tool`（默认继承 `all`，即 `false`）；关闭时 todo 提醒一并停用。
- GUI 运行中的活动区改为整个 run 期间保持展开，修复流式输出中途的折叠/展开闪烁。

### Fixed
- 飞书发送文件修复平台报错 234001；空文件与超限文件在发送前即报出具体原因；单个文件失败不再阻断其余文件。
- Telegram 发送文件保留原始文件名。

## [0.7.2] - 2026-07-25

### Fixed
- `reply_in_thread` 开启时，频道级 @ 不再附带频道闲聊记录（跨话题噪音）；话题内 @ 仍附带本话题记录。
- @bot 时注入的最近聊天记录不再包含 bot 已经处理过的消息（命令、触发消息、bot 自己创建的话题首条）。

## [0.7.1] - 2026-07-25

### Added
- 飞书群聊 @bot 时自动附带最近聊天记录作为上下文（`history_context`，默认 20 条，0 关闭）；私聊与命令不触发。
- 新增 `/help` 通道命令。
- 状态卡更有"个性"：开卡占位词、实时运行轨迹（工具与回复过程逐条展示）、thinking/typing 标题随机换词、统计行显示 steps 与 ctx 用量；最终回复附完整轨迹，run 中途有用户消息时回复以纯文本沉底。

### Fixed
- cron：任务可能被重复执行两次；操作失败会遗留闲置 session；手动 trigger 会误耗 max_runs 配额。
- 多处长文本截断 bug：CJK / emoji 内容截断错误导致发送失败，极端情况 panic。
- 事件总线：订阅后可能错过即时消息的唤醒；关闭后订阅者永久挂起。
- 命令前缀误匹配：`/clearance` 会被当成 `/clear` 执行。

### Changed
- edit 工具不再要求先读后改、不再因文件被外部碰过而拒绝编辑；只要 `old_str` 能匹配当前内容就生效。
- edit / append 不再能把"从未读过的文件"标记为已读，无法再借此绕过 write 的覆写校验。

## [0.7.0] - 2026-07-24

### Added
- 状态卡实时显示当前工具与正在生成的文本尾部。
- 最终回复附运行轨迹（工具调用与中间过程）：Feishu 为可折叠面板（需客户端 V7.9+），其他平台为纯文本。新增 `tool_trace` 开关（默认开启）。

### Changed
- 一次 run 只发一条消息：状态卡原地变为最终回复；run 中途用户发了新消息时，卡片冻结为凭据、回复另发一条沉底，避免答案插到用户消息之上。
- 状态卡出现更早：模型一开始输出（文本或 thinking）即显示，不再等首个工具。

### Removed
- 结算时不再发送 reaction（收到消息的 `OneSecond` 确认保留）。

### Fixed
- 大文件写入等场景下回复文本可能丢失。
- Telegram 超长消息整条发送失败：现在超长会截断并附标记。

## [0.6.22] - 2026-07-24

### Added
- 超长工具参数（如大文件写入）现在会周期性输出摘要日志，便于观测。

## [0.6.21] - 2026-07-24

### Added
- 新增 channel 运行状态卡（`observability` 开关，默认开启）：任务运行时以紧凑卡片实时展示阶段（Thinking/Typing/工具/重试等）、耗时、工具数与 token 用量，结束后变为终态（成功/失败/停止/超时）；不支持卡片的平台退回 typing 指示。

## [0.6.20] - 2026-07-23

### Fixed
- `/info` 的 Subagents 计数只算仍在运行的，不再包含已结束的。

## [0.6.19] - 2026-07-23

### Changed
- 后台 shell 任务完成通知直接附带输出内容（截断），不用再手动翻日志文件。

### Fixed
- GUI steer 长消息的展开/收起按钮不再被文本遮挡。

## [0.6.17] - 2026-07-22

### Changed
- `list_messages` 的消息结构简化：工具消息自包含（名称/参数/结果），前端不再需要按 id 配对。

### Fixed
- GUI 流式期间 activity 分组的工具统计与展开内容不一致；历史加载时偶发 `args.replace is not a function` 崩溃。

## [0.6.16] - 2026-07-22

### Fixed
- GUI `post_message` 工具图标显示错误（匹配条件未同步 snake_case 命名）。

## [0.6.15] - 2026-07-22

### Changed
- 工具名统一为 snake_case（`web_search`、`web_fetch`、`ask_user`、`post_message`、`task_*` 等）；展示层统一显示为 `WebSearch` 风格，历史会话不受影响。

## [0.6.14] - 2026-07-22

### Fixed
- 中文/混排词内的下划线不再被 markdown 误渲染为斜体（如 `变量_名`）。

## [0.6.13] - 2026-07-22

### Fixed
- GUI 聊天中 `finish_reason` 等 snake_case 标识符不再被误渲染为斜体（此前会斜体到段落结尾）。

## [0.6.12] - 2026-07-22

### Added
- 新增 `/info` 命令：查看当前 session 的模型、状态、权限、subagent 与后台 shell 信息。
- 支持 `repeat` finish_reason（如 Kimi 的重复检测停止），按正常结束处理。

## [0.6.11] - 2026-07-22

### Added
- `reply_in_thread` 群聊中，`/model <key>` 在群里发送可切换全群所有话题的模型（新话题自动继承）；话题内发送仍只切换当前话题。

## [0.6.10] - 2026-07-22

### Fixed
- `reply_in_thread` 群聊中各话题现在拥有真正独立的 session：此前顶层消息上下文互相污染、话题内追问反而没有上下文。

## [0.6.9] - 2026-07-22

### Fixed
- channel 会话及其子会话不再提供 ask_user 工具：此前模型调用后会空等到超时。

## [0.6.6] - 2026-07-20

### Added
- Assistant 消息记录实际使用的模型 id，并在 `list_messages` API 中暴露。

## [0.6.5] - 2026-07-20

### Added
- GUI 流式状态实时显示估算 token 数。

### Fixed
- 会话标题生成可能因 token 预算过低（64）而失败，提高到 1000。

## [0.6.4] - 2026-07-20

### Fixed
- token 用量不再重复记录（OpenAI 兼容流重复携带 usage 导致一次调用写 2~3 条）。

## [0.6.3] - 2026-07-19

### Added
- Requests 表格恢复 Type 列，按类型着色（normal / subagent / compactor）。

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
