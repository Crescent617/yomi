# GUI Agent Template 管理面板设计

> **实现备注（2026-08-07，已按最简方案落地）**：实现时做了以下简化，与下文设计稿的差异以此为准——
>
> 1. **无 `TemplateDetail`/`list_detailed` 层叠视图**：列表直接复用合并后的 `AgentTemplate { name, body, source }`，覆盖关系仅靠 source badge 表达（新建同名时才提示 "will override"）。
> 2. **无 `path` 字段**、无搜索框；dirty guard 用复用的 `ConfirmDialog`（切模板时确认丢弃）。
> 3. `TemplateScope` 只有 `global`/`workspace`，builtin 只读 + "Duplicate to Global"。
> 4. wire 协议版本 bump 到 24；`dispatcher_test.rs` 新增 `test_agent_templates_wire_round_trip` 全链路测试。
> 5. **review 后修正**：`session_asset_cwd` 对齐 spawn 解析（`working_dir` → `<data_dir>/workspace`，**无 project dir 步骤**）——原稿「抽 helper 与 `list_session_skills` 共用」作废，两者规则本就不同（skills 解析有 project dir 回落，spawn 没有），强行共用反而互相污染。
> 6. 前端纯逻辑（名字校验/覆盖提示/草稿脏判定）抽至 `agents/template-utils.ts` 并配 vitest；面板随活跃会话切换自动 reload 并重置选择。
>
> 保留的核心决策不变：独立 ActivityBar 面板、session 语境解析 workspace 层、全部操作走 wire。

## 背景

`agent` 工具的 `template` 参数引用角色模板（`<name>/ROLE.md`，全文即 subagent 系统提示，无 frontmatter）。内核 `crates/kernel/src/agent_tmpl/mod.rs` 提供三层合并的只读视图：

```
builtin（include_str! 地板层：planner/verifier/explorer/reviewer）
  → global（<data_dir>/agents/<name>/ROLE.md）
  → workspace（<session working_dir>/.yomi/agents/<name>/ROLE.md）
同名后者覆盖前者；resolve() 实时读盘，spawn 时生效。
```

现状缺口：

1. **无任何管理界面**——CLI/TUI/GUI 都没有模板的列出/查看/编辑入口，用户只能手动操作文件。
2. **`agent_tmpl` 只有读路径**（`list`/`resolve`/`available_summary`），没有 create/update/delete。
3. **合并视图丢失覆盖关系**——`merge()` 只保留赢家，管理界面需要看到「workspace 的 `reviewer` 覆盖了 builtin」这类信息。
4. **workspace 层上下文依赖会话**——subagent 工具以父会话 `working_dir`（缺省回落 `<data_dir>/workspace`）解析 workspace 层，面板必须复用同一解析规则，否则看到的内容与运行时不一致。

GUI 通过 `KernelApi` 与 daemon 通信（可能连远程 daemon），因此模板读写必须走 wire 协议，不能在 GUI 本地直接操作文件。

## 目标

- 在 GUI 中提供模板的一站式管理：查看（含来源层与覆盖关系）、新建、编辑、删除。
- 面板展示与 subagent spawn 时 `resolve()` 看到的完全一致的有效视图。
- builtin 只读，但支持一键「覆盖到 global/workspace」（复制正文创建上层同名模板）。

## 非目标

- 编辑内置模板本身（随二进制发布，不可写）。
- 模板导入/导出、模板包（pack）、变量插值、版本历史。
- TUI/CLI 管理命令（可后续复用同一批 kernel 方法）。

## 总体方案

新增 ActivityBar 面板 **Agents**（icon: `Bot`），master–detail 布局：左侧模板列表（按来源分组 + 搜索），右侧详情/编辑器。全链路改动：

```
frontend AgentsPanel ──invoke──> gui commands::agent_template ──KernelApi──> wire ──> dispatcher ──> Kernel ──> agent_tmpl
```

选择独立面板而非 Config 子页的理由：模板是 CRUD 管理面（同 Automation/Favorites），Config 的三个子页（Application/Theme/Kernel）都是单页设置表单；且模板有 workspace 层语境，语义上是「agent 资产」而非「应用配置」。

## Kernel 层改动

### `agent_tmpl` 模块扩展

