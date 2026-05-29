# Yomi GUI 执行计划

> 基于 [gui-design.md](./gui-design.md) 的详细实施路线图。目标：从零到可发布的跨平台 GUI，5 周内完成 MVP。

---

## 项目总览

**产出物**：`crates/gui/` — 独立的 Tauri v2 + Svelte 5 + Tailwind v4 项目，连接现有 `yomi-kernel` daemon。

**核心约束**：
- `crates/kernel/` **零修改**
- `crates/cli/`、`crates/tui/` **零修改**
- 所有新增代码限制在 `crates/gui/` 内

**成功标准**：桌面端（macOS/Linux/Windows）可用，代码结构清晰，Phase 4 结束后可进入移动适配（iOS/Android）。

---

## 里程碑时间线

```
Week 1    Week 2    Week 3    Week 4    Week 5
|---------|---------|---------|---------|---------|
Phase 1   Phase 1   Phase 2   Phase 3   Phase 4
Scaffold  Chat      Explorer  Terminal  Polish
+ Bridge  + Sidebar + Editor  + Diff    + CI
```

---

## Phase 1: Scaffold & Core Bridge（Week 1–2）

**目标**：项目能跑起来，能创建 session，能发消息，能看到流式回复。

### Week 1 Day 1–2: 项目脚手架

**任务清单**:
- [ ] 修改根目录 `Cargo.toml`，添加 `crates/gui/src-tauri` 到 `workspace.members`
- [ ] `cd crates/gui && npm create tauri-app@latest` — 选择 **Svelte / TypeScript / Vite**
- [ ] 升级 Svelte 到 v5：`npm install svelte@^5.0.0`
- [ ] 升级 TailwindCSS 到 v4：`npm install tailwindcss@latest` + 配置 `app.css` 入口
- [ ] 初始化 shadcn-svelte：`npx shadcn-svelte@latest init`
- [ ] 安装第一阶段所需 shadcn 组件：`button`, `textarea`, `scroll-area`, `sheet`, `alert-dialog`, `badge`
- [ ] 验证：`cargo tauri dev` 能启动空白窗口

**阻塞后续**：✅ 是，此步不做完无法开工前端

### Week 1 Day 3–4: Rust 后端 — RemoteCoordinator 连接

**任务清单**:
- [ ] `crates/gui/src-tauri/Cargo.toml` 添加 `yomi-kernel = { path = "../../kernel" }`
- [ ] 创建 `crates/gui/src-tauri/src/state.rs`:
  ```rust
  pub struct AppState {
      pub coordinator: Arc<Mutex<RemoteCoordinator>>,
  }
  ```
- [ ] 创建 `crates/gui/src-tauri/src/commands/session.rs`:
  - `list_sessions` — 薄包装 `CoordinatorApi::list_sessions`
  - `create_session` — 包装 `CoordinatorApi::create_session`
  - `restore_session` — 包装 `CoordinatorApi::restore_session`
- [ ] 创建 `crates/gui/src-tauri/src/commands/chat.rs`:
  - `send_message` — 包装 `CoordinatorApi::send_message`
  - `subscribe` / `unsubscribe` — 包装 `CoordinatorApi::subscribe` / `unsubscribe`
- [ ] `main.rs` 中 `tauri::Builder::default().manage(AppState::new(...))`
- [ ] 测试：Tauri 命令能成功调用 kernel daemon（先启动 `cargo run --bin yomi` 确保 daemon 在跑）

**关键决策**：
- `RemoteCoordinator` 的 `connect` 是 lazy 的，第一次 API 调用才会触发连接
- `Arc<Mutex<>>` 是必须的，因为 Tauri 命令是并发处理的

### Week 1 Day 5: Event Bridge（Rust → Frontend Push）

**任务清单**:
- [ ] 在 `main.rs` 的 `setup` hook 中启动事件转发任务：
  - `tokio::spawn` 循环 `recv_event().await`
  - 通过 `app_handle.emit("kernel:event", payload)` 推送到前端
- [ ] 定义 `KernelEventPayload` 序列化结构（sessionId + event）
- [ ] 前端 `lib/api.ts` 中注册全局 listener：
  ```typescript
  listen('kernel:event', (e) => handleEvent(e.payload))
  ```
- [ ] 验证：发送消息后能在前端 console 看到 `model_chunk` 事件

