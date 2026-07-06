# Yomi Project 概念引入设计文档

> **原则**：
> 1. `working_dir` 保留，优先级高于 `project.dir`。`project_id` 与 `working_dir` 均可独立存在、同时存在，也可同时为空。
> 2. GUI 不推翻重来，保持类似现有结构（树形/分组展示）。Session 分页改用 `updated_at` cursor，Project 列表按 `updated_at` 排序暂不分页。

---

## 1. 核心原则

1. **Project 是可选标签，不是容器**：Session 可以独立存在（`project_id = NULL`），也可以挂到 Project 下。
2. **工作目录来源**（无默认值 fallback）：
   ```
   session级 working_dir > project.dir > (无)
   ```
   - 只传 `working_dir` → 用它
   - 只传 `project_id` → 回退到 `project.dir`
   - 两者都不传 → **无工作目录**（agent prompt 不显示 cwd）
   - 两者都传 → `working_dir` 覆盖
3. **数据最小侵入**：Migration 只加 `sessions.project_id` 列，不删 `working_dir`，不强制反向生成 Project。
4. **分页策略**：
   - Session 列表：`updated_at` cursor 分页（传 `before` + `limit`）
   - Project 列表：全量，`updated_at DESC` 排序，暂不分页

---

## 2. 数据模型（kernel）

### 2.1 Project 类型

```rust
// crates/kernel/src/types.rs
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ProjectId(pub String); // UUID v7

impl ProjectId {
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7().to_string())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: ProjectId,
    pub name: String,              // 用户自定义，默认取 dir 的文件名
    pub dir: std::path::PathBuf,   // 绝对路径（创建时 normalize）
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}
```

### 2.2 CreateSessionInput（新增）

```rust
// crates/kernel/src/app/coordinator.rs（或 types.rs）
#[derive(Debug, Clone)]
pub struct CreateSessionInput {
    pub project_id: Option<ProjectId>,
    pub working_dir: Option<std::path::PathBuf>,
    pub auto_approve_level: crate::permissions::Level,
}
```

### 2.3 SessionInfo（改造）

```rust
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionInfo {
    pub id: SessionId,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    pub parent_id: Option<SessionId>,
    pub title: Option<String>,
    pub message_count: i64,
    pub project_id: Option<ProjectId>,   // 【新增】可选
    pub working_dir: Option<String>,       // 【保留】可选，运行时优先级最高
}
```

### 2.4 SessionConfig（改造）

```rust
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub agent: AgentConfig,
    pub project: Option<Project>,              // 【新增】可选
    pub working_dir: Option<std::path::PathBuf>, // 【保留】可选，优先级高于 project.dir
    pub auto_approve_level: Level,
    pub data_dir: std::path::PathBuf,
}
```

---

## 3. 存储层（kernel/storage）

### 3.1 ProjectStore Trait（新增）

```rust
// crates/kernel/src/storage/project/mod.rs
#[async_trait::async_trait]
pub trait ProjectStore: Send + Sync {
    async fn create(&self, id: &ProjectId, name: &str, dir: &str) -> crate::types::Result<()>;
    async fn get(&self, id: &ProjectId) -> crate::types::Result<Option<Project>>;
    async fn get_by_dir(&self, dir: &str) -> crate::types::Result<Option<Project>>;
    async fn list(&self) -> crate::types::Result<Vec<Project>>; // 默认 updated_at DESC
    async fn update_name(&self, id: &ProjectId, name: &str) -> crate::types::Result<()>;
    async fn touch(&self, id: &ProjectId) -> crate::types::Result<()>;
    async fn delete(&self, id: &ProjectId) -> crate::types::Result<()>;
}

pub mod sqlite;
pub use sqlite::SqliteProjectStore;
```

### 3.2 SessionStore Trait（改造）

