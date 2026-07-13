# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
