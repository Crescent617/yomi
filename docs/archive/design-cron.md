# Kernel Server Cron Task 设计文档

> 状态：设计阶段  
> 目标版本：0.3.0  
> 作者：Yomi  

---

## 1. 概述

为 `KernelServer` 引入定时任务（Cron Task）能力，允许用户通过 cron 表达式调度周期性任务。任务触发时可执行预定义动作，如向 Session 发送消息、执行 Shell 命令等。任务状态持久化到 SQLite，Server 重启后不丢失。

### 1.1 使用场景

- **定时巡检**：每天早上 9 点向指定 Session 发送 "检查今日待办" 消息，触发 Agent 自动回顾任务。
- **定时报告**：每周五下午 6 点执行脚本生成周报摘要，发送到 Session。
- **心跳保持**：每 30 分钟向某个 Session 发送轻量消息，防止长时间空闲导致 Session 被 pruner 回收。
- **自动化工作流**：结合 `Shell` action，定时执行构建、测试、数据同步等操作。

---

## 2. 设计原则

1. **最小侵入**：Cron 模块独立，通过 trait 与 Kernel/Server 交互，不修改现有 Agent/Session 核心逻辑。
2. **持久化优先**：所有任务定义和状态落库，Server 重启后可恢复调度。
3. **精确调度**：使用成熟 cron 解析库，支持标准 cron 表达式（含秒级）。调度引擎自研，精确 sleep 到触发点，避免轮询。
4. **异步执行**：调度器只负责"到点触发"，实际执行交给独立 Worker，避免阻塞调度循环。
5. **可观测**：每次执行记录结果、耗时、错误信息，便于排查。

---

## 3. 架构设计

### 3.1 模块结构

```
crates/kernel/src/cron/
├── mod.rs          # 模块入口，导出公共类型
├── types.rs        # CronJob, CronSchedule, CronAction, CronJobStatus 等
├── store.rs        # CronStore trait + SqliteCronStore 实现
├── scheduler.rs    # CronScheduler: 调度引擎核心
└── worker.rs       # CronWorker: 任务执行器
```

### 3.2 组件交互

```
┌─────────────┐     ┌─────────────────┐     ┌─────────────┐
│   Client    │────▶│   KernelServer  │────▶│   Wire RPC  │
│  (TUI/CLI)  │◀────│                 │◀────│  (新增 Cron) │
└─────────────┘     └─────────────────┘     └─────────────┘
                             │
                             ▼
                    ┌─────────────────┐
                    │  CronScheduler  │◀─── 从 DB 加载任务
                    │  (调度引擎)      │      计算 next_run
                    └────────┬────────┘
                             │ mpsc::Sender<CronJob>
                             ▼
                    ┌─────────────────┐
                    │   CronWorker    │
                    │   (执行器)       │
                    └────────┬────────┘
                             │
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
        ┌─────────┐   ┌──────────┐   ┌──────────┐
        │Kernel│  │tokio::process│  │  (扩展)   │
        │send_message│  │  Command    │   │          │
        └─────────┘   └──────────┘   └──────────┘
              │
              ▼
        ┌─────────┐
        │ Session │
        │ (Agent) │
        └─────────┘
```

---

## 4. 核心类型设计

### 4.1 CronJob — 定时任务