### Week 2 Day 1–3: Chat 页面

**任务清单**:
- [ ] `lib/state.svelte.ts` — 全局 reactive state（`appState`, `sessions: Map`, `activeSessionId`）
- [ ] `routes/chat/+page.svelte` — 布局 shell
- [ ] `chat/MessageList.svelte` — 遍历 `activeSession.messages`，渲染 `UserBubble` / `AssistantBubble`
- [ ] `chat/UserBubble.svelte` + `chat/AssistantBubble.svelte` — 基础气泡样式
- [ ] `chat/ChatInput.svelte` — shadcn `Textarea` + 发送按钮，回车提交
- [ ] `chat/ToolCard.svelte` — 可折叠的工具调用展示（shadcn `Collapsible`）
- [ ] 集成测试：完整对话流程（用户输入 → 流式输出 → 工具调用 → 完成）

### Week 2 Day 4: Sidebar Session Band + 响应式布局

**任务清单**:
- [ ] `layout/SessionBand.svelte` — 竖向 session 列表，点击切换
- [ ] `layout/Layout.svelte` — 响应式壳：Desktop 侧边栏 + 主区域，Mobile 底部导航
- [ ] 移动端适配：`<md` 时底部导航栏 + 全屏内容区
- [ ] 暗色模式：系统偏好检测 + `.dark` class 切换

### Week 2 Day 5: Checkpoint 基础 + 验收

**任务清单**:
- [ ] `chat/CheckpointTimeline.svelte` — 时间线展示（简化版，只读）
- [ ] 端到端测试：创建 session → 发送消息 → 看到 checkpoint → 点击 checkpoint 触发 rewind
- [ ] 修复 Week 1–2 累积 bug

**Phase 1 验收标准**:
1. `cargo tauri dev` 启动 GUI，自动连接本地 kernel daemon
2. 可以创建 session，发送消息，看到流式 assistant 回复
3. 可以切换 session（Sidebar SessionBand）
4. 可以看到工具调用卡片
5. 暗色模式工作正常
6. Mobile 布局无重叠、可点击

---

## Phase 2: Explorer, Editor & Sidebar Session Switcher（Week 3）

**目标**：能浏览文件树、预览代码、手动编辑保存，Session 管理完善。

### Week 3 Day 1: FileSystemProvider + Explorer

**任务清单**:
- [ ] `lib/fs/provider.ts` — 定义 `FileSystemProvider` interface
- [ ] `lib/fs/localProvider.ts` — 用 `@tauri-apps/api/fs` 实现
  - `listDir`, `readFile`, `writeFile`, `stat`
- [ ] `lib/fs/factory.ts` — `createFSProvider()`（现在只返回 `LocalFSProvider`）
- [ ] `explorer/FileTree.svelte` — 递归目录树（shadcn `Collapsible`）
- [ ] `explorer/FileTreeItem.svelte` — 单文件/文件夹行（图标 + 文件名）
- [ ] `.gitignore` 变灰处理：读取 `.gitignore` → 过滤显示

### Week 3 Day 2: File Preview + Shiki 集成

**任务清单**:
- [ ] `editor/FilePreview.svelte` — 只读代码查看
- [ ] 集成 Shiki：`lib/editor/highlight.ts` — `async function highlight(code, lang, theme)`
- [ ] 文件类型检测（扩展名 → language id）
- [ ] Markdown 文件：用 `MarkdownRenderer` 渲染而非 Shiki
- [ ] 图片文件：用 Tauri `convertFileSrc` 转为 `asset://` URL 显示
- [ ] Breadcrumb 导航条

### Week 3 Day 3: CodeMirror 6 编辑器

**任务清单**:
- [ ] `editor/FileEditor.svelte` — CodeMirror 6 嵌入
- [ ] `lib/editor/cmSetup.ts` — 初始化配置 + 主题切换（dark/light）
- [ ] `lib/editor/languageMap.ts` — 扩展名 → `@codemirror/lang-*` 动态 import
- [ ] 编辑器状态栏：行/列、语言模式、dirty indicator
- [ ] `Ctrl+S` → `LocalFSProvider.writeFile()` → 清除 dirty 标记
- [ ] 关闭前未保存 → shadcn `AlertDialog` 确认

### Week 3 Day 4: Tab Bar + 主区域 Tab 管理

