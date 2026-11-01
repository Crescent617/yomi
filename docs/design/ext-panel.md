# 设计文档：扩展面板（Extensions panel）—— GUI 的插件管理界面

定位：在 GUI 里加一个管理**扩展包**（`extensions/`）的界面——安装 /
清单 / 卸载 / 更新，对齐 `yomi extension` CLI 与
`docs/design/ext-packages.md`。运行时语义（hook 扫描、tool 合并、cron
收养）一行不动：本界面只是生命周期层（install/list/remove）的图形
入口，与 CLI 走同一批 wire 方法。

## 术语：界面叫 Extensions，不叫 Plugins

项目里"扩展"有两层含义，本界面只碰其中一层，命名跟 CLI 一致：

- **扩展包（extension package）** = `extensions/<名>/` + 包外单文件
  注册表 `extensions/ext.lock`。**本界面管理的对象。** CLI 命令
  `yomi extension install|list|remove`。
- **外挂（hooks / tools）** = `hooks/` `tools/` 两个扁平目录，文件系统
  即注册表。**本界面不管**（它们是扩展包落地后的槽位，手工放置的外挂
  同样不经本界面）。契约见 `yomi doc extensions`。

因此界面标签用 **Extensions**（图标 `Blocks`），与 `yomi extension`
同词；不用 "Plugins"（无此术语），不造伞概念（cf. ext.md 决策）。

## 放置：Config 面板的一节

不进一级 ActivityBar（那是 chat/favorites/automation/agents/usage/
debug/config 这种平级大区），而是作为 **Config 面板的一个 section**，
与现有 `Application` / `Themes` / `Kernel` 并列——扩展管理属于"设置
这个 yomi"，不是日常作业区。

改动点：

- `lib/components/layout/ConfigPanel.svelte`：`Section` 联合加
  `"extensions"`；header 的 `actions` 段 nav 加第四个按钮
  （icon `Blocks`，label `Extensions`）；`if/else` 渲染分支加
  `<ExtensionsPanel />`。
- 新组件 `lib/components/extensions/ExtensionsPanel.svelte`（+ 子组件）。
- `lib/state.svelte.ts`：`ActivePanel` 不加新成员（仍在 `"config"`）；
  但 `requestActivePanel("config")` 需要能带一个目标 section——见下
  「导航接入」。

config 面板的 section 切换是组件内局部状态，外部（命令面板）要能直接
跳到 Extensions 节，需把当前 section 提到可寻址处：最简做法是给
`appState` 加一个 `configSection` 字段（默认 `"application"`），
ConfigPanel 读它、命令面板写它，切换 section 时更新。这是为导航引入的
一处小状态，不改其余行为。

## 数据与 wire

复用 kernel 既有方法（`crates/kernel/src/client/mod.rs`）：

- `extension_list() -> Vec<serde_json::Value>`
- `extension_install(source) -> InstallReport`
- `extension_remove(name) -> RemoveReport`

**kernel 微调（采纳，见开放问题 3）**：把 `extension_list` 的返回从
`Vec<serde_json::Value>` 收紧为带
`#[serde(rename_all = "snake_case")]` 的 `ExtensionInfo` 结构体，字段与
现 JSON 逐字对齐（name/version/description/dir/health [+ source/rev/
content_hash/installed_at/resources]）。序列化产物**几乎**不变（两处微差：
`rev` 缺失时由 `null` 变为省略；`installed_at` 时间戳由 `+00:00` 尾缀变
`Z`），wire 不变，只
是前端 TS 直接对齐 serde 输出（AGENTS.md 协议规则：前端 TS 与 Rust
serde 不做二次映射），CLI 的 `row["name"]` 改成 `row.name`、`health`
也从字符串换成枚举或保持 String（保持 String 最省事，前端做映射）。
纯类型收紧，风险低。

### GUI 命令层（新增 `crates/gui/src/commands/extension.rs`）

照 `commands/automation.rs` 样板，全走 `state.kernel_snapshot()`：