```rust
/// 定时任务唯一 ID
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CronJobId(pub String);

impl CronJobId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
}

/// 任务触发时要执行的动作
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "ty", rename_all = "snake_case")]
pub enum CronAction {
    /// 向指定 Session 发送消息（触发 Agent 响应）
    SendMessage {
        session_id: String,
        /// 消息内容，支持模板变量：
        /// - {{timestamp}} — ISO8601 时间戳
        /// - {{date}} — YYYY-MM-DD
        /// - {{time}} — HH:MM:SS
        content: String,
    },
    /// 执行 Shell 命令
    Shell {
        command: String,
        working_dir: Option<String>,
    },
    /// 调用内部 API（预留扩展）
    Internal {
        endpoint: String,
        payload: serde_json::Value,
    },
}

/// 定时任务状态
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CronJobStatus {
    Active,
    Paused,
    Completed, // 达到 max_runs 或过期
    Failed,    // 连续失败超过阈值（预留）
}

/// 定时任务记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronJob {
    pub id: CronJobId,
    pub name: String,
    /// cron 表达式，如 "0 0 9 * * 1-5"（工作日 9:00）
    pub schedule: String,
    pub action: CronAction,
    pub status: CronJobStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// 下次应该执行的时间（由 scheduler 维护）
    pub next_run_at: Option<DateTime<Utc>>,
    /// 最后一次执行时间
    pub last_run_at: Option<DateTime<Utc>>,
    /// 执行次数统计
    pub run_count: u32,
    /// 最大执行次数（None = 无限）
    pub max_runs: Option<u32>,
    /// 过期时间（None = 永不过期）
    pub expires_at: Option<DateTime<Utc>>,
    /// 最近错误信息
    pub last_error: Option<String>,
}
```

### 4.2 CronSchedule — 表达式封装

