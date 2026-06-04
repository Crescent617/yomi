# GUI Cron 管理页面设计文档

> 状态：设计阶段  
> 目标：在 yomi-gui 中新增 Automation 面板，优雅地管理 cron 定时任务  

---

## 1. 架构定位

### 1.1 现有 GUI 架构

```
┌─────────────────────────────────────────────────────────────┐
│                     yomi-gui (Tauri)                        │
├─────────────────────────────────────────────────────────────┤
│  Frontend (Svelte)                                          │
│  ├── ActivityBar  ← 左侧图标栏                              │
│  ├── Layout       ← 主布局，根据 activePanel 切换           │
│  └── API Layer    ← 通过 Tauri invoke 调用 Rust commands    │
├─────────────────────────────────────────────────────────────┤
│  Rust Backend                                               │
│  ├── commands/    ← Tauri commands                          │
│  ├── daemon.rs    ← 启动/管理 kernel daemon                 │
│  └── state.rs     ← AppState 持有 coordinator               │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
                    ┌─────────────────┐
                    │   Coordinator   │  ← in-process 或 daemon
                    │   (kernel)      │
                    └────────┬────────┘
                             │
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
        ┌─────────┐   ┌──────────┐   ┌──────────┐
        │ Session │   │ CronStore│   │ Project  │
        │         │   │ (SQLite) │   │ Store    │
        └─────────┘   └──────────┘   └──────────┘
```

### 1.2 Cron 在 GUI 中的定位

- **Kernel 侧**：`CronScheduler` + `CronWorker` + `SqliteCronStore` 已经实现
- **GUI 侧**：只负责**管理界面**（CRUD + 展示），不处理调度逻辑
- **调度执行**：由 `KernelServer` 中的 `CronScheduler` 负责
  - GUI 启动 daemon 时，scheduler 自动启动
  - 如果只用 in-process `Coordinator`，scheduler 不运行，任务仅持久化

### 1.3 数据流

```
User ──▶ Frontend ──▶ Tauri Command ──▶ coordinator.cron_store()
                                              │
                                              ▼
                                        SqliteCronStore
                                              │
                                              ▼
                                        cron_jobs 表
                                              ▲
                                              │
                              KernelServer CronScheduler
                              (when daemon is running)
```

---

## 2. 设计原则

1. **只读优先**：页面加载时列出所有 cron 任务，状态一目了然
2. **最小操作**：每个任务卡片上直接展示状态、下次执行时间、操作按钮
3. **优雅空态**：没有任务时展示引导创建的空状态
4. **实时反馈**：创建/删除/暂停后立即刷新列表
5. **前后端一致**：前端类型与后端 `CronJob` 结构完全对齐

---

## 3. 页面设计

### 3.1 ActivityBar 新增 Automation Tab

```
┌────┐
│ 💬 │  Chat
├────┤
│ 📊 │  Usage
├────┤
│ ⚡ │  Automation  ← 新增
├────┤
│ ⚙️ │  Config
└────┘
```

图标使用 `Zap` (lucide-svelte)。

### 3.2 Automation Panel 布局

