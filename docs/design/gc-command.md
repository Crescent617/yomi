# 设计文档：`yomi gc` —— session 资源统一垃圾回收

## 背景

一个 session 的资源散落在多处。当前 `yomi session cleanup` 只覆盖其中一部分（sessions 表 + messages/todos/file_states 三类文件），存在明显遗漏：

### Session 资源全景（盘点）

| # | 资源 | 位置 | 现有 cleanup 是否覆盖 |
|---|---|---|---|
| 1 | session 元数据 | sqlite `sessions` 表 | ✅（含 `sub_*` 子行级联） |
| 2 | 消息历史 | `sessions/{id}.jsonl` | ✅ |
| 3 | todo | `sessions/todos/{id}.json` | ✅ |
| 4 | 文件状态 | `sessions/file_states/{id}.jsonl` | ✅ |
| 5 | goal 状态 | `sessions/goals/{id}.json` | ❌ |
| 6 | checkpoint | `checkpoints/{id}/`（目录，含 manifest 与快照） | ❌ |
| 7 | subagent 的文件资源 | `sessions/sub_*.jsonl` 及其 todos/goals/file_states | ⚠️ 仅当子行 id 出现在 cleanup 返回值中才删，且 checkpoint/goal 同样遗漏 |
| 8 | token 用量 | sqlite `token_usage` 表 | **明确不动**（历史用量统计，见非目标） |
| 9 | channel 映射 | sqlite `channel_session_mappings`（无外键） | ❌ |
| 10 | pinned 标记 | sqlite `pinned_sessions`（`ON DELETE CASCADE`，自动） | ✅（隐式） |
| 11 | 写入残留 | `sessions/*.tmp`（原子写中断残留，实际环境已观察到） | ❌ |
| 12 | 孤儿文件 | 上述各目录中 db 已无对应行的文件（历史 bug / 手动删库产生） | ❌ |
| 13 | CLI app_data | `app_data/projects/{hash}.json` 指向已删 session | ❌（可自愈：加载失败时走新建 session，明确不管） |
| 14 | 日志 | `logs/`，kernel 已有 7 天自动清理 | 不归 gc 管 |

## 目标

- 新增顶层命令 `yomi gc`，按时间阈值一次性回收 session 的**全部**关联资源。
- 回收逻辑下沉到 kernel（`storage::gc`），CLI 只是壳；GUI / daemon 定时任务未来可复用同一实现。
- 默认 dry-run，输出将删除的明细与预计回收空间；`--yes` 才真删。
- **直接删除 `yomi session cleanup` 子命令**（含 `commands/session/cleanup.rs`），由 `yomi gc` 取代，不留别名。

## 非目标

- **绝不触碰 `token_usage` 表**。用量数据是跨 session 的历史统计资产（`yomi usage` 按天/模型聚合），与 session 生命周期解耦；session 删除后保留其用量行不产生悬空引用（该表无外键）。
- 不做按 project / working_dir 维度的选择性回收（后续可加 `--dir` 过滤）。
- 不清理 `app_data`（自愈）、`logs`（已有机制）、`workspace/`（用户数据）。
- 不在 daemon 内自动定时 gc（本期只做手动命令，留 hook）。

## 方案

### 1. kernel 新增 `storage/gc/mod.rs`

```rust
pub struct GcOptions {
    /// updated_at 早于 now - days 的 session 视为过期
    pub days: i64,
    /// 是否跳过 pinned session（默认 true）
    pub keep_pinned: bool,
    /// 是否顺带清扫孤儿文件（默认 true）
    pub sweep_orphans: bool,
    /// 删除后是否 VACUUM（默认 false，大库耗时）
    pub vacuum: bool,
    /// dry-run：只统计不删除
    pub dry_run: bool,
}

#[derive(Default, serde::Serialize)]
pub struct GcReport {
    pub sessions: Vec<SessionId>,      // 含 sub_* 子 session
    pub files_deleted: u64,            // jsonl/json/tmp 文件数
    pub checkpoint_dirs_deleted: u64,
    pub channel_mappings_deleted: u64,
    pub orphan_files_deleted: u64,
    pub bytes_reclaimed: u64,
    pub errors: Vec<String>,           // 单项失败不中断，收集后汇报
}

pub struct GarbageCollector { storage: StorageSet }

impl GarbageCollector {
    pub async fn run(&self, opts: &GcOptions) -> Result<GcReport>;
}
```