```rust
/// 单条模板的层叠详情：layers 按 floor→top 排序，最后一个即生效版本。
pub struct TemplateDetail {
    pub name: String,
    pub layers: Vec<TemplateLayer>,
}

pub struct TemplateLayer {
    pub source: TemplateSource,
    pub body: String,
    pub path: Option<PathBuf>,   // builtin 为 None
}

/// 保留覆盖关系的清单（不 merge）。
pub async fn list_detailed(global_dir: &Path, working_dir: Option<&Path>) -> Vec<TemplateDetail>;

/// 名字校验：^[a-z0-9][a-z0-9-]{0,63}$，拒绝路径分隔符。
pub fn validate_name(name: &str) -> Result<(), AgentTemplateError>;

/// 写入 <root>/<name>/ROLE.md（自动建目录）。root 由调用方按 scope 算出。
pub async fn save(root: &Path, name: &str, body: &str) -> Result<PathBuf, AgentTemplateError>;

/// 删除 <root>/<name>/ 整个目录。builtin 不可删（调用方保证 root 不含 builtin）。
pub async fn delete(root: &Path, name: &str) -> Result<(), AgentTemplateError>;
```

- `list()`/`resolve()` 保持不动（spawn 热路径签名不变），`list_detailed` 与 `merge` 共用 `load_dir`。
- `save` 拒绝空 body（与现有空 body warn 对齐，写入侧直接报错）。
- 单测放 `agent_tmpl_test.rs`：校验规则、save/delete 往返、覆盖关系在 `list_detailed` 中的表达。

### Kernel coordinator

`Kernel` 新增三个方法。关键是**复用会话 cwd 解析**——把 `list_session_skills` 里「session working_dir → project dir → `<data_dir>/workspace`」这段抽成私有 helper `resolve_session_cwd(&SessionId) -> PathBuf`，两处共用，保证面板与 spawn 解析一致：

```rust
pub async fn list_agent_templates(&self, session_id: Option<&SessionId>)
    -> Result<Vec<TemplateDetail>>;
pub async fn save_agent_template(&self, session_id: Option<&SessionId>,
    scope: TemplateScope, name: &str, body: &str) -> Result<()>;
pub async fn delete_agent_template(&self, session_id: Option<&SessionId>,
    scope: TemplateScope, name: &str) -> Result<()>;

pub enum TemplateScope { Global, Workspace }   // builtin 不可写，无需表示
```

- `session_id = None`：只列/写 global 层（无会话上下文时 workspace 不可达）。
- `save` 到 builtin 同名 = 合法覆盖，但由 UI 显式确认（见下）。

### Wire / dispatcher / client

`ReqMethod` 增加：

```rust
ListAgentTemplates { session_id: Option<String> },
SaveAgentTemplate { session_id: Option<String>, scope: TemplateScope, name: String, body: String },
DeleteAgentTemplate { session_id: Option<String>, scope: TemplateScope, name: String },
```

`TemplateScope`/`TemplateDetail`/`TemplateLayer` 挂 `#[serde(rename_all = "snake_case")]`（遵循全项目 snake_case 约定）。dispatcher 三个新分支；`KernelApi` trait + client 实现各加三个方法。

## GUI 层改动

### Rust commands（`src/commands/agent_template.rs`）

```rust
#[tauri::command(rename_all = "snake_case")]
pub async fn list_agent_templates(state, session_id: Option<String>) -> Result<Vec<Value>, GuiError>;
pub async fn save_agent_template(state, session_id: Option<String>, scope: String, name: String, body: String) -> Result<(), GuiError>;
pub async fn delete_agent_template(state, session_id: Option<String>, scope: String, name: String) -> Result<(), GuiError>;
```

在 `commands/mod.rs`、`main.rs` 的 `generate_handler!` 注册。

### 前端

- `lib/api.ts`：`AgentTemplateDetail`/`AgentTemplateLayer` 类型 + 三个 invoke wrapper。
- `lib/state.svelte.ts`：`ActivePanel` 加 `"agents"`。
- `lib/components/layout/ActivityBar.svelte`：tabs 加 `{ id: "agents", icon: Bot, label: "Agents" }`。
- `lib/components/layout/Layout.svelte`：`activePanel === "agents"` 分支渲染 `AgentsPanel`。
- 新组件 `lib/components/agents/`：
  - `AgentsPanel.svelte`——容器：数据加载、选中态、dirty guard。
  - `TemplateList.svelte`——左侧列表。
  - `TemplateEditor.svelte`——右侧详情/编辑。

## 面板交互设计

遵循 DESIGN.md：workspace 式整 pane、micro-label 分组头、语义色、软按钮、反馈就地。

```
┌──────────────────────────────┬───────────────────────────────────────┐
│ [搜索…]              [+ New] │ reviewer            [global] [覆盖 ⓘ] │
│──────────────────────────────│ ~/.yomi/agents/reviewer/ROLE.md       │
│ BUILTIN                      │───────────────────────────────────────│
│   explorer                   │ ┌───────────────────────────────────┐ │
│   planner                    │ │ # Reviewer                        │ │
│   reviewer ⓘ                 │ │ You are a code reviewer...        │ │
│   verifier                   │ │ (mono editor)                     │ │
│ GLOBAL                       │ └───────────────────────────────────┘ │
│   my-critic                  │                                       │
│ WORKSPACE (当前项目)          │                    [Delete]  [Save]   │
│   release-checker            │                                       │
└──────────────────────────────┴───────────────────────────────────────┘
```