```rust
#[async_trait::async_trait]
pub trait SessionStore: Send + Sync {
    /// 【改造】project_id 和 working_dir 均可为 None
    async fn create(
        &self,
        id: &SessionId,
        project_id: Option<&ProjectId>,
        working_dir: Option<&str>,
    ) -> crate::types::Result<()>;

    async fn fork(&self, parent_id: &SessionId) -> crate::types::Result<SessionId>;
    async fn get(&self, id: &SessionId) -> crate::types::Result<Option<SessionInfo>>;
    async fn delete(&self, id: &SessionId) -> crate::types::Result<()>;

    /// 【改造】Cursor 分页：传 before (updated_at) + limit
    /// None = 全部（含独立 session）
    async fn list(
        &self,
        project_id: Option<&ProjectId>,
        before: Option<chrono::DateTime<chrono::Utc>>,
        limit: usize,
    ) -> crate::types::Result<(Vec<SessionInfo>, bool)>;

    async fn update_message_count(&self, id: &SessionId, count: i64) -> crate::types::Result<()>;
    async fn update_title(&self, id: &SessionId, title: &str) -> crate::types::Result<()>;
    async fn cleanup(&self, days: i64) -> crate::types::Result<Vec<SessionId>>;
}
```

### 3.3 SQLite Schema（Migration v4）

```sql
-- 1. 新增 projects 表
CREATE TABLE projects (
    id         TEXT PRIMARY KEY,
    name       TEXT NOT NULL,
    dir        TEXT NOT NULL UNIQUE,
    created_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE UNIQUE INDEX idx_projects_dir ON projects(dir);

-- 2. sessions 表只加一个列（不重建，不删 working_dir）
ALTER TABLE sessions ADD COLUMN project_id TEXT;
CREATE INDEX idx_sessions_project_id ON sessions(project_id);
```

> **旧数据零迁移**：已有 `working_dir` 的 session 保持原样，`project_id = NULL`。

### 3.4 SqliteProjectStore 实现要点

```rust
// crates/kernel/src/storage/project/sqlite.rs
use sqlx::sqlite::SqlitePool;

pub struct SqliteProjectStore { pool: SqlitePool }

impl SqliteProjectStore {
    pub fn new(pool: SqlitePool) -> Self { Self { pool } }
}

#[async_trait::async_trait]
impl ProjectStore for SqliteProjectStore {
    async fn create(&self, id: &ProjectId, name: &str, dir: &str) -> Result<()> {
        sqlx::query("INSERT INTO projects (id, name, dir) VALUES (?, ?, ?)")
            .bind(&id.0).bind(name).bind(dir)
            .execute(&self.pool).await.map_err(...)?;
        Ok(())
    }

    async fn get(&self, id: &ProjectId) -> Result<Option<Project>> {
        let row = sqlx::query_as::<_, ProjectRow>("SELECT * FROM projects WHERE id = ?")
            .bind(&id.0).fetch_optional(&self.pool).await.map_err(...)?;
        Ok(row.map(Into::into))
    }

    async fn list(&self) -> Result<Vec<Project>> {
        let rows = sqlx::query_as::<_, ProjectRow>(
            "SELECT * FROM projects ORDER BY updated_at DESC"
        ).fetch_all(&self.pool).await.map_err(...)?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn update_name(&self, id: &ProjectId, name: &str) -> Result<()> {
        sqlx::query("UPDATE projects SET name = ?, updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(name).bind(&id.0).execute(&self.pool).await.map_err(...)?;
        Ok(())
    }

    async fn touch(&self, id: &ProjectId) -> Result<()> {
        sqlx::query("UPDATE projects SET updated_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(&id.0).execute(&self.pool).await.map_err(...)?;
        Ok(())
    }

    async fn delete(&self, id: &ProjectId) -> Result<()> {
        sqlx::query("DELETE FROM projects WHERE id = ?")
            .bind(&id.0).execute(&self.pool).await.map_err(...)?;
        Ok(())
    }
}

#[derive(sqlx::FromRow)]
struct ProjectRow {
    id: String, name: String, dir: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
}

impl From<ProjectRow> for Project {
    fn from(r: ProjectRow) -> Self {
        Self { id: ProjectId(r.id), name: r.name, dir: r.dir.into(), created_at: r.created_at, updated_at: r.updated_at }
    }
}
```

### 3.5 SqliteSessionStore 关键 SQL 改造

**`SessionRow` 新增 `project_id` 字段**：