```
┌────────────────────────────────────────────────────────────────────┐
│  ⚡ Automation                                      [+ New Task]   │
├────────────────────────────────────────────────────────────────────┤
│                                                                    │
│  ┌─────────────────────────┐  ┌─────────────────────────────────┐ │
│  │  ● Daily Standup        │  │  Daily Standup                  │ │
│  │  Every day at 9:00 AM   │  │  ━━━━━━━━━━━━━━━━━━━━━━━━━━━━  │ │
│  │  Next: in 2 hours       │  │                                 │ │
│  │  [💬 Send Message]      │  │  Schedule                       │ │
│  │                         │  │  ┌─────────────────────────┐   │ │
│  │  ● Weekly Report        │  │  │ 0 0 9 * * *             │   │ │
│  │  Every Friday 6:00 PM   │  │  └─────────────────────────┘   │ │
│  │  Next: 3 days           │  │  Every day at 9:00 AM          │ │
│  │  [💬 Send Message]      │  │                                 │ │
│  │                         │  │  Action                         │ │
│  │  ○ Heartbeat            │  │  ┌─────────────────────────┐   │ │
│  │  Every 30 minutes       │  │  │ 💬 Send Message         │   │ │
│  │  Paused                 │  │  │ Session: project-alpha  │   │ │
│  │  [🔧 Shell]             │  │  │ "Review today's tasks"  │   │ │
│  │                         │  │  └─────────────────────────┘   │ │
│  │  ✕ Failed Task          │  │                                 │ │
│  │  Every hour             │  │  Status: ● Active               │ │
│  │  Last error: timeout    │  │  Next run: Today, 2:00 PM       │ │
│  │  [🔧 Shell]             │  │  Last run: Today, 1:30 PM ✓     │ │
│  │                         │  │  Runs: 42                       │ │
│  │                         │  │                                 │ │
│  │                         │  │  [Pause] [Edit] [Delete]        │ │
│  │                         │  │                                 │ │
│  └─────────────────────────┘  └─────────────────────────────────┘ │
│                                                                    │
└────────────────────────────────────────────────────────────────────┘
```

### 3.3 任务卡片状态

| 状态 | 颜色 | 图标 | 说明 |
|------|------|------|------|
| Active | 绿色 | `●` | 正常调度中 |
| Paused | 黄色 | `○` | 手动暂停 |
| Completed | 灰色 | `✓` | 达到 max_runs 或过期 |
| Failed | 红色 | `✕` | 上次执行失败 |

### 3.4 创建任务弹窗

```
┌─────────────────────────────────────────┐
│  Create Automation Task          [×]    │
├─────────────────────────────────────────┤
│                                         │
│  Name *                                 │
│  ┌─────────────────────────────────┐   │
│  │ Daily Standup                   │   │
│  └─────────────────────────────────┘   │
│                                         │
│  Schedule *                             │
│  ┌─────────────────────────────────┐   │
│  │ 0 0 9 * * *                     │   │
│  └─────────────────────────────────┘   │
│  [Every minute] [Every hour] [Daily]   │
│  [Weekdays] [Weekly] [Monthly]         │
│  Every day at 9:00 AM                  │
│                                         │
│  Action Type *                          │
│  [💬 Send Message] [🔧 Shell]          │
│                                         │
│  ── Send Message ──                     │
│  Session *                              │
│  ┌─────────────────────────────────┐   │
│  │ project-alpha                   │   │
│  └─────────────────────────────────┘   │
│                                         │
│  Content *                              │
│  ┌─────────────────────────────────┐   │
│  │ Check today's tasks             │   │
│  │                                 │   │
│  └─────────────────────────────────┘   │
│  💡 Supports {{date}}, {{time}}        │
│                                         │
│  [Cancel]              [Create Task]    │
│                                         │
└─────────────────────────────────────────┘
```

---

## 4. 数据模型

### 4.1 后端 `CronJob`（kernel 已定义）

```rust
pub struct CronJob {
    pub id: CronJobId,              // String
    pub name: String,
    pub schedule: String,           // cron expression, e.g. "0 0 9 * * *"
    pub action: CronAction,         // enum with tag "ty"
    pub status: CronJobStatus,      // "active" | "paused" | "completed" | "failed"
    pub created_at: DateTime<Utc>,  // RFC3339
    pub updated_at: DateTime<Utc>,
    pub next_run_at: Option<DateTime<Utc>>,
    pub last_run_at: Option<DateTime<Utc>>,
    pub run_count: u32,
    pub max_runs: Option<u32>,
    pub expires_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub enum CronAction {
    SendMessage { session_id: String, content: String },
    Shell { command: String, working_dir: Option<String> },
    Internal { endpoint: String, payload: Value },
}
```