使用 [`cron`](https://github.com/zslayton/cron) crate（v0.15）解析和计算触发时间。该库仅负责表达式解析和 `next_after` 计算，调度引擎自研。

```rust
use cron::Schedule;
use std::str::FromStr;

pub struct CronSchedule {
    schedule: Schedule,
    source: String,
}

impl CronSchedule {
    pub fn parse(expression: &str) -> Result<Self, CronError> {
        let schedule = Schedule::from_str(expression)
            .map_err(|e| CronError::InvalidSchedule(e.to_string()))?;
        Ok(Self {
            schedule,
            source: expression.to_string(),
        })
    }

    /// 计算下一次触发时间
    pub fn next_after(&self, from: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.schedule.after(&from).next()
    }

    /// 计算 upcoming N 次触发时间
    pub fn upcoming(&self, from: DateTime<Utc>, n: usize) -> Vec<DateTime<Utc>> {
        self.schedule.after(&from).take(n).collect()
    }
}
```

**Cron 表达式格式**：标准 Unix cron，7 个字段（含秒）：

```
sec  min  hour  day_of_month  month  day_of_week  year
0    0    9     *             *      1-5          *
```

常用示例：

| 表达式 | 含义 |
|--------|------|
| `0 0 9 * * *` | 每天上午 9:00 |
| `0 0 9 * * 1-5` | 工作日（周一至周五）上午 9:00 |
| `0 */30 * * * *` | 每 30 分钟 |
| `0 0 0 1 * *` | 每月 1 号午夜 |

---

## 5. 存储层设计

### 5.1 CronStore Trait

```rust
#[async_trait]
pub trait CronStore: Send + Sync {
    /// 创建任务
    async fn create(&self, job: &CronJob) -> Result<()>;
    /// 获取单个任务
    async fn get(&self, id: &CronJobId) -> Result<Option<CronJob>>;
    /// 列出任务（可按状态过滤）
    async fn list(&self, status: Option<CronJobStatus>, limit: usize) -> Result<Vec<CronJob>>;
    /// 更新任务（部分更新）
    async fn update(&self, id: &CronJobId, input: &UpdateCronJobInput) -> Result<bool>;
    /// 删除任务
    async fn delete(&self, id: &CronJobId) -> Result<bool>;
    /// 获取所有 active 任务（供 scheduler 加载）
    async fn list_active(&self) -> Result<Vec<CronJob>>;
    /// 原子更新执行记录（run_count++, last_run_at, next_run_at, last_error）
    async fn record_execution(
        &self,
        id: &CronJobId,
        next_run: Option<DateTime<Utc>>,
        error: Option<String>,
    ) -> Result<()>;
}
```

### 5.2 SQLite Schema

Migration version 6：

```sql
CREATE TABLE cron_jobs (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    schedule TEXT NOT NULL,
    action TEXT NOT NULL,         -- JSON serialized CronAction
    status TEXT NOT NULL CHECK(status IN ('active', 'paused', 'completed', 'failed')),
    created_at TEXT NOT NULL,     -- RFC3339
    updated_at TEXT NOT NULL,     -- RFC3339
    next_run_at TEXT,             -- RFC3339, NULL if no upcoming
    last_run_at TEXT,             -- RFC3339
    run_count INTEGER NOT NULL DEFAULT 0,
    max_runs INTEGER,
    expires_at TEXT,              -- RFC3339
    last_error TEXT
);

CREATE INDEX idx_cron_jobs_status_next_run
    ON cron_jobs(status, next_run_at);

CREATE INDEX idx_cron_jobs_next_run_active
    ON cron_jobs(next_run_at) WHERE status = 'active';
```

### 5.3 接入 StorageSet

```rust
// storage/init.rs
pub struct StorageSet {
    // ... existing fields ...
    cron_store: Arc<dyn CronStore>,
}

impl StorageSet {
    pub async fn open(...) -> Result<Self> {
        // ... existing setup ...
        let cron_store: Arc<dyn CronStore> =
            Arc::new(crate::cron::SqliteCronStore::new(pool.clone()));
        
        Ok(Self {
            // ... existing ...
            cron_store,
        })
    }
    
    pub fn cron_store(&self) -> Arc<dyn CronStore> {
        self.cron_store.clone()
    }
}
```

---

## 6. 调度引擎设计

### 6.1 CronScheduler（自研轻量调度引擎）

不引入外部 scheduler 库（如 `tokio-cron-scheduler`），自研约 150 行调度核心：

```rust
pub struct CronScheduler {
    store: Arc<dyn CronStore>,
    task_tx: mpsc::Sender<CronJob>,
    /// 按 next_run_at 排序的优先队列
    queue: Arc<RwLock<BTreeMap<DateTime<Utc>, CronJobId>>>,
    /// 任务 ID -> 完整 CronJob 的缓存
    jobs: Arc<RwLock<HashMap<CronJobId, CronJob>>>,
    /// 有新任务加入时通知调度循环重新计算
    notify: Arc<tokio::sync::Notify>,
    shutdown: CancellationToken,
}

impl CronScheduler {
    pub fn new(
        store: Arc<dyn CronStore>,
        task_tx: mpsc::Sender<CronJob>,
        shutdown: CancellationToken,
    ) -> Self { ... }

    /// 启动调度主循环
    pub async fn run(self: Arc<Self>) {
        // 1. 从 DB 加载所有 active 任务，计算 next_run_at
        self.load_jobs().await;

        loop {
            let sleep_until = {
                let queue = self.queue.read().await;
                queue.first_key_value()
                    .map(|(t, _)| *t)
                    .unwrap_or_else(|| Utc::now() + Duration::from_secs(60))
            };

            tokio::select! {
                biased;
                () = self.shutdown.cancelled() => break,
                () = tokio::time::sleep(sleep_until - Utc::now()) => {
                    self.fire_due_jobs().await;
                }
                () = self.notify.notified() => {
                    self.load_jobs().await;
                }
            }
        }
    }

    /// 外部调用：任务变更后通知调度器重新加载
    pub fn reload(&self) {
        self.notify.notify_one();
    }

    async fn load_jobs(&self) { ... }
    async fn fire_due_jobs(&self) { ... }
}
```

### 6.2 为什么自研调度引擎

调研了 [`tokio-cron-scheduler`](https://github.com/mvniekerk/tokio-cron-scheduler)，但评估后不适合：

| 问题 | 说明 |
|------|------|
| 存储模型不匹配 | 它的 `MetaDataStorage` 存储 `JobStoredData`（protobuf），与我们的 `CronJob` 业务模型差异大 |
| 闭包不可持久化 | `Job::new_async` 接收闭包，Server 重启后无法恢复，需额外映射层 |
| 依赖过重 | 内部使用 actor 模型（JobCreator/JobDeleter/JobRunner...），引入不必要复杂度 |
| 轮询精度 | 默认 500ms tick 检查，非精确睡眠唤醒 |

自研方案仅约 150 行，完全可控，直接使用 SQLite 持久化。

### 6.3 调度算法

1. **初始化**：从 `cron_jobs` 表加载 `status = 'active'` 的所有任务，用 `cron` crate 计算每个任务的 `next_run_at`，写入数据库。
2. **主循环**：
   - 维护一个按 `next_run_at` 排序的优先队列（`BTreeMap<DateTime, CronJobId>`）。
   - 计算到最近任务的等待时间 `sleep_duration`。
   - `tokio::select!` 等待：
     - `tokio::time::sleep(sleep_duration)` — 精确睡眠到触发时间
     - `notify.notified()` — 有新任务加入/更新，打断 sleep 重新加载
     - `shutdown.cancelled()` — Server 关闭
   - 触发时：将任务 clone 后通过 `task_tx` 发送给 Worker，然后重新计算该任务的 `next_run_at`。
3. **动态变更**：RPC 创建/更新/删除任务后，调用 `scheduler.reload()`，打断当前 sleep 重新加载。

### 6.4 时间精度

- 默认秒级精度（cron 表达式支持秒字段）。
- 系统休眠恢复后，检查是否有"漏执行"的任务（`next_run_at < now`），可选择立即补发或跳过（**策略：立即补发一次**）。

---

## 7. 执行器设计

### 7.1 CronWorker

```rust
pub struct CronWorker {
    coordinator: Arc<Kernel>,
    task_rx: mpsc::Receiver<CronJob>,
    store: Arc<dyn CronStore>,
}

impl CronWorker {
    pub async fn run(mut self) {
        while let Some(job) = self.task_rx.recv().await {
            let coordinator = Arc::clone(&self.coordinator);
            let store = Arc::clone(&self.store);
            
            tokio::spawn(async move {
                let start = Instant::now();
                let result = Self::execute(&coordinator, &job).await;
                let elapsed = start.elapsed();
                
                // 记录执行结果
                let (next_run, error) = match &result {
                    Ok(_) => {
                        let schedule = CronSchedule::parse(&job.schedule).ok();
                        let next = schedule.and_then(|s| s.next_after(Utc::now()));
                        (next, None)
                    }
                    Err(e) => {
                        tracing::error!("Cron job {} failed: {}", job.id.0, e);
                        (None, Some(e.to_string()))
                    }
                };
                
                if let Err(e) = store.record_execution(&job.id, next_run, error).await {
                    tracing::error!("Failed to record cron execution: {}", e);
                }
                
                tracing::info!(
                    "Cron job {} executed in {:?}: {:?}",
                    job.id.0, elapsed, result
                );
            });
        }
    }

    async fn execute(coordinator: &Kernel, job: &CronJob) -> Result<(), CronError> {
        match &job.action {
            CronAction::SendMessage { session_id, content } => {
                let sid = SessionId(session_id.clone());
                // 如果 session 不在内存，尝试恢复
                if coordinator.get_session(&sid).is_none() {
                    coordinator.restore_session(&sid).await?;
                }
                let text = Self::render_template(content);
                let blocks = vec![ContentBlock::Text { text }];
                coordinator.send_message(&sid, blocks).await?;
            }
            CronAction::Shell { command, working_dir } => {
                let output = tokio::process::Command::new("sh")
                    .arg("-c")
                    .arg(command)
                    .current_dir(working_dir.as_deref().unwrap_or("."))
                    .output()
                    .await?;
                if !output.status.success() {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    return Err(CronError::ShellFailed(stderr.to_string()));
                }
            }
            CronAction::Internal { .. } => {
                return Err(CronError::UnsupportedAction("Internal".to_string()));
            }
        }
        Ok(())
    }

    /// 简单模板渲染
    fn render_template(template: &str) -> String {
        let now = Utc::now();
        template
            .replace("{{timestamp}}", &now.to_rfc3339())
            .replace("{{date}}", &now.format("%Y-%m-%d").to_string())
            .replace("{{time}}", &now.format("%H:%M:%S").to_string())
    }
}
```

### 7.2 执行策略

| 策略 | 说明 |
|------|------|
| 并发 | 每个任务独立 spawn，互不阻塞 |
| 超时 | 单次执行默认 5 分钟超时（可配置） |
| 重试 | 执行失败不自动重试，记录错误到 `last_error` |
| Session 恢复 | `SendMessage` 自动 `restore_session`，恢复失败则记录错误 |

---

## 8. Wire Protocol 扩展

### 8.1 RequestMethod 新增

```rust
pub enum RequestMethod {
    // ... existing ...

    // ── Cron Job ──────────────────────────────────────────────────
    CreateCronJob {
        name: String,
        schedule: String,       // cron 表达式
        action: CronAction,
        max_runs: Option<u32>,
        expires_at: Option<DateTime<Utc>>,
    },
    ListCronJobs {
        status: Option<String>, // "active" | "paused" | "completed" | "failed"
        limit: usize,
    },
    GetCronJob {
        job_id: String,
    },
    UpdateCronJob {
        job_id: String,
        name: Option<String>,
        schedule: Option<String>,
        action: Option<CronAction>,
        status: Option<String>,
        max_runs: Option<u32>,
        expires_at: Option<DateTime<Utc>>,
    },
    DeleteCronJob {
        job_id: String,
    },
}
```

### 8.2 RPC 响应格式

- `CreateCronJob` → `Ok { job_id: String }`
- `ListCronJobs` → `Ok { jobs: Vec<CronJob> }`
- `GetCronJob` → `Ok { job: CronJob }`
- `UpdateCronJob` → `Ok {}`
- `DeleteCronJob` → `Ok {}`

---

## 9. KernelServer 集成

### 9.1 构造函数变更

```rust
pub struct KernelServer {
    coordinator: Arc<Kernel>,
    config_file_path: Option<PathBuf>,
    base_dir: PathBuf,
    reload_lock: Arc<tokio::sync::Mutex<()>>,
    connections: Arc<dashmap::DashMap<u64, CancellationToken>>,
    next_conn_id: Arc<AtomicU64>,
    // NEW:
    cron_scheduler: Option<Arc<CronScheduler>>,
}

impl KernelServer {
    pub fn new(
        coordinator: Arc<Kernel>,
        config_file_path: Option<PathBuf>,
        base_dir: PathBuf,
        cron_store: Option<Arc<dyn CronStore>>,  // NEW
    ) -> Self {
        let cron_scheduler = cron_store.map(|store| {
            let (task_tx, task_rx) = mpsc::channel(64);
            let shutdown = CancellationToken::new();
            let scheduler = Arc::new(CronScheduler::new(store.clone(), task_tx, shutdown));
            
            // Spawn scheduler
            let sched_clone = Arc::clone(&scheduler);
            tokio::spawn(async move { sched_clone.run().await });
            
            // Spawn worker
            let worker = CronWorker::new(Arc::clone(&coordinator), task_rx, store);
            tokio::spawn(async move { worker.run().await });
            
            scheduler
        });
        
        Self { ..., cron_scheduler }
    }
}
```

### 9.2 dispatch_request 新增分支

```rust
async fn dispatch_request(...) -> ResponseBody {
    match method {
        // ... existing ...
        
        RequestMethod::CreateCronJob { name, schedule, action, max_runs, expires_at } => {
            // 1. 验证 cron 表达式
            // 2. 创建 CronJob，计算 next_run_at
            // 3. 写入 store
            // 4. 通知 scheduler reload
            // 5. 返回 job_id
        }
        RequestMethod::ListCronJobs { status, limit } => { ... }
        RequestMethod::GetCronJob { job_id } => { ... }
        RequestMethod::UpdateCronJob { job_id, ... } => {
            // 更新后通知 scheduler reload
        }
        RequestMethod::DeleteCronJob { job_id } => {
            // 删除后通知 scheduler reload
        }
    }
}
```

### 9.3 优雅关闭

```rust
pub async fn shutdown(&self) {
    // 关闭所有连接
    for entry in self.connections.iter() {
        entry.value().cancel();
    }
    // 关闭 cron scheduler
    if let Some(ref scheduler) = self.cron_scheduler {
        scheduler.shutdown.cancel();
    }
}
```

---

## 10. 错误处理

```rust
#[derive(Debug, thiserror::Error)]
pub enum CronError {
    #[error("Invalid cron schedule: {0}")]
    InvalidSchedule(String),
    
    #[error("Job not found: {0}")]
    JobNotFound(String),
    
    #[error("Shell command failed: {0}")]
    ShellFailed(String),
    
    #[error("Session error: {0}")]
    Session(#[from] crate::types::KernelError),
    
    #[error("Unsupported action: {0}")]
    UnsupportedAction(String),
    
    #[error("Storage error: {0}")]
    Storage(String),
}
```

---

## 11. 依赖变更

### 11.1 Workspace 新增

```toml
# Cargo.toml [workspace.dependencies]
cron = "0.15"   # zslayton/cron — cron 表达式解析与 next_after 计算
```

### 11.2 kernel crate 新增

```toml
# crates/kernel/Cargo.toml [dependencies]
cron = { workspace = true }
```

> **注意**：仅引入 `cron` crate 做表达式解析，不引入 `tokio-cron-scheduler` 等完整调度库。调度引擎自研（约 150 行），完全可控。

---

## 12. 实现计划

| 步骤 | 内容 | 涉及文件 |
|------|------|----------|
| 1 | 新增 `cron` workspace dependency | `Cargo.toml` |
| 2 | 创建 `cron/` 模块骨架 + types | `src/cron/mod.rs`, `src/cron/types.rs` |
| 3 | 实现 SQLite store + migration | `src/cron/store.rs`, `src/storage/migrations.rs` |
| 4 | 实现 Scheduler 调度引擎 | `src/cron/scheduler.rs` |
| 5 | 实现 Worker 执行层 | `src/cron/worker.rs` |
| 6 | 扩展 Wire protocol | `src/wire.rs` |
| 7 | 接入 KernelServer RPC dispatch | `src/server/mod.rs` |
| 8 | 接入 StorageSet | `src/storage/init.rs` |
| 9 | 导出公共类型 | `src/lib.rs` |
| 10 | 编写单元测试 | `src/cron/tests.rs` |

---

## 13. 测试策略

1. **Cron 解析测试**：验证各种 cron 表达式的 `next_after` 计算正确。
2. **Store 测试**：CRUD 操作、并发读写、record_execution 原子性。
3. **Scheduler 测试**：
   - 模拟时间，验证任务在正确时间点被触发。
   - 验证动态添加/删除/更新任务后调度正确。
   - 验证 Server 重启后从 DB 恢复调度。
4. **Worker 测试**：
   - Mock Kernel，验证 `SendMessage` action 正确调用。
   - 验证 Shell action 命令执行和错误处理。
5. **集成测试**：通过 RPC 创建任务，验证端到端触发。

---

## 14. 未来扩展

- [ ] **任务链**：支持一个任务触发后链式执行多个 action。
- [ ] **条件触发**：支持根据环境变量、文件状态等条件决定是否执行。
- [ ] **Webhook action**：支持 HTTP POST 调用外部服务。
- [ ] **执行历史**：独立的 `cron_job_runs` 表记录每次执行的详细日志。
- [ ] **失败重试**：支持配置重试次数和退避策略。
- [ ] **任务分组/标签**：按项目或标签管理任务。