**任务清单**:
- [ ] `layout/TabBar.svelte` — 主区域 tab 栏
  - Chat tab 始终 pinned，不可关闭
  - 文件预览/编辑 tab 动态开闭
  - tab 显示文件名 + 关闭按钮
- [ ] 点击 Explorer 文件 → 打开预览 tab
  - 双击 / "Edit" 按钮 → 转为编辑 tab
- [ ] Tab 状态存入 `activeSession.tabs`（session 级别，切换 session 时恢复）

### Week 3 Day 5: SessionBand 完善 + 移动端适配

**任务清单**:
- [ ] SessionBand 添加未读 badge、streaming 动画 dot
- [ ] Session 右键菜单：Fork, Rename, Export, Delete
- [ ] Mobile 底部导航：Chat / Files / Settings（Terminal 留到 Phase 3）
- [ ] Files tab（Mobile）→ 全屏文件树 + 预览
- [ ] Bug 修复 + Phase 2 验收

**Phase 2 验收标准**:
1. Explorer 能正确展示项目文件树，`.gitignore` 文件变灰
2. 点击文件打开预览 tab，Shiki 语法高亮正确
3. 可以进入编辑模式，修改保存后文件立即落盘
4. 切换 session 时 explorer 自动刷新到新的 `projectPath`
5. Mobile：底部 nav 切换流畅，文件树可滚动、可点击

---

## Phase 3: Terminal & Diff Preview（Week 4）

**目标**：能开真终端、能跑 AI 建议的命令、能在文件修改前看到精确 diff。

### Week 4 Day 1: Rust PTY 后端

**任务清单**:
- [ ] `src-tauri/Cargo.toml` 添加 `portable-pty = "0.8"`
- [ ] `terminal/session.rs` — `TerminalSession` struct（portable-pty wrapper）
- [ ] `terminal/manager.rs` — 多 tab 管理（`HashMap<String, TerminalSession>`）
- [ ] `commands/terminal.rs` — Tauri commands：
  - `terminal_spawn(id, cwd, cols, rows)`
  - `terminal_write(id, data)`
  - `terminal_resize(id, cols, rows)`
  - `terminal_kill(id)`
- [ ] 事件转发：`pty read loop` → `app_handle.emit("terminal:data", ...)`

### Week 4 Day 2: 前端 Terminal 面板

**任务清单**:
- [ ] `terminal/TerminalPanel.svelte` — 底部可拖拽面板
- [ ] `terminal/TerminalTab.svelte` — xterm.js 实例
  - `new Terminal()` + `FitAddon`
  - `term.onData` → `invoke('terminal_write')`
  - `listen('terminal:data')` → `term.write()`
- [ ] `terminal/TerminalTabBar.svelte` — 多 tab 切换
- [ ] 拖拽调整面板高度（min 100px，max 60vh）
- [ ] 面板折叠/展开按钮

### Week 4 Day 3: Terminal-AI 集成

**任务清单**:
- [ ] AI 建议 `cargo test` → Chat 显示 "Run in Terminal" 按钮
- [ ] 按钮点击 → 命令写入 active terminal tab
- [ ] 终端内文字选中 → 右键 "Explain" → 发送到 Chat 作为新消息
- [ ] Terminal 工作目录跟随 active session 的 `projectPath`

### Week 4 Day 4: Diff Preview v1（Unified View）

**任务清单**:
- [ ] `npm install diff-match-patch fast-diff`
- [ ] `lib/diff/types.ts` — `FileDiff`, `Hunk`, `DiffLine`, `IntraSegment`
- [ ] `lib/diff/engine.ts` — `computeFileDiff(old, new)`:
  - `fast-diff` 做行级 diff
  -  grouping 成 hunks（context: 3 lines）
- [ ] `diff/DiffPreview.svelte` — shadcn `Dialog`（desktop）/ `Sheet`（mobile）
- [ ] `diff/UnifiedView.svelte` — 单栏 diff（红/绿/context）
- [ ] `diff/DiffHunk.svelte` — hunk header + checkbox（全选/不选）
- [ ] `diff/DiffLine.svelte` — 行号 gutter + 内容

### Week 4 Day 5: Diff Preview v2（Split View + 集成权限系统）