| Tauri 命令 | 参数 | 返回 |
|---|---|---|
| `list_extensions` | — | `Vec<ExtensionInfo>` |
| `install_extension` | `source: String` | `InstallReport` |
| `remove_extension` | `name: String` | `RemoveReport` |

`commands/mod.rs` 加 `pub mod extension;`；`main.rs` 的
`generate_handler![]` 注册三命令。前端 `lib/api.ts` 加对应
`invokeCmd` 包装与 TS 接口（`ExtensionInfo` / `InstallReport` /
`RemoveReport` / `ConfiguredResources`）。

**超时**：`invokeCmd` 默认 30s。GitHub 源 `git clone` 上限 3 分钟，
`install_extension` 必须单独放宽（`withTimeout` 传 ~240000ms），否则慢
网必现假超时。本地源无此问题，但统一用同一放宽值即可。

## 面板布局

`PanelHeader` 由 ConfigPanel 提供，本面板只出主体。遵循 DESIGN.md：
单列、master–detail、渐进披露、语义色、不套卡片。

### Header actions（挂在 Extensions 节的 nav 行右端或面板顶部）

- **Refresh**（软按钮，`RefreshCw`）——卡片是快照，外部改动靠它重读
  （与通道面板卡"不自动跟踪变更"同口径）。
- **Install**（软按钮，`Plus`，主操作）——打开安装对话框。

### 列表（master）

一个已装扩展一行（折叠态）：

```
▸ memory-system   0.1.0                    ● ok
  persistent memory system for the agent
  Crescent617/yomi-extensions/ext/memory-system
  1 cron · 0 hooks · 1 bin · 1 snippet
```

- **name** 等宽；**version** 等宽、quiet。
- **health** 语义色圆点 + 文本标签（颜色不单独承载信息，见下表）。
- **description** quiet；**source** 等宽、截断、hover 显示全串。
- **资源计数** 用 `micro-label`（10px 大写 mono）：
  `{n} cron · {n} hooks · {n} bins · {n} snippets`（零项省略）。

排序：按 name 字典序（稳定、可预期）。

### 详情（点击展开，渐进披露）

- **Identity**：name / version / description / `dir`（等宽、可复制）。
- **Provenance**：`source`（原始字符串全串；本地源显示
  `local: /abs/path` 并用 `FolderOpen` 按钮在文件管理器打开）、`rev`
  （等宽，前 8 位展示、可复制全 sha；本地源显示 `—`）、`installed_at`
  （相对时间）、`content_hash`（等宽截断）。
- **Resources**：
  - cron：job 全名 `ext:<名>:<条目>`，行尾一个"去 Automation"跳转
    （`requestActivePanel("automation")`）。
  - hooks：挂载相对路径（`pre_tool_use/50-guard`）。
  - bins：文件名（提示 "command on PATH: `<名>`"）。
  - snippets：文件名（提示 "assembled into system prompt"）。
- **Actions**：`Remove`（破坏性，靠右，`destructive` 语义色）；
  `Reinstall`（以同一 `source` 再走一遍 install = 更新）。

无注册表条目（health=`foreign`）时：隐藏 Reinstall/Remove，提示
"not installed by yomi (no ownership record); remove will refuse"，并给
"Open folder"。

### 健康态语义表

| health | 色 | 文本 | 处置提示 |
|---|---|---|---|
| `ok` | `success` | OK | — |
| `modified` | `warning` | Modified locally | 重装可还原（重装覆盖包内容） |
| `foreign` | `warning` | Not installed by yomi | 用户手放或 ext.toml 损坏；remove 拒绝，只能手动删目录 |
| `unreadable` | `error` | Unreadable | 权限/损坏，检查目录权限 |
| `oversized` | `error` | Oversized file | 包内单文件 > 1 MiB 或总量超限，移走大文件或重装 |

## 安装流程

`Install extension` 对话框（仿 `CreateJobModal`）：

- **Source** 文本框，文案与占位见下「安装引导」。
- **Choose folder…** 按钮（见「本地路径安装」）。
- 提交 → `install_extension(source)`。
- **Busy 态**：clone 可能长达 3 分钟，显示不确定进度
  （`LoadingIndicator`）+ "Fetching & installing…"，禁用输入与提交。
