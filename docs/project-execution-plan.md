# Project 概念引入执行计划

> 基于 `docs/project-design.md`，按依赖顺序分阶段执行。每阶段结束必须验证编译通过。

---

## 阶段划分

| 阶段 | 范围 | 预估文件数 | 关键验证点 |
|------|------|-----------|-----------|
| 1 | 数据层（类型 + 存储） | ~6 个 | `cargo check` 通过 |
| 2 | 业务层（Session/Agent 改造） | ~4 个 | 单元测试通过 |
| 3 | 传输层（Wire v3 + RPC） | ~4 个 | `cargo check` 通过 |
| 4 | 公开 API（lib.rs） | ~1 个 | 无编译错误 |
| 5 | GUI 层（commands + 前端） | ~4 个 | GUI 编译通过 |

---

## 阶段 1：数据层（类型 + 存储）

**目标**：数据库 Schema + 存储 Trait + 实体类型就绪，`StorageSet` 能构建。

### Step 1.1：类型定义（`kernel/src/types.rs`）

- 新增 `ProjectId`（UUID v7，构造函数 `new()`）
- 新增 `Project` struct

**验证**：
```bash
cd crates/kernel && cargo check --lib
```

### Step 1.2：ProjectStore Trait + SqliteProjectStore（新建）

- 新建 `src/storage/project/mod.rs` — 定义 `ProjectStore` trait
- 新建 `src/storage/project/sqlite.rs` — `SqliteProjectStore` 实现
- 在 `src/storage/mod.rs` 中加入 `pub mod project;`

**验证**：
```bash
cargo check --lib
```

### Step 1.3：Migration v4（`kernel/src/storage/migrations.rs`）

- `CURRENT_SCHEMA_VERSION` 从 `3` 改为 `4`
- 新增 Migration v4：
  ```sql
  CREATE TABLE projects (...)
  ALTER TABLE sessions ADD COLUMN project_id TEXT
  CREATE INDEX idx_sessions_project_id ON sessions(project_id)
  ```

**验证**：
```bash
cargo test -p kernel --test migrations  # 或运行迁移测试
```

### Step 1.4：StorageSet 集成 ProjectStore（`kernel/src/storage/mod.rs`）

- `StorageSet` 新增 `project_store` 字段
- `open_with_config` 中初始化 `SqliteProjectStore`
- 新增 `project_store()` 方法

**验证**：
```bash
cargo check --lib
```

### Step 1.5：SessionStore 改造（`kernel/src/storage/session/`）

- `mod.rs`：
  - `SessionInfo` 新增 `project_id: Option<ProjectId>`
  - `SessionStore::create` 签名改为 `(id, project_id, working_dir)`
  - `SessionStore::list` 签名改为 cursor 分页：`list(project_id, before, limit) -> Result<(Vec<SessionInfo>, bool)>`
  - `ListArgs` 标记 deprecated（不再用于新接口，但保留避免编译失败）
- `sqlite.rs`：
  - `SessionRow` 新增 `project_id: Option<String>`
  - 更新 `create` SQL（INSERT `project_id`）
  - 更新 `fork` SQL（复制 parent 的 `project_id`）
  - 重写 `list` SQL：支持 `project_id` 过滤 + `updated_at < before` + `LIMIT limit+1`
  - 更新 `From<SessionRow>` for `SessionInfo`

**验证**：
```bash
cargo test -p kernel --test sqlite_session  # 或运行现有 session 存储测试
```

> **风险**：`sqlx` 编译时检查（`query!`/`query_as!`）可能因 Schema 未更新而报错。如果用了 `query_as!` 宏，确保 migration 在编译时已运行，或改用 `sqlx::query_as::<_, RowType>(...)` 非宏形式。检查当前代码使用的是 `query_as` 宏还是非宏，如果是宏，需要确认构建时数据库 schema 已更新。

---

## 阶段 2：业务层（Session + Agent 改造）

**目标**：`Coordinator` 能正确创建带/不带 Project 的 Session，`AgentSpawnArgs` 支持无工作目录。

### Step 2.1：`AgentSpawnArgs` 默认值改造（`kernel/src/agent/types.rs`）

- `AgentSpawnArgs::new()` 中 `working_dir` 默认从 `current_dir()` 改为 `PathBuf::new()`

**验证**：
```bash
cargo check --lib
```

### Step 2.2：`SessionConfig` 改造（`kernel/src/app/session.rs`）

- 将 `project_path: PathBuf` 改为：
  ```rust
  pub project: Option<Project>,
  pub working_dir: Option<PathBuf>,
  ```
- 在 `Session::init` / `spawn_main_agent` 中：
  ```rust
  let mut spawn_args = AgentSpawnArgs::new(...);
  if let Some(cwd) = resolve_cwd(config) {
      spawn_args = spawn_args.with_working_dir(cwd);
  }
  ```

**验证**：
```bash
cargo check --lib
```