**任务清单**:
- [ ] `diff/SplitView.svelte` — 左右双栏（Old / New），同步滚动
- [ ] `diff/IntraLineDiff.svelte` — `diff-match-patch` 字级高亮
- [ ] Diff 与权限系统打通：
  - `PreToolUse`（write/edit）→ 读取原文件 → 计算 diff → 弹窗
  - `Accept Selected` → 过滤未勾选 hunk → 作为 `updated_input` 发送
- [ ] Keyboard shortcuts：`j`/`k` 导航 hunk，`Space` 切换，`y` 接受，`n` 拒绝

**Phase 3 验收标准**:
1. 能打开真 bash/zsh，运行 `cargo build`，看到颜色输出
2. AI 建议的命令可以一键写入终端执行
3. `write`/`edit` 工具调用前弹出 diff 预览窗口
4. Unified/Split 双视图可切换
5. 可以取消勾选部分 hunk，只应用选中的修改
6. diff 渲染性能：1000 行文件 diff 打开 < 500ms

---

## Phase 4: Checkpoints, Skills & Polish（Week 5）

**目标**：补完剩余功能，打磨体验，准备发布。

### Week 5 Day 1: Checkpoints + Rewind

**任务清单**:
- [ ] `CheckpointTimeline.svelte` 升级：支持点击 rewind
- [ ] Rewind 确认对话框（shadcn `AlertDialog`）
- [ ] Rewind 后刷新 Chat 消息列表、Explorer 文件状态
- [ ] 测试：rewind 到早期 checkpoint → 文件系统回滚 → 继续对话

### Week 5 Day 2: Skill Manager + Settings

**任务清单**:
- [ ] `routes/skills/+page.svelte` — 已加载 skill 列表展示
- [ ] `routes/settings/+page.svelte` — 连接地址、主题、字体大小、auto-scroll
- [ ] Settings 持久化：Tauri `store` plugin（`~/.config/yomi-gui/settings.json`）
- [ ] 启动时读取 settings 应用主题/字体

### Week 5 Day 3: 打磨（Copy、Search、DND、通知）

**任务清单**:
- [ ] 代码块/终端输出/ diff 行：hover 显示 copy 按钮
- [ ] Diff preview 内 `Ctrl+F` 搜索
- [ ] 拖拽文件到 ChatInput → 转为 `image`/`file` content block
- [ ] Desktop notifications：Tauri `notification` plugin（AI 完成/错误时推送）
- [ ] Toast 提示：shadcn `Sonner`（操作成功/失败反馈）

### Week 5 Day 4: 性能优化 + Bug Bash

**任务清单**:
- [ ] 长 session（>500 条消息）虚拟滚动优化
- [ ] Shiki highlight 缓存（相同 content hash 直接复用）
- [ ] Explorer 大目录（>1000 文件）懒加载/分页
- [ ] 全面测试：create/restore/fork/delete session
- [ ] 全面测试：file edit → save → kernel read 看到新内容
- [ ] 全面测试：terminal + AI 建议命令集成

### Week 5 Day 5: CI / Build

**任务清单**:
- [ ] GitHub Actions workflow：
  - `cargo clippy --all-targets`
  - `cargo test`（kernel crate 测试 + gui Rust 测试）
  - `npm run lint` + `npm run check`（Svelte/TS）
- [ ] Release build：
  - macOS (universal binary)
  - Linux (AppImage + deb)
  - Windows (MSI)
- [ ] 版本 tag `v0.2.0-gui`（kernel/cli/tui 保持原版本）

**Phase 4 验收标准**:
1. Checkpoint rewind 工作正常，文件系统正确回滚
2. Settings 持久化，重启后恢复
3. 代码块/终端/ diff 均可一键复制
4. Desktop notification 在 AI 完成时弹出
5. CI 绿，三个平台 release build 成功
6. 无明显 crash，无内存泄漏（xterm.js dispose 正确）

---

## 附录 A：依赖关系图

```
Phase 1 (Scaffold & Bridge)
├── Tauri project scaffold
├── RemoteCoordinator connection
├── Event bridge (Rust → FE)
└── Chat page (MessageList + Input)
    └── Phase 2
        ├── FileSystemProvider (LocalFSProvider)
        ├── Explorer (FileTree)
        ├── FilePreview (Shiki)
        ├── FileEditor (CodeMirror 6)
        └── TabBar (main area tabs)
            └── Phase 3
                ├── Terminal PTY (Rust)
                ├── TerminalPanel (xterm.js)
                ├── Diff engine (fast-diff + diff-match-patch)
                ├── DiffPreview (Unified/Split)
                └── Permission integration
                    └── Phase 4
                        ├── Checkpoint rewind
                        ├── Settings persistence
                        ├── Copy/Search/DND polish
                        └── CI / Release builds
```