- **结果报告**：照 CLI 口径逐项列出——
  - `cron <全文名> (created / content updated / exists, identical)`
  - `hook|bin <挂载路径> (linked / already linked)`
  - `init <脚本> ran: <输出尾部单行>`（声明了 init 才有）
  - `snippet <文件> (assembled into system prompt)`
- **失败**：直接用 kernel 的类型化错误（invalid/conflict/io/cron/init）；
  部分安装的情形附 "re-run to converge" 提示（与 CLI 一致）。

安装成功或 touched 了 cron 时，一并 `automationStore.load()`（cron 表
已变，Automation 面板需刷新）。

## 卸载流程

两段确认（`ConfirmDialog`，与 cron delete 同档）：

- 标题 `Remove <名>?`，正文列出将移除的内容（cron 全名、挂载路径、包
  目录），并说明 "只删由 yomi 安装的部分；被换过的挂载会留下"。
- 确认 → `remove_extension(name)` → 结果：`cron_removed` 条数、
  `mounts_removed` / `mounts_left`（留下并 warn 的槽位）、
  `ext_dir_removed`。
- 残留挂载（`mounts_left`）在详情面板标一条 warning 行。
- 卸载后刷新列表；cron 有变动则 `automationStore.load()`。

## 状态与空态

- **Loading**：首次加载列表骨架（`LoadingSkeleton` ×3，`role=status`）。
- **Empty**：见下「安装引导」。
- **Error**：顶部内联错误条（`error` 语义色）+ Retry；不用全局通知。
- **Busy**：install/remove 进行时动作按钮转 spinner 并禁用，防重复提交。

## 安装引导：怎么告知用户怎么装

分三层，都在界面上就地出现，不要求用户先读终端文档：

1. **空态**（无已装扩展）——不是干巴巴一句 "No extensions installed"，
   而是一个短引导块：
   - 一句话说明扩展是什么（cron/hooks/bin/snippets 打成一个包）。
   - **来源三形式**一行：`owner/repo[/subdir][@ref]` · GitHub URL ·
     local folder。
   - 一个**可复制的示例**：
     `Crescent617/yomi-extensions/ext/memory-system`（官方资产仓里的
     真实包）。
   - `[Install]` 按钮；旁边一行小字 "没有现成的？让 agent 帮你写一个"
     （见「教 AI 写扩展」）。
2. **安装对话框内联帮助**——输入框下方常驻三行示例（缩略版），随输入
   变化不高亮、只作说明；以及一句 "install copies the package into
   <dir>; reinstall = update"。
3. **规范来源不重复**：完整契约在 `yomi doc extension`（终端）与
   `docs/design/ext-packages.md`。对话框角上一个 "Docs" 链接用
   `opener` 打开仓库文档，或提示在终端跑 `yomi doc extension`。
   **单一真相仍在那两处**，面板只放"够用就好"的摘要；摘要与文档若
   漂移，以文档为准（并记一条维护注意）。

## 教 AI 写扩展（authoring）

"教 AI 写插件"在 yomi 里的正解是 **skill**：skill 把知识送进 prompt，
教 agent 怎么做事（见 AGENTS.md 分工）。现状是**没有**这样的 skill——
`yomi doc extension` 是给人读的契约，不是给 agent 的触发式工艺指南。
建议补上：

1. **新增 authoring skill**（建议名 `write-extension`，落
   `~/.agents/skills/` 全局 + 收进 `yomi-extensions/skills/` 资产仓）。
   内容（简洁，指向而非照抄全契约）：
   - 触发词：写个扩展包 / write a yomi extension / package for yomi。
   - 包格式与 manifest 必填项（`ext.toml` 的 `[ext]` + `[[cron]]`）。
   - 四类资源（cron/hooks/bin/snippets）各自契约与命名约定（hook 条目
     `50-` 起、脚本放 `scripts/` 别放 `bin/`、cron 消息文本约定）。
   - **开发回路**：写目录 → `yomi extension install <本地目录>` 验证 →
     `yomi cron list` / `yomi rpc preview_system_prompt` 查效果 →
     `yomi extension remove` 清理。
   - 参考实现：`examples/extensions/demo`（最小全类型样例）与
     `ext/memory-system`（真实包）。
   - 完整契约指路 `yomi doc extension`。