```rust
#[derive(sqlx::FromRow)]
struct SessionRow {
    id: String,
    created_at: chrono::DateTime<chrono::Utc>,
    updated_at: chrono::DateTime<chrono::Utc>,
    parent_id: Option<String>,
    title: Option<String>,
    message_count: i64,
    working_dir: Option<String>,
    project_id: Option<String>,   // 【新增】
}

impl From<SessionRow> for SessionInfo {
    fn from(r: SessionRow) -> Self {
        Self {
            id: SessionId(r.id),
            created_at: r.created_at, updated_at: r.updated_at,
            parent_id: r.parent_id.map(SessionId),
            title: r.title, message_count: r.message_count,
            working_dir: r.working_dir,
            project_id: r.project_id.map(ProjectId),   // 【新增】
        }
    }
}
```

**`create` SQL**：
```rust
sqlx::query("INSERT INTO sessions (id, project_id, working_dir) VALUES (?, ?, ?)")
    .bind(&id.0)
    .bind(project_id.map(|p| &p.0))
    .bind(working_dir)
    .execute(&self.pool).await?;
```

**`fork` SQL**（复制 parent 的 `project_id` + `working_dir`）：
```rust
let new_id = SessionId::new();
sqlx::query(
    "INSERT INTO sessions (id, parent_id, project_id, working_dir)
     SELECT ?, ?, project_id, working_dir FROM sessions WHERE id = ?"
)
.bind(&new_id.0)
.bind(&parent_id.0)
.bind(&parent_id.0)
.execute(&self.pool).await?;
```

**`list` SQL**（cursor 分页，`has_more` 通过 `limit + 1` 判断）：
```rust
async fn list(
    &self,
    project_id: Option<&ProjectId>,
    before: Option<chrono::DateTime<chrono::Utc>>,
    limit: usize,
) -> Result<(Vec<SessionInfo>, bool)> {
    let mut conditions = vec!["1=1"];
    let mut binds: Vec<Box<dyn sqlx::Encode<'_, sqlx::Sqlite>>> = Vec::new();
    // 注意：sqlx 动态绑定较麻烦，实际实现中可用 format! 构建 SQL
    // 以下为伪代码：
    let mut query = format!(
        "SELECT id, created_at, updated_at, parent_id, title, message_count, working_dir, project_id
         FROM sessions WHERE {} ORDER BY updated_at DESC LIMIT {}",
        conditions.join(" AND "),
        limit + 1   // 多查一条判断是否还有更多
    );
    // ... 执行查询，如果结果数 > limit，截断并返回 has_more=true
}
```

---

## 4. StorageSet 改造

```rust
// crates/kernel/src/storage/mod.rs
pub struct StorageSet {
    session_store: Arc<dyn SessionStore>,
    message_store: Arc<dyn MessageStore>,
    todo_store: Arc<dyn TodoStore>,
    usage_store: Arc<dyn UsageStore>,
    checkpoint_store: Arc<dyn CheckpointStore>,
    project_store: Arc<dyn ProjectStore>,   // 【新增】
    data_dir: PathBuf,
}

impl StorageSet {
    pub async fn open_with_config(data_dir: &Path, _config: &Config) -> Result<Self> {
        // ... 现有初始化 ...
        let project_store: Arc<dyn ProjectStore> = Arc::new(SqliteProjectStore::new(pool.clone()));
        // ...
    }

    pub fn project_store(&self) -> Arc<dyn ProjectStore> {
        Arc::clone(&self.project_store)
    }

    // ... 其余不变
}
```

---

## 5. 业务逻辑层（kernel/app）

### 5.1 SessionConfig + AgentSpawnArgs 改造

> **关键修正**：`AgentSpawnArgs::new()` 默认 `working_dir = current_dir()`。要支持"无工作目录"，**将默认值改为空 `PathBuf::new()`**，并在 `Session::init` 中仅在 `resolve_cwd` 返回 `Some` 时调用 `with_working_dir`。

```rust
// crates/kernel/src/agent/types.rs
impl AgentSpawnArgs {
    pub fn new(base_prompt: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            // ...
            working_dir: std::path::PathBuf::new(),   // 【改】从 current_dir() 改为空
            // ...
        }
    }
}
```