### Step 2.3：`Coordinator` 重写（`kernel/src/app/coordinator.rs`）

- 新增 `CreateSessionInput` struct
- `Coordinator` struct 新增 `project_store: Arc<dyn ProjectStore>`（从 `StorageSet` 获取）
- `Coordinator::new` 接收 `project_store`
- 新增 `resolve_cwd` helper：
  ```rust
  fn resolve_cwd(config: &SessionConfig) -> Option<PathBuf> {
      config.working_dir.clone().or_else(|| config.project.as_ref().map(|p| p.dir.clone()))
  }
  ```
- 新增 Project API：
  - `create_project(dir, name)`
  - `list_projects()`
  - `get_project(id)`
  - `rename_project(id, name)`
  - `delete_project(id)`（检查是否有关联 Session）
- 重写 `create_session`：接收 `CreateSessionInput`，支持 `project_id` + `working_dir` 组合
- 重写 `restore_session`：从 `SessionInfo` 中解析 `project_id` + `working_dir`
- 重写 `fork_session`：继承 parent 的 `project_id` + `working_dir`
- 重写 `list_sessions`：透传 `project_id` + `before` + `limit` 到 `session_store`

**验证**：
```bash
cargo check --lib
cargo test -p kernel  # 跑全部 kernel 测试，修复编译错误
```

> **风险**：`coordinator.rs` 改动面最大，可能引入编译错误。建议分步改：先加 Project API，再改 `create_session`，再改 `restore_session`/`fork_session`，最后改 `list_sessions`。每改一个函数 `cargo check` 一次。

> **风险**：`create_session` 签名变化后，现有 `server/mod.rs` 和 `client/mod.rs` 的调用点会编译失败。这是预期的，阶段 3 会修复。

---

## 阶段 3：传输层（Wire v3 + RPC）

**目标**：Wire 协议升级到 v3，Client/Server 双端适配。

### Step 3.1：`wire.rs` 升级 v3

- `WIRE_PROTOCOL_VERSION` 从 `2` 改为 `3`
- `RequestMethod` 改造：
  - 新增 `ListProjects`, `CreateProject { dir, name }`, `GetProject { project_id }`, `RenameProject { project_id, name }`, `DeleteProject { project_id }`
  - `CreateSession` 改为：`{ project_id: Option<String>, working_dir: Option<String>, auto_approve_level: Level }`
  - `ListSessions` 改为：`{ project_id: Option<String>, before: Option<DateTime<Utc>>, limit: usize }`
- 其余 `RequestMethod` 不变

**验证**：
```bash
cargo check --lib
```

### Step 3.2：`CoordinatorApi` Trait 扩展（`kernel/src/client/mod.rs`）

- 新增 Project 相关方法
- `create_session` 签名改为接收 `CreateSessionInput`
- `list_sessions` 签名改为 `(project_id, before, limit) -> Result<(Vec<SessionInfo>, bool)>`
- `LocalCoordinator`（即 `Coordinator` 的 `impl CoordinatorApi`）同步扩展
- `RemoteCoordinator` 实现新增 Project 方法的 RPC 转发

**验证**：
```bash
cargo check --lib
```

> **风险**：`RemoteCoordinator` 的 `call` 方法需要为新增的 Project 请求写转发逻辑。如果 `RequestMethod` 有 `Serialize` 问题（如 `DateTime` 在 `ListSessions` 中），确保序列化正确。`DateTime` 已有 `Serialize` 支持，应该没问题。

### Step 3.3：`KernelServer` 适配 v3（`kernel/src/server/mod.rs`）

- `dispatch_request` 中：
  - 新增 Project 分支：`ListProjects`, `CreateProject`, `GetProject`, `RenameProject`, `DeleteProject`
  - 改造 `CreateSession`：从新的 `CreateSession` 结构体构造 `CreateSessionInput`
  - 改造 `ListSessions`：从新的 `ListSessions` 结构体解析参数
  - 其余 Session 分支不变（因为 `session_id` 相关方法签名未变）

**验证**：
```bash
cargo check --lib
```

> **风险**：`CreateSession` 的旧分支从 `project_path` 变为了 `project_id` + `working_dir`，需要确认 `server` 中不再有旧调用点。直接编译报错会指出位置。

---

## 阶段 4：公开 API（`kernel/src/lib.rs`）

**目标**：外部使用者（GUI、CLI）能访问新增类型。

### Step 4.1：`lib.rs` re-export 调整

- 新增：
  ```rust
  pub use types::{Project, ProjectId, CreateSessionInput};
  pub use storage::project::{ProjectStore, SqliteProjectStore};
  ```
- 确认 `SessionInfo` 的 re-export 包含 `project_id` 字段（已自动包含，因为 `SessionInfo` 结构体本身已改）
- `ListArgs` 保留但标注 deprecated，或从 `pub use` 移除（如果 CLI 已不用）

**验证**：
```bash
cargo check --lib
cargo check --all-targets  # 确保 cli/tui 也不会因移除 re-export 编译失败
```