**列表（左）**

- 按生效来源分组：`BUILTIN` / `GLOBAL` / `WORKSPACE`（micro-label 组头）。workspace 组标题旁标注当前解析到的项目目录名；无会话上下文时该组不渲染。
- 每行：模板名（IBM Plex Mono）+ 覆盖标记 `ⓘ`（该名字在更低层也存在时）。tooltip 说明覆盖链，如 `overrides builtin`。
- 顶部搜索框按名字过滤；`+ New` 弹出新建对话框。
- 排序组内按名字（与 kernel 输出一致，稳定）。

**详情（右）**

- 头部：名字（mono）+ 来源 badge（`builtin` 用 neutral、`global` 用 `info`、`workspace` 用 `primary` 色系语义色）+ 覆盖说明；下一行完整路径（`code-bg`、mono、muted；远程 daemon 时仅作信息展示）。
- 正文：等宽编辑器（参考 `ConfigEditor` 的 textarea 模式，IBM Plex Mono），行高舒适，整 pane 铺满。
- builtin 模板：编辑器只读，主操作为 **「Override to Global / Workspace」**（下拉选目标层），点击后以其正文创建上层副本并进入编辑态。
- 底部操作条：`Save`（soft primary，仅 dirty 时可用）、`Delete`（destructive，确认对话框；若删除的是覆盖层，文案提示「删除后将恢复显示 builtin 版本」）。
- 校验：保存前本地跑与 kernel 相同的名字规则与空 body 检查，错误就地显示在操作条旁。

**新建对话框**

- 字段：名字（mono input，实时校验 kebab-case）、目标层（radio rows：`Global`——所有项目可用，默认选中；`Workspace`——仅当前项目，无会话上下文时禁用并说明）。
- 名字与现存模板冲突时：若为 builtin/global 同名，明确文案「将创建覆盖层」，提交按钮变为 `Override`；同层同名直接报错。

**状态与反馈**

- 加载：`LoadingSkeleton` 列表骨架（参考 skills 页）。
- 保存/删除：按钮内联 loading；成功后 toast + 列表刷新（保存后选中项不丢）。
- 切换模板时有未保存修改：dirty guard 确认（与 ConfigEditor 一致的模式）。
- 空态：无任何自定义模板时，workspace/global 组渲染空态文案 + 「从 builtin 复制一个开始」引导。
- 刷新：列表头部 refresh 按钮（模板可能被外部进程改动；spawn 实时读盘，保存即生效）。

## 边界与风险

| 问题 | 决策 |
|---|---|
| 远程 daemon | 全部走 wire，GUI 不碰本地文件；路径仅展示 |
| 无活跃会话 | global/builtin 可管，workspace 组隐藏；`session_id=None` |
| 并发外部修改 | 不做乐观锁，last-write-wins + 手动 refresh（与 config 编辑一致） |
| 删除 workspace 覆盖层 | 确认对话框说明「恢复下层生效」；builtin 永不可删 |
| 生效时机 | spawn 时实时读盘，无需通知运行中 agent；文档与 UI 提示「下次 spawn 生效」 |
| 名字注入 | kernel `validate_name` 统一拒绝 `..`、`/`、非 kebab 字符 |

## 实施步骤

1. **kernel `agent_tmpl`**：`TemplateDetail`/`list_detailed`/`validate_name`/`save`/`delete` + 单测。
2. **Kernel coordinator**：抽 `resolve_session_cwd` helper（`list_session_skills` 同步重构），加三个模板方法。
3. **wire + dispatcher + client**：三个新 ReqMethod 及全链路实现 + dispatcher 测试。
4. **GUI commands**：`agent_template.rs` 三个 command + 注册。
5. **前端**：api.ts、ActivePanel/ActivityBar/Layout 接入、`AgentsPanel` 三组件 + vitest 组件测试（参考现有 `lib/*.test.ts` 模式）。
6. 冒烟：本地起 daemon，验证 builtin 只读、global CRUD、workspace 覆盖与删除恢复、无会话上下文降级。

## 测试要点

- `agent_tmpl_test.rs`：名字校验矩阵、save→resolve 往返、delete 后下层恢复可见、`list_detailed` 层序正确。
- dispatcher：三方法参数序列化/反序列化、错误路径（builtin scope 写入、非法名字）。
- 前端：覆盖标记渲染、dirty guard、builtin 只读态、新建对话框冲突分支。