```rust
// crates/kernel/src/app/session.rs
impl Session {
    pub(crate) async fn init(
        id: SessionId,
        config: SessionConfig,
        agent_shared: Arc<AgentShared>,
    ) -> Result<(Self, mpsc::Receiver<Event>)> {
        let file_state_store = Self::create_file_state_store(&id, &config).await?;
        let goal_store: Arc<dyn GoalStore> = Arc::new(JsonGoalStore::new(&config.data_dir));
        let permission_state = Self::create_permission_state(&config);

        let (main_agent, event_rx) = Self::spawn_main_agent(
            &id, &config, &agent_shared, &file_state_store, &goal_store, permission_state.clone(),
        ).await?;
        // ...
    }

    async fn spawn_main_agent(
        id: &SessionId,
        config: &SessionConfig,
        // ...
    ) -> Result<(AgentHandle, mpsc::Receiver<Event>)> {
        let history = /* ... */;
        let goal_state = /* ... */;

        let mut spawn_args = AgentSpawnArgs::new(
            config.agent.system_prompt.clone(), id.0.clone(),
        )
        .with_skills(config.agent.skills.clone())
        .with_history(history)
        .with_max_iterations(config.agent.max_iterations)
        .with_subagent(config.agent.enable_subagent)
        .with_file_state_store(Arc::clone(file_state_store));

        // 【改造】仅在 resolve_cwd 返回 Some 时设置 working_dir
        if let Some(cwd) = resolve_cwd(config) {
            spawn_args = spawn_args.with_working_dir(cwd);
        }

        // ... 后续不变
    }
}

/// 统一解析 SessionConfig → 实际工作目录
fn resolve_cwd(config: &SessionConfig) -> Option<PathBuf> {
    config.working_dir.clone()
        .or_else(|| config.project.as_ref().map(|p| p.dir.clone()))
    // 两者都无 → None，agent prompt 不显示 cwd（因为 AgentSpawnArgs 默认空 PathBuf）
}
```

### 5.2 Kernel 新增 Project API

```rust
impl Kernel {
    pub async fn create_project(
        &self,
        dir: PathBuf,
        name: Option<String>,
    ) -> Result<Project> {
        let abs = std::fs::canonicalize(&dir).unwrap_or(dir);
        let name = name.unwrap_or_else(|| {
            abs.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unnamed")
                .to_string()
        });
        let id = ProjectId::new();
        self.project_store.create(&id, &name, abs.to_str().unwrap()).await?;
        Ok(Project {
            id, name, dir: abs,
            created_at: Utc::now(), updated_at: Utc::now(),
        })
    }

    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        self.project_store.list().await
    }

    pub async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>> {
        self.project_store.get(id).await
    }

    pub async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()> {
        self.project_store.update_name(id, &name).await
    }

    pub async fn delete_project(&self, id: &ProjectId) -> Result<()> {
        let (sessions, _) = self.session_store.list(Some(id), None, 1).await?;
        if !sessions.is_empty() {
            return Err(KernelError::Session(SessionError::Other(
                format!("Project {} has sessions, remove or reassign them first", id.0)
            )));
        }
        self.project_store.delete(id).await
    }
}
```

### 5.3 Kernel Session 创建

```rust
impl Kernel {
    pub async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId> {
        let project = match &input.project_id {
            Some(pid) => Some(
                self.project_store.get(pid).await?
                    .ok_or_else(|| SessionError::Other(format!("Project {} not found", pid.0)))?
            ),
            None => None,
        };

        let working_dir = input.working_dir.map(|p| {
            std::fs::canonicalize(&p).unwrap_or(p).to_string_lossy().to_string()
        });

        let id = SessionId::new();
        self.session_store.create(
            &id,
            input.project_id.as_ref(),
            working_dir.as_deref(),
        ).await?;

        let config = SessionConfig {
            agent: self.agent_config.read().await.clone(),
            project,
            working_dir: working_dir.map(PathBuf::from),
            auto_approve_level: input.auto_approve_level,
            data_dir: self.data_dir().clone(),
        };

        if let Err(e) = self.init_session(id.clone(), config).await {
            let _ = self.session_store.delete(&id).await;
            return Err(e);
        }

        if let Some(ref pid) = input.project_id {
            let _ = self.project_store.touch(pid).await;
        }
        Ok(id)
    }
}
```