> **风险**：如果 CLI 中用了 `ListArgs`，移除 re-export 会导致编译失败。先 `cargo check` 看 CLI 是否受影响，再决定是保留还是移除。如果 CLI 已不用 `list_sessions`，可以安全移除；否则保留。

---

## 阶段 5：GUI 层（crates/gui）

**目标**：GUI 暴露 Project 命令，Session 列表支持分页。

### Step 5.1：新建 `commands/project.rs`（`gui/src/commands/`）

- 定义 `ProjectInfo` DTO
- 实现 `list_projects`, `create_project`, `rename_project`, `delete_project` 命令

**验证**：
```bash
cargo check -p yomi-gui
```

### Step 5.2：改造 `commands/session.rs`（`gui/src/commands/session.rs`）

- `SessionInfo` 新增 `project_id` + `working_dir` 字段
- `list_sessions`：
  - 参数改为 `(project_id, before, limit)`
  - 返回 `PaginatedSessions { sessions, has_more }`
  - 调用 `coord.list_sessions` 新签名
- `create_session`：参数改为 `(project_id, working_dir, auto_approve_level)`

**验证**：
```bash
cargo check -p yomi-gui
```

### Step 5.3：`main.rs` 注册命令（`gui/src/main.rs`）

- 在 `invoke_handler` 中加入 `commands::project::*`
- 更新 `commands::session::list_sessions` 和 `commands::session::create_session`（注册名不变，但参数结构变了）

**验证**：
```bash
cargo check -p yomi-gui
```

> **风险**：Tauri 的 `generate_handler` 宏要求所有函数签名在编译时可用。如果参数结构体不匹配（如 `Option<String>` vs `Option<&str>`），编译会报错。确保 `list_sessions` 和 `create_session` 的签名与 `generate_handler` 中的声明完全一致。

### Step 5.4：前端适配（TS/Vue/React）

- 新增 `project` 相关 API 调用
- `list_sessions` 改为 cursor 分页：首次不传 `before`，后续传上一页最后一条 `updated_at`
- 首页改为树形/分组展示（Projects + Independent Sessions）

**验证**：
- 前端编译通过（TypeScript 类型检查）
- 运行时测试：创建 Project → 在 Project 下创建 Session → 列表分页加载

> **风险**：前端改动最大，但风险最低（因为前端不阻塞后端编译）。建议在前端完成前，先用 `tauri` 的 `invoke` 在 console 里手动测试后端命令是否正常工作。

---

## 执行顺序速查表

```
阶段 1: 数据层
  1.1 types.rs (ProjectId, Project)
  1.2 storage/project/mod.rs + sqlite.rs
  1.3 migrations.rs (v4)
  1.4 storage/mod.rs (StorageSet)
  1.5 storage/session/mod.rs + sqlite.rs (SessionInfo, SessionStore)
  
  验证: cargo check --lib && cargo test

阶段 2: 业务层
  2.1 agent/types.rs (AgentSpawnArgs 默认 working_dir)
  2.2 app/session.rs (SessionConfig, Session::init)
  2.3 app/coordinator.rs (CreateSessionInput, Project API, 重写 Session 入口)
  
  验证: cargo check --lib && cargo test

阶段 3: 传输层
  3.1 wire.rs (v3)
  3.2 client/mod.rs (CoordinatorApi 扩展 + RemoteCoordinator)
  3.3 server/mod.rs (dispatch_request)
  
  验证: cargo check --lib

阶段 4: 公开 API
  4.1 lib.rs (re-export)
  
  验证: cargo check --all-targets

阶段 5: GUI
  5.1 gui/commands/project.rs (新建)
  5.2 gui/commands/session.rs (改造)
  5.3 gui/main.rs (注册命令)
  5.4 前端 (TS + UI)
  
  验证: cargo check -p yomi-gui
```

---

## 每阶段 Rollback 策略

| 阶段 | Rollback 方式 | 条件 |
|------|--------------|------|
| 1-4 | `git stash` / `git checkout` 回到 HEAD | 编译失败无法在规定时间内修复 |
| 5（GUI） | 单独分支开发，不合并到主分支 | 前端改动未完成 |

**建议**：阶段 1-4 在同一个分支做，每步 commit 一次。GUI 可以等 kernel 稳定后另开分支。

---

## 关键检查清单（每步结束必查）

- [ ] `cargo check` / `cargo test` 通过
- [ ] `cargo clippy --all-targets` 无新 warning（理想情况，但非阻塞）
- [ ] 改动的函数有对应编译调用点（如改了 `create_session` 签名，确认 `server`/`client` 中已改）
- [ ] 旧数据兼容性：Migration 只加列，不删列，不删表
- [ ] Wire Protocol 版本号已 bump（v3）

---

> **开始时机**：建议在当前工作分支上直接执行阶段 1-4。GUI 阶段可等 kernel 侧通过 `cargo check --all-targets` 后并行开发。