2. **界面联动**：空态/对话框里的 "Ask the agent to write one" 一键
   把预填 prompt 发进当前会话（例如 "帮我写一个 yomi 扩展包：<用途>"），
   agent 命中 skill 即进入编写流程。用户不必知道 skill 存在。
3. `examples/extensions/demo` 保持为官方最小样例，skill 与文档都指向
   它。

## 本地路径安装

**kernel 已支持**（`extension/source.rs`：`ExtSource::Local`，已存在的
目录优先按本地处理），无需改 kernel。要做的是把这条路在界面上做顺：

- **输入**：对话框 Source 框即可直接填绝对路径（`/Users/.../my-ext`）；
  `parse_source` 在 daemon 端解析（同一台机器）。**不支持 `~`**——
  kernel 不做波浪号展开，帮助文案只写绝对路径或 `./` 前缀。
- **Choose folder…**：按钮调 `@tauri-apps/plugin-dialog` 的
  `open({ directory: true })`（该插件已在 `main.rs` init），选中目录
  填进输入框。
- **歧义提示**：相对形式 `owner/repo` 若恰好存在同名目录会被当本地，
  帮助文案里写明"本地请用绝对路径或 `./` 前缀"。
- **来源标注**：本地装的包，详情里 source 显示
  `local: /abs/path`，`rev` 显示 `—`，并提供 `FolderOpen` 打开目录。
- **开发回路提示**：本地源是**复制**不是 symlink，改完包内容要
  `Reinstall`（重跑 install 重新取货复制）才生效——详情里给一句提示，
  避免用户改完目录发现"没变化"。
- **超时**：本地源无 clone，不受 3 分钟上限影响（统一放宽值下无副作用）。

## 导航接入

- 命令面板 `lib/command-palette.svelte.ts` 加一条 "扩展"（keywords 含
  `extension / extensions / plugin / 插件`），run = 切到 config 面板并
  定位 `extensions` section（经上面新增的 `appState.configSection`）。
  命令面板现有条目均为中文，本条与邻居保持一致用中文，同时带英文
  keywords。

## 明确不做（本期）

- **不做应用内市场 / 远程注册表 / 搜索浏览**——ext-packages 决策
  "不做远程 registry"；装法就是给一个 URL/路径，不是逛商品。
- **不做包内容编辑**（ext.toml / 脚本）——注册表真相在文件系统，编辑
  靠外部编辑器/终端；面板不引入第二真相源（本地源可用 Open folder
  引导到编辑器）。
- **不做单资源开关**（hooks/bin 的"执行位即开关"）——属外挂体系语义，
  不在生命周期层。
- **不做健康自动轮询**——快照 + 手动 Refresh。
- **不管理手工外挂**（`hooks/` `tools/` 直接放的）——那是外挂体系。
- **不做 update 命令**（manifest diff + 带确认清扫）——那是
  ext-packages 二期；本期"更新" = 用同一 source 重装（Reinstall 即此
  语义）。

## 分期

- **一期（本设计）**：kernel `extension_list` 类型收紧 + 三 GUI 命令
  + Config 面板新增 Extensions 节（列表 + 详情 + 安装对话框 + 卸载确认
  + 刷新）+ 安装引导（空态/对话框内联帮助）+ 本地路径（输入 + Choose
  folder + 来源标注）+ 命令面板接入 + authoring skill（首版）。
- **二期（不占版，等真实需求）**：Resource 行跳 Automation 高亮具体
  job、详情里 "Open in Finder"、tools/skill 资源展示（随 ext-packages
  二期落地）、`extension update`（manifest diff + 确认清扫）。

## 测试