### 5.4 Kernel 恢复 / Fork / 列表

```rust
/// 恢复 Session
pub async fn restore_session(
    &self, session_id: &SessionId, auto_approve_level: Level,
) -> Result<SessionId> {
    if self.get_session(session_id).is_some() {
        return Ok(session_id.clone());
    }
    let info = self.session_store.get(session_id).await?
        .ok_or_else(|| SessionError::NotFound { session_id: session_id.0.clone() })?;

    let project = match &info.project_id {
        Some(pid) => self.project_store.get(pid).await?,
        None => None,
    };
    let working_dir = info.working_dir.map(PathBuf::from);

    let config = SessionConfig {
        agent: self.agent_config.read().await.clone(),
        project, working_dir,
        auto_approve_level,
        data_dir: self.data_dir().clone(),
    };
    self.init_session(info.id, config).await
}

/// Fork：继承 parent 的 project_id 和 working_dir
pub async fn fork_session(
    &self, parent_id: &SessionId, auto_approve_level: Level,
) -> Result<SessionId> {
    let parent_info = self.session_store.get(parent_id).await?
        .ok_or_else(|| SessionError::NotFound { session_id: parent_id.0.clone() })?;

    let new_id = self.session_store.fork(parent_id).await?;

    let project = match &parent_info.project_id {
        Some(pid) => self.project_store.get(pid).await?,
        None => None,
    };

    let config = SessionConfig {
        agent: self.agent_config.read().await.clone(),
        project,
        working_dir: parent_info.working_dir.map(PathBuf::from),
        auto_approve_level,
        data_dir: self.data_dir().clone(),
    };

    if let Err(e) = self.init_session(new_id.clone(), config).await {
        let _ = self.session_store.delete(&new_id).await;
        return Err(e);
    }
    Ok(new_id)
}

/// Session 列表（cursor 分页）
pub async fn list_sessions(
    &self,
    project_id: Option<&ProjectId>,
    before: Option<DateTime<Utc>>,
    limit: usize,
) -> Result<(Vec<SessionInfo>, bool)> {
    self.session_store.list(project_id, before, limit).await
}
```

### 5.5 Kernel::new 接收 StorageSet

```rust
impl Kernel {
    pub fn new(
        storage: &StorageSet,
        provider: Arc<dyn Provider>,
        agent_config: AgentConfig,
        // ... 其余不变
    ) -> Self {
        let project_store = storage.project_store();
        // ...
        Self {
            // ...
            project_store,
            // ...
        }
    }
}
```

---

## 6. 传输协议层（kernel/wire）

### 6.1 Wire Protocol v3

```rust
pub const WIRE_PROTOCOL_VERSION: u32 = 3;

pub enum RequestMethod {
    // -------------- Project --------------
    ListProjects,
    CreateProject { dir: String, name: Option<String> },
    GetProject { project_id: String },
    RenameProject { project_id: String, name: String },
    DeleteProject { project_id: String },

    // -------------- Session --------------
    CreateSession {
        project_id: Option<String>,       // 可选
        working_dir: Option<String>,       // 可选，优先级高于 project.dir
        auto_approve_level: Level,
    },
    RestoreSession {
        session_id: String,
        auto_approve_level: Level,
    },
    ForkSession {
        parent_id: String,
        auto_approve_level: Level,
    },
    ListSessions {
        project_id: Option<String>,       // None = 全部
        before: Option<chrono::DateTime<chrono::Utc>>,
        limit: usize,
    },

    SendMessage { session_id: String, blocks: Vec<ContentBlock> },
    Command { session_id: String, cmd: ControlCommand },
    Subscribe { session_id: String, auto_approve_level: Level },
    Unsubscribe { session_id: String },
    GetSessionMessages { session_id: String },
    ShutdownSession { session_id: String },
    DeleteSession { session_id: String },

    // -------------- 其余不变 --------------
    Hello,
    GetCheckpoints { session_id: String },
    GetTodos { session_id: String },
    ReloadAgentConfig,
}
```