### 4.2 前端 `CronJob` 类型

```typescript
interface CronJob {
  id: string;
  name: string;
  schedule: string;
  action: {
    ty: "send_message" | "shell" | "internal";
    session_id?: string;
    content?: string;
    command?: string;
    working_dir?: string;
    endpoint?: string;
    payload?: unknown;
  };
  status: "active" | "paused" | "completed" | "failed";
  created_at: string;   // RFC3339
  updated_at: string;
  next_run_at: string | null;
  last_run_at: string | null;
  run_count: number;
  max_runs: number | null;
  expires_at: string | null;
  last_error: string | null;
}
```

### 4.3 Schedule 预设

```typescript
const SCHEDULE_PRESETS = [
  { label: "Every minute", value: "0 * * * * *" },
  { label: "Every 15 minutes", value: "0 */15 * * * *" },
  { label: "Every hour", value: "0 0 * * * *" },
  { label: "Every day at 9AM", value: "0 0 9 * * *" },
  { label: "Every weekday at 9AM", value: "0 0 9 * * 1-5" },
  { label: "Weekly on Monday", value: "0 0 9 * * 1" },
  { label: "Monthly 1st", value: "0 0 9 1 * *" },
];
```

---

## 5. API 设计

### 5.1 Tauri Commands（Rust）

```rust
// crates/gui/src/commands/automation.rs

#[tauri::command]
pub async fn list_cron_jobs(
    state: State<'_, AppState>,
    status: Option<String>,
    limit: usize,
) -> Result<Vec<serde_json::Value>, GuiError>;

#[tauri::command]
pub async fn create_cron_job(
    state: State<'_, AppState>,
    name: String,
    schedule: String,
    action: serde_json::Value,
    max_runs: Option<u32>,
) -> Result<String, GuiError>;  // returns job_id

#[tauri::command]
pub async fn update_cron_job(
    state: State<'_, AppState>,
    job_id: String,
    name: Option<String>,
    schedule: Option<String>,
    action: Option<serde_json::Value>,
    status: Option<String>,
    max_runs: Option<u32>,
) -> Result<(), GuiError>;

#[tauri::command]
pub async fn delete_cron_job(
    state: State<'_, AppState>,
    job_id: String,
) -> Result<(), GuiError>;
```

### 5.2 前端 API 封装

```typescript
// api.ts

export async function listCronJobs(
  status?: string,
  limit = 100
): Promise<CronJob[]> {
  return invokeCmd("list_cron_jobs", { status, limit });
}

export async function createCronJob(input: CreateCronJobInput): Promise<string> {
  return invokeCmd("create_cron_job", input);
}

export async function updateCronJob(
  jobId: string,
  updates: Partial<CreateCronJobInput & { status: string }>
): Promise<void> {
  return invokeCmd("update_cron_job", { jobId, ...updates });
}

export async function deleteCronJob(jobId: string): Promise<void> {
  return invokeCmd("delete_cron_job", { jobId });
}
```

---

## 6. 组件设计

### 6.1 组件树

```
Layout.svelte
└── (when activePanel === "automation")
    └── AutomationPanel.svelte
        ├── Header.svelte
        │   └── CreateJobButton
        ├── JobList.svelte
        │   └── JobCard.svelte × N
        └── JobDetail.svelte (or empty state)

CreateJobModal.svelte (portal/overlay)
```

### 6.2 组件职责

| 组件 | 职责 |
|------|------|
| `AutomationPanel` | 整体布局、加载数据、管理选中状态 |
| `JobList` | 渲染任务列表、处理排序/过滤 |
| `JobCard` | 单个任务卡片、状态展示、快速操作 |
| `JobDetail` | 选中任务的详细信息、编辑/删除 |
| `CreateJobModal` | 创建/编辑任务的弹窗表单 |

### 6.3 状态管理