- **Rust 单测**：`installed_test.rs` 覆盖 `ExtensionInfo` 序列化形状
  （snake_case、缺元数据字段省略、`installed_at` 以 `Z` 结尾）、反序列化
  回环（remote client `call_json` 路径）、以及 `ExtensionHealth::as_str()`
  与 serde 输出逐字一致。
- **前端 vitest**（`extensions/model.test.ts`）：health→(色/文本/提示) 映射、
  未知态落中性（绝不显示成功）、资源计数单复数、安装报告标签、相对时间、
  本地/GitHub 判别、移除确认清单、残留挂载提示。
- **Playwright 功能 e2e**（`e2e/ext-panel.spec.ts`，mock `invoke`、真组件）：
  列表渲染、Refresh 重新拉取、空态引导、未知健康态、移除（确认清单枚举目标
  → 调 `remove_extension` → 列表刷新）、安装（提交来源 → 报告 → 刷新）。
  另有 `e2e/ext-panel-screenshots.spec.ts`（一次性截图产物，无断言）。
- **隔离真机 e2e**：`evals/ext-e2e.sh`（CLI→wire→kernel→fs/sqlite，45 断言）
  在本机隔离 daemon 上跑过全绿；另手工 install/list/remove 一轮验证
  `extension_list` typed 返回的线上 JSON 形状。

## 待办与开放问题

已定（hrli 2026-10-31 拍板，均按"最优雅"落地）：
- ① 放 Config 面板的一节（非一级 tab）。
- ② 界面标签英文（Extensions）；命令面板条目随邻居用中文「扩展」，
  keywords 带英文。
- ③ `extension_list` 收紧为 typed `ExtensionInfo`（采纳）。
- ④ 一期含 Reinstall（重装=更新唯一路径），保持简洁。
- ⑤ authoring skill 落点：正文在 `yomi-extensions/skills/write-extension`
  资产仓（单一真相），并 symlink 进 `~/.agents/skills/`（本机即刻可用，
  与其余资产仓 skill 同法）。
- ⑥ Docs 指路：不做硬编码 URL 外链，面板内联帮助自足 + 提示终端
  `yomi doc extension`（也免去离线/开发环境下链接失效）。
- ⑦（2026-11-01）列表行直接显示 Reinstall/Remove 为安静 ghost 图标
  （默认灰、hover 转色），不藏进展开；展开详情回归纯信息。本地来源不用
  `file://`，保持纯路径（Folder… 选择器产出绝对路径）。

**一期已落地**（2026-10-31 起，未发版）：

- kernel：`ExtensionInfo` / `ExtensionHealth`（`extension/installed.rs`）
  + `extension_list` 改返回 typed；client trait/local/remote、dispatcher、
  CLI `list`、`doctor` 同步收紧（JSON 形状近乎不变，见「数据与 wire」）。
- GUI 后端：`commands/extension.rs`（list/install/remove）注册进
  `commands/mod.rs` 与 `main.rs`。
- 前端：`api.ts` 类型与三函数（install 放宽超时 240s）；
  `components/extensions/`（`ExtensionsPanel.svelte` 列表+行内动作+详情+
  工具栏+空态引导、`InstallExtensionDialog.svelte` 来源输入 + Folder…
  选择器 + 结果报告、`model.ts` 纯函数）；`extensions.svelte.ts` 跨节 busy
  态；`ConfigPanel.svelte` 加 Extensions 节（in-flight 时禁切节）；
  `state.svelte.ts` 加 `ConfigSection`/`appState.configSection`；命令面板加
  「扩展」条目；`ConfirmDialog` 加 `disabled`。
- skill：`yomi-extensions/skills/write-extension/SKILL.md` + 全局
  symlink；资产仓 README 增行。
- 验证（2026-11-01）：`just ci` 全绿；`evals/ext-e2e.sh` 45/45；前端
  svelte-check 0 错、vitest 478 过、Playwright 功能 e2e 5/5、prettier 干净。

未做（留二期或后续）：见「明确不做」与「分期 · 二期」。