### 6.2 KernelApi Trait（扩展）

```rust
#[async_trait::async_trait]
pub trait KernelApi: Send + Sync {
    // --- Project ---
    async fn list_projects(&self) -> Result<Vec<Project>>;
    async fn create_project(&self, dir: PathBuf, name: Option<String>) -> Result<Project>;
    async fn get_project(&self, id: &ProjectId) -> Result<Option<Project>>;
    async fn rename_project(&self, id: &ProjectId, name: String) -> Result<()>;
    async fn delete_project(&self, id: &ProjectId) -> Result<()>;

    // --- Session ---
    async fn create_session(&self, input: CreateSessionInput) -> Result<SessionId>;
    async fn restore_session(&self, id: &SessionId, level: Level) -> Result<SessionId>;
    async fn fork_session(&self, parent: &SessionId, level: Level) -> Result<SessionId>;
    async fn list_sessions(
        &self, project_id: Option<&ProjectId>,
        before: Option<DateTime<Utc>>, limit: usize,
    ) -> Result<(Vec<SessionInfo>, bool)>;

    // 其余 Session 操作不变（send_message, cancel, etc.）
    async fn send_message(&self, session_id: &SessionId, blocks: Vec<ContentBlock>) -> Result<()>;
    async fn cancel(&self, session_id: &SessionId) -> Result<()>;
    async fn send_permission_response(&self, session_id: &SessionId, req_id: &str, approved: bool, remember: bool) -> Result<()>;
    async fn set_permission_level(&self, session_id: &SessionId, level: Level) -> Result<()>;
    async fn compact_session(&self, session_id: &SessionId) -> Result<()>;
    async fn rewind_session(&self, session_id: &SessionId, message_id: MessageId, target: RewindTarget) -> Result<()>;
    async fn start_goal(&self, session_id: &SessionId, state: GoalState) -> Result<()>;
    async fn stop_goal(&self, session_id: &SessionId) -> Result<()>;
    async fn delete_session(&self, session_id: &SessionId) -> Result<()>;
    async fn get_session_messages(&self, session_id: &SessionId) -> Result<Vec<Message>>;
    async fn subscribe_session_events(&self, session_id: &SessionId, level: Level) -> Result<broadcast::Receiver<Event>>;
    async fn get_checkpoints(&self, session_id: &SessionId) -> Result<Vec<Checkpoint>>;
    async fn get_todos(&self, session_id: &SessionId) -> Result<Option<String>>;
    async fn send_ask_user_response(&self, session_id: &SessionId, req_id: &str, response: AskUserResponse) -> Result<()>;
    async fn shutdown_session(&self, session_id: &SessionId) -> Result<()>;
    async fn reload_agent_config(&self) -> Result<()>;
}
```

---

## 7. lib.rs re-export 调整

```rust
// crates/kernel/src/lib.rs
pub mod memory;  // 已存在，保留

// 新增 re-export
pub use types::{Project, ProjectId, CreateSessionInput};
pub use storage::project::{ProjectStore, SqliteProjectStore};

// SessionStore 的 ListArgs 废弃后，从 re-export 中移除或标记 deprecated
// pub use storage::session::{ListArgs, SessionInfo, SessionStore, SqliteSessionStore};
// 改为：
pub use storage::session::{SessionInfo, SessionStore, SqliteSessionStore};
// ListArgs 仅保留类型定义供旧代码过渡，不再用于 list_sessions 接口
```

---

## 8. GUI 层（crates/gui）

### 8.1 新增 commands/project.rs

```rust
#[derive(serde::Serialize)]
pub struct ProjectInfo {
    pub id: String,
    pub name: String,
    pub dir: String,
    pub created_at: String,
    pub updated_at: String,
}

#[tauri::command]
pub async fn list_projects(state: State<'_, AppState>) -> Result<Vec<ProjectInfo>, GuiError>

#[tauri::command]
pub async fn create_project(
    state: State<'_, AppState>, dir: String, name: Option<String>,
) -> Result<ProjectInfo, GuiError>

#[tauri::command]
pub async fn rename_project(
    state: State<'_, AppState>, project_id: String, name: String,
) -> Result<(), GuiError>

#[tauri::command]
pub async fn delete_project(
    state: State<'_, AppState>, project_id: String,
) -> Result<(), GuiError>
```