**跨 Phase 阻塞点**：
- `FileSystemProvider` 必须在 **Phase 2 Day 1** 完成，否则 Explorer 和 Editor 无法工作
- `Diff engine` 必须在 **Phase 3 Day 4** 完成，否则 Permission-Diff 集成无法测试
- `Event bridge` 必须在 **Phase 1 Day 5** 完成，否则所有实时功能（chat streaming、terminal output、checkpoint updates）都无法工作

---

## 附录 B：每日开工检查表

每个工作日开始时，按此顺序确认：

1. [ ] Kernel daemon 已启动（`cargo run --bin yomi` 或已有 daemon 在跑）
2. [ ] `cargo tauri dev` 能正常启动 GUI
3. [ ] 前端 `npm run dev`（或 tauri dev 自带）热重载正常
4. [ ] 当前分支：`feat/gui-phase-{N}`
5. [ ] 昨日未提交的改动已 commit / stash

---

## 附录 C：风险与对策

| 风险 | 概率 | 影响 | 对策 |
|---|---|---|---|
| `RemoteCoordinator` 与 Tauri 的 async runtime 冲突 | 低 | 高 | 提前在 Phase 1 Day 3 做 PoC，若有问题用 `tokio::sync::mpsc` 隔离 |
| CodeMirror 6 + Svelte 5 runes  reactive 冲突 | 中 | 中 | 用 `$effect` 同步 CM6 state ↔ Svelte state，不直接 bind |
| xterm.js + Tauri v2 mobile 不兼容 | 中 | 中 | Phase 4 再处理移动端 terminal，Desktop 优先 |
| Shiki 大文件高亮卡顿 | 中 | 低 | 限制 preview 文件大小（>1MB  fallback 为纯文本），加 async loading |
| portable-pty Windows 权限问题 | 中 | 低 | 用 `powershell.exe` / `cmd.exe` 替代 bash，测试阶段覆盖 |
| diff-match-patch 大文件性能差 | 低 | 中 | 行级 diff 用 `fast-diff`，只对小 hunk 做字级 diff |

---

## 附录 D：测试策略

**手动测试矩阵**（每个 Phase 结束时执行）：

| 场景 | Desktop | Mobile |
|---|---|---|
| 创建 session + 发送消息 | ✅ | ✅ |
| 流式输出不卡顿 | ✅ | ✅ |
| 切换 session | ✅ | ✅ |
| 文件浏览（>100 文件目录）| ✅ | N/A |
| 文件编辑 + 保存 | ✅ | ⚠️（触屏键盘体验）|
| Terminal `cargo build` | ✅ | N/A |
| Diff Preview + 部分应用 | ✅ | ✅ |
| Checkpoint rewind | ✅ | ✅ |
| 暗色模式切换 | ✅ | ✅ |
| 离线后自动重连 | ✅ | ✅ |

**自动化测试**：
- Rust：`cargo test`（如果 kernel 已有测试，确保仍通过）
- 前端：暂不强制要求 E2E（Tauri 测试较重），但鼓励对 `lib/diff/engine.ts` 写单元测试

---

## 附录 E：与 kernel 的边界约定

GUI 和 kernel daemon 的唯一交互点：

| 方向 | 方式 | 内容 |
|---|---|---|
| GUI → Kernel | `CoordinatorApi` trait 方法 | `create_session`, `send_message`, `subscribe`, `get_checkpoints`, etc. |
| Kernel → GUI | Tauri `Emitter` (broadcast `Event`) | `model_chunk`, `tool_start`, `permission_request`, `error`, etc. |

**禁止事项**：
- GUI 前端**不直接**访问 kernel 的 SQLite 数据库
- GUI Rust 层**不直接**调用 kernel 的内部模块（如 `app::coordinator`）
- 所有数据交换必须通过 `CoordinatorApi` + `Event` 两个接口

---

*计划版本: 1.0*
*最后更新: 2026-05-23*