放在 kernel 的理由：`StorageSet` 拥有全部 store 与 `data_dir`，路径规则（`sessions/`、`todos/`、`goals/`、`file_states/`、`checkpoints/`）都是 kernel 内部约定，不应让 CLI 二次硬编码（现有 cleanup.rs 就是反例——kernel 加一种资源，CLI 必然遗漏）。

### 2. 执行流程（五阶段）

**Phase 1 — 圈定victims**（一条 SQL，只读）：

```sql
SELECT id FROM sessions
WHERE updated_at < :cutoff
  AND (:keep_pinned = 0 OR id NOT IN (SELECT session_id FROM pinned_sessions))
```

再补两类：
- 过期 session 的 `sub_*` 子 session（无论子行自身 updated_at）；
- **孤儿 subagent**：`parent_id IS NULL AND id LIKE 'sub_%' AND updated_at < :cutoff`（父行早年被 `ON DELETE SET NULL` 置空的历史遗留）。

dry-run 在此返回：对每个 victim 统计其文件尺寸（`fs::metadata`）累加为 `bytes_reclaimed` 预估。

**活跃保护**：CLI 直连 sqlite 时无法知道 daemon 内存中哪些 session 正在运行，依赖两点兜底：
1. `updated_at < cutoff` 本身排除活跃 session（任何消息写入都会 touch）；
2. `days` 参数强制下限 `>= 1`，拒绝 `--days 0` 之类的危险输入。

**Phase 2 — 删 sqlite 行**（单事务，victims 按 100 分块 `IN (...)`）：

```sql
DELETE FROM sessions WHERE id IN (...);                        -- pinned_sessions 级联
DELETE FROM channel_session_mappings WHERE session_id IN (...);
-- token_usage 一律不动
```

先删 DB 后删文件：崩溃时留下的是可被 Phase 4 兜底的孤儿文件，而非"有行无文件"的脏状态。

**Phase 3 — 删文件**（逐 victim，失败记入 `errors` 继续）：

- `sessions/{id}.jsonl`
- `sessions/todos/{id}.json`
- `sessions/goals/{id}.json`
- `sessions/file_states/{id}.jsonl`
- `checkpoints/{id}/` → 直接 `remove_dir_all`（checkpoint 目录设计即"self-contained for atomic cleanup"）

删除前 `metadata` 累加真实 `bytes_reclaimed`。

**Phase 4 — 孤儿清扫**（`sweep_orphans`）：

1. `SELECT id FROM sessions` 得到存活集合（HashSet）；
2. 遍历上述五个目录，文件名解析出 session id，不在存活集合 → 删除并计数；
3. `sessions/*.tmp`：mtime 早于 1 小时即删（原子写残留，与 cutoff 无关）。

**Phase 5 — 收尾**：

- `vacuum` 为 true 且非 dry-run：`VACUUM` + `PRAGMA wal_checkpoint(TRUNCATE)`，回收 db 文件与 WAL 空间；
- 汇总 `GcReport` 返回。

### 3. CLI（`crates/cli/src/commands/gc.rs`）

```
yomi gc [OPTIONS]

OPTIONS:
    --days <N>          回收 N 天前的 session [default: 90] [min: 1]
    -y, --yes           真正执行（默认 dry-run）
    --include-pinned    连 pinned session 一起回收
    --no-orphans        跳过孤儿文件清扫
    --vacuum            回收后压缩 sqlite（VACUUM + WAL truncate）
    --json              以 JSON 输出 GcReport（供脚本消费）
```

参数说明与典型用法：

