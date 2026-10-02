# memory_recall 内置工具 + 记忆规则进系统提示词

状态：待 hrli 评审
范围（hrli 2026-10-01 拍板）：**不做 hook、不做 NOW.md 注入**。只做两件事：
1. 内置 `memory_recall` 工具（kernel 侧，替代 bash 脚本 `memory/recall`）；
2. 记忆规则进系统提示词（existence-gated，与现有 MEMORY.md 指针同一模式）。

## 背景

当前记忆系统完全跑在 agent 侧：

- 纪律文字在 workspace `AGENTS.md`（经 `memory::load` 拼进系统提示词）；
- 检索靠 bash 脚本 `memory/recall`（tier + 行数上限的 rg 封装）；
- janitor、cron、写入全靠 agent 自觉。

kernel 里已有一块 `crates/kernel/src/memory/`，但只做 project memory（AGENTS.md/CLAUDE.md → 系统提示词），与这套 agent 记忆是两套东西。另有 `prompt/mod.rs` 的 MEMORY.md 指针（`.agents/memory/MEMORY.md`，一行一事实的轻量约定），与本设计共存、不冲突。

动机（实锤先例）：部署坑「瘦 PATH 缺 rg → kernel grep 工具残废」——bash 脚本依赖运行环境，内建工具不依赖；工具化后权限分级（Safe）、超时、输出截断都有统一收口。

## 非目标

- 不做 embedding / 向量检索（tiered rg 的精度够用，文件即真相源）；
- 不做 kernel 自动写记忆（写是模型判断）；
- 不做 hook、不做 NOW.md 自动注入（本次明确排除）。

## 一、memory_recall 工具

### 名称与权限

- 名称：`memory_recall`（常量 `MEMORY_RECALL_TOOL_NAME`）；
- 权限：`Safe`（只读），加入 `permission/resolver.rs` 的 Safe 名表。

### 参数

| 参数 | 类型 | 必填 | 说明 |
|---|---|---|---|
| `keyword` | string | 是 | 正则模式（与 grep 工具一致），大小写不敏感固定开 |
| `root` | string | 否 | 显式指定 memory 根目录（绝对路径）。缺省走自动解析 |

不加 limit/offset：行数上限是这套检索的纪律本体，放开就等于退回普通 grep。

### memory 根目录解析

1. 给了 `root` 用之；
2. 否则从 `ctx.working_dir` 向上逐级找第一个含 `memory/` 的目录（与 bash 脚本行为一致）；
3. 都找不到 → 返回错误文本 `no memory/ found from <cwd> upward`，不报错成工具失败（ToolOutput::text，让模型自己决定）。

不引入 env 覆盖（脚本里的 `RECALL_ROOT` 放弃）：cron 等场景用 `root` 参数显式表达，行为可审计。

### 分层与上限（自 `memory/recall` 移植）

| tier | 内容 | 上限 |
|---|---|---|
| 0 hot | `NOW.md`（在办工作） | 10 行 |
| 1 evergreen | 顶层其余 `*.md`（除 `NOW.md`/`archive.md`）+ 非日期型子目录（friend/group/parallel 及未来新增） | 30 行 |
| 2 dated | 日期型子目录（内含 `YYYY-MM-DD.md`）内的匹配，按日期新→旧排 | 50 行 |
| 3 cold | `archive.md`（已归档） | 30 行 |

通用化决策：tier 1/2 的分类**按文件名模式判定，不硬编码目录名**——子目录里若文件全是 `YYYY-MM-DD.md` 则为 dated，否则为 evergreen。新增日期目录（如未来加 `expenses/`）自动归入 dated，不用改 kernel。若 hrli 要求与脚本严格一致，回退为硬编码白名单（friend/group/parallel、diary/worklog/dream/janitor），二选一在评审时定。

### 匹配与截断

- 每个文件最多 20 条匹配；
- 匹配行长于 200 字符截断（截断处标 `…`）；
- 整体搜索超时 30s（与 grep 工具一致）；
- 实现：候选文件集合是显式路径清单，直接 `regex::RegexBuilder`（已有依赖）逐文件逐行扫，**不走 grep_engine**（引擎面向目录 walk + gitignore，而 memory 目录要确定性全扫，且可能落在 .gitignore 里）。文件均为小文本（经验最大几百 KB），逐行扫足够快。

### 输出格式

```
## NOW (in-flight)
NOW.md:3:- yomi 开发主线：…

## evergreen
contacts.md:12:…
friend/白水.md:34:…

## dated (newest first)
worklog/2026-10-01.md:40:…

## archive (superseded)
archive.md:120:…
```

tier 无匹配则该段不输出；某段命中达上限附一行 `(capped, N more — refine your keyword)`。路径一律相对 memory 根目录。

### 注册

`ToolRegistry::with_standard_tools` 注册，**existence-gated**：以 session working_dir 向上解析到 memory 根则注册，否则跳过（与 MEMORY.md 指针「目录不存在零成本」同一哲学）。注册发生在 session spawn 时、working_dir 已知，判断一次即可。

## 二、记忆规则进系统提示词

位置：`prompt/mod.rs` `SystemPromptBuilder`，existence-gated——解析到 `memory/` 根才注入（与 `MEMORY_SECTION_HEADER` 同一模式）。

内容（从 workspace AGENTS.md 的 Memory 节提炼为通用规则，去掉 hrli 个人专属细节）：

```markdown
# Memory
You have a persistent memory at <relative path>/memory/ (plain markdown files, git-committed).

- **Search before asking**: `memory_recall <keyword>` before asking a human about prior decisions, people, or project history.
- Layout: `NOW.md` = in-flight register (one line per task, closed out to worklog);
  top-level `*.md` + `friend/`/`group/` = evergreen facts; `worklog/` = append-only daily log;
  `archive.md` = superseded (grep, never read whole).
- Writes are your judgment: record durable facts in place; one fact per line;
  people with ≥2 distinct interactions earn a file under `friend/` or `group/`;
  volatile facts cite their authoritative source instead of copying.
```

注入后 workspace `AGENTS.md` 里对应的 Memory 大段**删除**（否则 `memory::load` 与 kernel 注入双份，白烧 prompt）。

### 与 MEMORY.md 指针的共存

两个块都命中时会出两个 `# Memory` 标题。处理：合并为一段——`memory/` 存在则发上面的富段；`.agents/memory/MEMORY.md` 存在则在其下补一行索引指针（不另起标题）。

## 三、迁移与验证

1. 工具落地后 workspace `memory/recall` 脚本保留不动（逃生舱 + 对照），提示词规则改为指向工具；观察一至两周无差再删脚本。
2. 测试：`crates/kernel/src/tools/memory_recall_test.rs`（AGENTS.md 规则：ut 独立 `_test.rs`）——根解析（显式/向上/未找到）、tier 分类（含新增日期目录自动归类）、各 tier 上限、200 字符截断、日期排序、权限 Safe。
3. 改完跑 `evals/harness-e2e.sh`（动了工具 desc/注册），`just ci`。

## 待拍板

1. tier 分类：按 `YYYY-MM-DD.md` 模式通用化 vs 硬编码目录名白名单（建议通用化）；
2. 注册 gating：existence-gated vs 恒注册（建议 existence-gated）；
3. env 覆盖：放弃 `RECALL_ROOT`，只留 `root` 参数（建议放弃）；
4. AGENTS.md 同步删 Memory 大段的时机：随本改动一起删 vs 观察期后删（建议一起删，双份注入是纯成本）。