### 8.2 改造 commands/session.rs

```rust
#[derive(serde::Serialize)]
pub struct SessionInfo {
    pub id: String,
    pub created_at: String,
    pub updated_at: String,
    pub parent_id: Option<String>,
    pub title: Option<String>,
    pub message_count: i64,
    #[serde(rename = "projectId")]
    pub project_id: Option<String>,
    #[serde(rename = "workingDir")]
    pub working_dir: Option<String>,
}

#[derive(serde::Serialize)]
pub struct PaginatedSessions {
    pub sessions: Vec<SessionInfo>,
    pub has_more: bool,
}

#[tauri::command]
pub async fn list_sessions(
    state: State<'_, AppState>,
    project_id: Option<String>,
    before: Option<String>,          // RFC3339
    limit: Option<usize>,            // 默认 20
) -> Result<PaginatedSessions, GuiError>

#[tauri::command]
pub async fn create_session(
    state: State<'_, AppState>,
    project_id: Option<String>,      // 可选
    working_dir: Option<String>,     // 可选，优先级高于 project
    auto_approve_level: String,
) -> Result<String, GuiError>
```

### 8.3 main.rs 注册命令

```rust
.invoke_handler(tauri::generate_handler![
    commands::project::list_projects,
    commands::project::create_project,
    commands::project::rename_project,
    commands::project::delete_project,

    commands::session::list_sessions,
    commands::session::create_session,
    commands::session::restore_session,
    commands::session::fork_session,
    commands::session::delete_session,
    commands::session::shutdown_session,

    commands::chat::send_message,
    commands::chat::subscribe,
    commands::chat::unsubscribe,
    commands::chat::get_messages,
    commands::checkpoint::get_checkpoints,
    commands::checkpoint::rewind,
    commands::skill::list_skills,
    commands::skill::reload_config,
    commands::system::ping,
    commands::system::get_cwd,
    commands::terminal::terminal_spawn,
    commands::terminal::terminal_write,
    commands::terminal::terminal_resize,
    commands::terminal::terminal_kill,
])
```

### 8.4 前端展示结构（树形/分组）

```
┌─────────────────────────────┐
│  Yomi                        │
│                              │
│  Projects                    │
│  ▼ my-app       ~/app        │
│      ├── Session A (2h ago)  │
│      └── Session B (1d ago)  │
│  ▼ yomi-core    ~/yomi       │
│      └── Session C (just now)│
│                              │
│  Independent Sessions        │
│  ├── Session D  ~/tmp        │
│  └── Session E  ~/Downloads  │
│                              │
│  [+ New Project]  [+ New Session]
└─────────────────────────────┘
```

- **Project 区**：`list_projects()` 返回，按 `updated_at DESC`。每个 Project 可展开/收起，内部 Session 走 `list_sessions(project_id=xxx)` 分页加载。
- **独立 Session 区**：`list_sessions(project_id=None)` 返回无 Project 的 Session，同样 cursor 分页。
- 点击 `[+ New Session]`：
  - 若在 Project 下展开 → 默认 `project_id=当前`，`working_dir=None`
  - 若在独立区 → 两者均为 None，或弹窗让用户选路径

---

## 9. 实现顺序