| 参数 | 默认 | 说明 |
|---|---|---|
| `--days <N>` | `90` | 阈值：`updated_at` 早于 `now - N 天` 的 session 视为过期。强制 `N >= 1`，拒绝 `--days 0` |
| `-y, --yes` | dry-run | 不带 `--yes` 只打印报告，磁盘与 db 零改动 |
| `--include-pinned` | 跳过 pinned | pinned session 默认视为用户显式保留 |
| `--no-orphans` | 清扫 | 跳过 Phase 4（孤儿文件 / 过期 `.tmp` 清扫）。孤儿清扫只依赖"db 中不存在"这一事实，与 `--days` 无关 |
| `--vacuum` | 不压缩 | 大量删除后回收 sqlite 文件空间；大库耗时，故 opt-in |
| `--json` | 表格输出 | 输出 `GcReport` JSON，便于 cron 脚本判断 |

```bash
yomi gc                        # 看看 90 天前有什么可回收（安全，只读）
yomi gc --yes                  # 实际回收
yomi gc --days 30 --yes        # 更激进：30 天
yomi gc --days 365 --yes --vacuum   # 年度大扫除并压缩 db
yomi gc --json | jq .sessions  # 脚本消费
```

全局参数沿用 `GlobalArgs`（`--config <PATH>` / `--dir <DIR>`），用于定位数据目录。

输出示例（dry-run）：

```
yomi gc (dry-run) — sessions older than 90 days

  sessions        42  (including 7 subagents)
  checkpoints     15 dirs
  orphan files    12  (3 stale .tmp)
  est. reclaim    186.4 MB

Run again with --yes to delete.
```

`main.rs` 变更：
- `Commands` 增加 `Gc(GcArgs)`；
- 删除 `SessionsCommands::Cleanup` 变体及 `commands/session/cleanup.rs`。

### 4. 涉及的 kernel 接口变更

- `SessionStore` 增加 `list_expired(&self, cutoff, keep_pinned) -> Result<Vec<SessionId>>` 与 `delete_batch(&self, ids) -> Result<u64>`；现有 `cleanup(days)` 方法直接删除（唯一调用方 cleanup.rs 已随命令移除）。
- `ChannelStore` 增加 `delete_by_sessions(&self, ids) -> Result<u64>`。
- `CheckpointStore::delete_session_checkpoints` 已存在，直接复用。
- `StorageSet` 暴露 `pub fn gc(&self) -> GarbageCollector`。

### 5. 风险与决策点

| 决策 | 选择 | 理由 |
|---|---|---|
| token_usage | **完全不动** | 用量是跨 session 的历史统计资产，与 session 生命周期解耦；该表无外键，保留不产生悬空引用 |
| session cleanup 命令 | 直接删除，不留 alias | 功能被 gc 完全覆盖且实现更完整，留 alias 徒增维护面 |
| pinned 默认处理 | 默认跳过 | pin 是用户显式意图，静默级联删除违反最小惊讶 |
| 先删 DB 还是先删文件 | 先 DB | 崩溃残留可被 orphan sweep 兜底，反向则产生悬空行 |
| daemon 运行时执行 gc | 允许 | WAL 模式支持并发写；活跃 session 被 updated_at 天然保护 |
| goals 目录命名 | `sessions/goals/` | 与 `JsonGoalStore::path` 一致，gc 内用常量与 store 共享，避免路径漂移 |

## 测试

kernel `storage/gc/gc_test.rs`（独立测试文件，遵循项目惯例）：

1. 过期 session 全链路删除：造 session + 五类文件 + channel 行 + checkpoint 目录 + token_usage 行，gc 后前者全部消失且 **token_usage 行原样保留**，`GcReport` 计数正确；
2. 未过期 / pinned session 不受影响；`--include-pinned` 时 pinned 被删；
3. subagent 级联：父过期 → 子行与子文件同删；孤儿 `sub_*` 被回收；
4. 孤儿清扫：手工放置无主 jsonl 与过期 `.tmp`，被清；新鲜 `.tmp` 保留；
5. dry-run：报告非空但磁盘与 db 均无变化；
6. 单文件删除失败（权限）不中断整体，`errors` 有记录。

CLI 层：`gc` 参数解析冒烟测试（`--days` 下限校验、dry-run 默认）。

## 工作量

kernel：新模块 gc（~300 行）+ `SessionStore`/`ChannelStore` 各加批删方法；CLI：新 `commands/gc.rs` + main.rs 接线，删除 `commands/session/cleanup.rs` 及对应子命令。