```typescript
// automation.svelte.ts
export const automationState = $state({
  jobs: [] as CronJob[],
  loading: false,
  error: null as string | null,
  selectedJobId: null as string | null,
  showCreateModal: false,
  editingJob: null as CronJob | null,
});

export function selectJob(jobId: string | null) {
  automationState.selectedJobId = jobId;
}

export async function loadJobs(status?: string) {
  automationState.loading = true;
  try {
    automationState.jobs = await listCronJobs(status, 100);
    automationState.error = null;
  } catch (e) {
    automationState.error = String(e);
  } finally {
    automationState.loading = false;
  }
}
```

---

## 7. 实现计划

### 7.1 Rust 后端

| 步骤 | 文件 | 内容 |
|------|------|------|
| 1 | `crates/gui/src/commands/automation.rs` | 新建，实现 4 个 cron commands |
| 2 | `crates/gui/src/commands/mod.rs` | 导出 automation commands |
| 3 | `crates/gui/src/main.rs` | 注册 commands |

### 7.2 Kernel 适配

| 步骤 | 文件 | 内容 |
|------|------|------|
| 4 | `crates/kernel/src/app/coordinator.rs` | `Coordinator` 持有 `cron_store` 并暴露 `cron_store()` 方法 |
| 5 | `crates/kernel/src/server/mod.rs` | `KernelServer::new` 改为从 `coordinator` 获取 `cron_store` |

### 7.3 前端

| 步骤 | 文件 | 内容 |
|------|------|------|
| 6 | `crates/gui/frontend/src/lib/api.ts` | 添加 cron API 函数 |
| 7 | `crates/gui/frontend/src/lib/automation.svelte.ts` | 添加 automation 状态 |
| 8 | `ActivityBar.svelte` | 添加 automation tab |
| 9 | `Layout.svelte` | 添加 automation panel 分支 |
| 10 | `AutomationPanel.svelte` | 主面板 |
| 11 | `JobList.svelte` | 任务列表 |
| 12 | `JobCard.svelte` | 任务卡片 |
| 13 | `JobDetail.svelte` | 任务详情 |
| 14 | `CreateJobModal.svelte` | 创建/编辑弹窗 |

### 7.4 验证

| 步骤 | 内容 |
|------|------|
| 15 | `cargo check -p gui` |
| 16 | `cargo clippy -p gui --all-targets --all-features` |
| 17 | `cargo fmt -- --check` |
| 18 | 前端 TypeScript 类型检查 |

---

## 8. 关键决策

### 8.1 为什么 GUI command 直接访问 `cron_store` 而不是 `CoordinatorApi`？

- `CoordinatorApi` trait 目前没有 cron 方法
- 添加 trait 方法需要同时修改 `LocalCoordinator` 和 `RemoteCoordinator`
- GUI 使用 in-process `Coordinator`，直接访问更高效
- 未来如果 CLI/TUI 也需要 cron 管理，可以再补 `CoordinatorApi`

### 8.2 为什么 GUI 不启动自己的 scheduler？

- Scheduler 需要长期运行，GUI 窗口可能随时关闭
- 调度执行应该由 daemon 负责
- GUI 启动 daemon 时，`KernelServer` 会启动 scheduler
- 如果用户只开 GUI 不开 daemon，cron 任务只是被持久化，不会执行

### 8.3 前后端数据如何对齐？

- 前端 `CronJob` 接口字段与后端 `CronJob` struct 一一对应
- `action` 使用 `#[serde(tag = "ty", rename_all = "snake_case")]` 对齐
- `status` 使用 snake_case 字符串对齐
- 时间字段统一使用 RFC3339 字符串

---

## 9. 未来扩展

- [ ] 任务执行历史独立页面
- [ ] 按状态/类型筛选
- [ ] 任务运行日志查看
- [ ] 批量操作（暂停/删除多个）
- [ ] 任务执行失败通知