| 步骤 | 内容 | 破坏性 |
|------|------|--------|
| 1 | `ProjectId` / `Project` / `CreateSessionInput` 类型定义 | 否 |
| 2 | `ProjectStore` trait + `SqliteProjectStore` | 否 |
| 3 | Migration v4：加 `projects` 表 + `sessions.project_id` 列 | **是**（协议 v3） |
| 4 | `StorageSet` 集成 `ProjectStore` | 是 |
| 5 | `SessionRow` 新增 `project_id` + `SessionStore` 改造 cursor 分页 | 是 |
| 6 | `SessionConfig` 改为 `Option<Project>` + `Option<working_dir>` | 是 |
| 7 | `AgentSpawnArgs::new()` 默认值改为空 `PathBuf` | 是 |
| 8 | `Session::init` 中按 `resolve_cwd` 条件调用 `with_working_dir` | 是 |
| 9 | `Kernel` 新增 Project API + 重写 Session 创建/恢复/Fork | 是 |
| 10 | `wire.rs` 升到 v3 | 是 |
| 11 | `KernelApi` 扩展 + `RemoteKernel` 实现 | 是 |
| 12 | `KernelServer::dispatch_request` 适配 v3 | 是 |
| 13 | `lib.rs` re-export 调整 | 否 |
| 14 | GUI `commands/project.rs` + 改造 `commands/session.rs` | 是 |
| 15 | GUI `main.rs` 注册命令 | 否 |
| 16 | GUI 前端树形展示 + cursor 分页 | 是 |

---

## 10. 关键决策

| 决策点 | 选择 | 理由 |
|--------|------|------|
| `working_dir` 是否保留列 | 保留，可为 NULL | 支持 session 级覆盖；旧数据零迁移 |
| `project_id` 是否可为 NULL | 是 | 支持独立 Session |
| `project.dir` 是否唯一约束 | 是（`UNIQUE`） | 同一目录 = 同一 Project，避免重复 |
| Session 分页方式 | `updated_at` cursor | 数据变动时 offset 会跳页，cursor 稳定 |
| Project 列表分页 | 暂不分页 | 预期数量少，全量 + 内存搜索足够 |
| Project 删除时 Session 处理 | 禁止删除（有 Session 时报错） | 避免误操作 |
| 旧数据 `working_dir` 是否反向生成 Project | 否 | 最小侵入 |
| `AgentSpawnArgs` 默认 working_dir | 改为空 `PathBuf` | 支持"无工作目录"的 Session |
| `ListArgs` 旧类型 | 废弃，从 `list_sessions` 接口移除 | 改为 cursor 参数 |

---

## 11. 文件改动清单

### kernel（~14 个文件）

1. `src/types.rs` — 新增 `ProjectId`, `Project`
2. `src/storage/migrations.rs` — v4 migration
3. `src/storage/project/mod.rs` — 新增 `ProjectStore`
4. `src/storage/project/sqlite.rs` — `SqliteProjectStore`
5. `src/storage/session/mod.rs` — `SessionInfo` 加 `project_id`；`list` 改 cursor 分页
6. `src/storage/session/sqlite.rs` — `SessionRow` 加 `project_id`；重写 `list` SQL；`create`/`fork` SQL 改造
7. `src/storage/mod.rs` — `StorageSet` 集成 `ProjectStore`
8. `src/agent/types.rs` — `AgentSpawnArgs::new()` 默认 `working_dir` 改为空 `PathBuf`
9. `src/app/session.rs` — `SessionConfig` 改字段；`Session::init` 按 `resolve_cwd` 条件调用 `with_working_dir`
10. `src/app/coordinator.rs` — 新增 `CreateSessionInput`；新增 Project API；重写 Session 入口；`resolve_cwd`
11. `src/app/mod.rs` — 确认 `SessionConfig` / `CreateSessionInput` pub use
12. `src/wire.rs` — v3
13. `src/client/mod.rs` — `KernelApi` 扩展
14. `src/server/mod.rs` — v3 dispatch
15. `src/lib.rs` — re-export `Project`, `ProjectId`, `CreateSessionInput`, `ProjectStore`

### gui（~5 个文件）

16. `src/commands/project.rs` — 新增
17. `src/commands/session.rs` — 改造 `list_sessions`（cursor 分页）+ `create_session`（多参数）
18. `src/main.rs` — 注册新命令
19. 前端页面 — 树形展示 + Project/Session 分组 + 分页加载

---

> 本方案为**结构化扩展**。`working_dir` 保留为最高优先级覆盖，`project_id` 作为可选归属标签。数据库 Migration 最小侵入，旧数据零改动。GUI 保持树形/分组结构，支持独立 Session 与 Project 内 Session 的混排展示。关键修正：`AgentSpawnArgs` 默认 `working_dir` 改为空 `PathBuf`，支持 Session 无任何工作目录。
