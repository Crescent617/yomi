# 设计文档：Agent Harness 演进 —— 模板 / 工单 / Janitor

源自 2026-08 的 harness engineering 调研（Ralph Loop、Gas Town/Beads、Anthropic long-running harness、Claude Code 源码泄露、微软 SRE Agent、Compound Engineering 等），结合 yomi 现状得出的一组相互配套的演进方案。

## 设计原则（贯穿所有模块）

1. **内核薄，资产文件化**：能做成文件 + skill 的，不做成内核机制。文件可 diff、可 git 管理、可删改。
2. **harness 假设会过期**：每个组件都是"模型暂时做不到"的补丁，必须易拆除（Anthropic）。
3. **约束优先编码进 tool schema，其次才是 prompt 文本**（schema-filtered subagents）；自然语言约束在委派中会衰减（constraint drift），关键边界要在 spawn 时显式重申。
4. **一切皆文件 + bash 优于专用工具/MCP**：微软 SRE Agent 从 100+ 专用工具转向文件暴露后 Intent Met 45%→75%。
5. **权限只能收窄不能扩大**：任何模板/委派机制的工具集只能是父 agent 工具集的子集。
6. **创建自由、生效分层、晋升有闸**：agent 自主创建的资产先在 workspace 沙盒生效，全局生效需要证据或用户确认。

## 总览

| # | 模块 | 形态 | 内核改动 | 状态 |
|---|---|---|---|---|
| 0 | cron job name 唯一 + create ensure 语义 | 内核 | 已完成（schema v19） | ✅ 已落地 |
| 1 | Agent Templates（subagent SP 模板） | 文件资产 + 少量内核（desc 自足，无 skill） | `agent` 工具加 `template` 参数 + conductor resolve | ✅ 已落地（schema v20；内置 planner/verifier/explorer/reviewer；发现靠 desc + 目录直读） |
| 1.5 | 记忆层 `.agents/memory/`（agent 自写笔记，**仅项目层**） | 文件资产 + SP 门控指针（已落地） | 二期：索引装配（loader+截断+git-root）；全局层暂缓 | 一期已落地 |
| 2 | 工单 tickets `.yomi/tickets/` | 文件约定 + task-tickets skill + `ticket.sh` 建单脚本 | **零** | ✅ 文件+skill+脚本已落地；GUI 投影曾实现并 e2e 验证，后按决策撤下（先验证好用再上展示，wire 保持 23 未升） |
| 3 | Janitor 后台策展循环 | janitor skill + daemon ensure cron | 小：daemon 按项目 ensure + config 开关 | **暂缓**（2026-08，见 P3 暂缓说明） |
| 4 | tickets 内核化 / GUI 看板 / agent discovery | — | 条件触发，见"观望清单" | 观望 |

依赖关系：P1 让"派活"变便宜（角色标准化）→ P2 让"跟踪活"变便宜（状态外化）→ P3 把两者串成复利系统（资产策展）。P0 的 ensure 语义是 P3 调度的前置（已就绪）。

---

## P0：cron job name 唯一 + ensure 语义（已完成）

**动机**：session 无状态，agent 跨 session 会重复创建同名 cron job；此前 create 是裸 INSERT，无任何防重。

**语义**：`name` 是 job 的身份。create 撞名时不报错、不新建、不改写，返回已存在 job 的 id（`created=false`）；要调整走 update。

- `cron_jobs.name` 唯一索引（migration v19）；存量重名保留 `updated_at` 最新一条，其余改名 `name#dup-<id前缀>`，不删行。
- `create_cron_job`（tool 与 RPC 共用入口）先按 name 查重短路；并发撞名被唯一索引拒绝时回滚专用 session 后返回竞态胜者。
- cron tool 输出 `{job_id, created, ...}`，desc 已注明唯一性——模型 create 前无需先 list。
- 改动文件：`cron/{types,store,mod}.rs`、`kernel/mod.rs`、`tools/cron.rs`、`storage/migrations.rs` + 测试。

---

## P1：Agent Templates（subagent SP 模板）

**动机**：当前每次 spawn 子 agent 都在 prompt 里重复造角色；真正的价值是 **per-role 工具收敛**（schema 级约束）+ 一致性 + token 效率 + 用户可策展团队。

### 资产格式

全局 `~/.yomi/agents/<name>/ROLE.md` + workspace `<cwd>/.yomi/agents/<name>/ROLE.md`，workspace 覆盖 global（glob `*/ROLE.md`，按目录名得名）。**纯 markdown，无 frontmatter**（2026-08 收敛：模型反正读全文，元数据只剩一行的价值不抵格式开销）——全文即角色 SP。文件名 `ROLE.md`（避免与项目级 `AGENTS.md` 混淆），目录层为将来附属文件预留。~~INDEX.md 索引约定~~已废弃（2026-08）：手工索引对小规模角色是负资产（忘更新=静默失联），选择靠目录名 + 首行角色陈述 + read 全文。**位置定案（2026-08）：放 `.yomi/agents/` 而非 `.agents/agents/`**——模板正文是 yomi 的提示词约定（工具名、内置角色语义都是 yomi 的），与别家 subagent 格式不保证兼容；共享目录的前提是共享格式。`.agents/` 只留给真正的跨厂商标准（skills）。

```markdown
你是独立验收者。你没有参与实现，只对照验收标准逐条判定……
```

要点：
- 全文即角色 SP——开头一句点明角色定位（模型按 name + 首行陈述速览，read 全文确认）。
- body 保持短：角色定位 + 输出契约 + 关键边界重申。知识一律走 skill preload，模板不膨胀成第二个 skill 体系。
- 格式纯 md、无 frontmatter（模型反正读全文）；无 yomi 私有魔法（NLAH 式可移植，未来可交换/共享）。

### 发现机制：`agent` 工具 desc 自足（无系统提示段、无单独 skill）

- **不加 `# Agent Templates` 系统提示段，也不要单独 skill**（2026-08 收敛）：发现完全由 `agent` 工具 desc 自足承载——内置角色一句话用途（planner/verifier/explorer/reviewer）+ 目录约定（模型 glob 目录、read 全文选择）。大部分 case 按 name 即可判断使用场景。
- 传不存在的模板名时报错并附当前可用列表（名称+来源）作自纠正兜底。
- 无快照过期问题：resolve 实时读盘，session 中段新建的模板立即可用。
- **子 agent 不注入模板清单**（子不能再生子，给了也是浪费 token）。

### 内核改动点

1. `tools/subagent.rs`：`agent` 工具 schema 加可选 `template` 参数；exec 时按名实时 resolve（global → workspace 合并后查找）。
2. `kernel/conductor.rs` spawn 处：模板命中时 base_prompt 换为模板 body（替代父 base_prompt），空 body 回落默认 + warn。
3. ~~tools_block / model_key / skills 字段~~：**全部不实现**（2026-08 收敛）——纯 prompt 角色，model/skills/工具集全继承。tools_block 曾实现（含跨层并集、sessions.tools_block 快照列 v21）后经评审与简化决策移除；**v21 列在 DB 中休眠**，重新启用时 git 历史可溯。正则匹配语义脆弱（评审 Major 2）是另一教训：若重做应考虑精确名匹配。
4. 观测：**sessions 表加 `template TEXT` nullable 列（migration v20，对齐 v13 `model_key` 单列先例）**，subagent spawn 时写入模板名，NULL = 自由文本派活。用途：janitor 的 usage 统计（`GROUP BY template` 可 SQL 聚合，比事后捞 metadata 事件可靠）、GUI 展示"这个 sub 是 verification"、质量回退归因（Anthropic postmortem 教训）。

### 自治分级（模板治理）

| 层级 | 自治程度 |
|---|---|
| 一次性角色写进 `prompt` 文本，不落盘 | 完全自由（**默认**，绝大多数角色止步于此） |
| workspace `.yomi/agents/` | agent 自主创建，项目级爆炸半径，git 可 review |
| 全局 `~/.yomi/agents/` | **半自主**：用户明说，或 janitor 带证据晋升（跨 session 出现 N 次、workspace 验证过）；晋升发事件、留 diff、可回滚 |

防污染配套：usage 记录（模板需要死亡路径）、janitor 合并近重复/重写 description、创建/修改发事件（自主可以，隐身不行）。

### 内置首批模板（预置进内核，2026-08 修正）

`planner` / `verifier` / `explorer` 三个，与 Claude Code 内置对齐（Plan / verification / Explore）。**2026-08 对齐决策：不设 builder**——CC 的证据是执行不需要专门角色（通用 agent + 好的任务简报即可），yomi 不指定 template 的自由派活即等价物；builder 的输出契约价值不足以抵消一个冗余内置。`verifier` 最值钱：固化"独立 evaluator 对照验收清单逐条判 PASS/FAIL + `VERDICT:` 锚点行"，对治自我评估偏差——这也是契约式 QA 循环的载体。**内置一律不设 `tools_block`**（同为 2026-08 决策）：约束写进正文即可，硬收窄机制保留给真正需要的自定义模板；planner 的计划可作为交付物落文件（如 `plan.md`/`docs/design/`），只读仅是默认而非硬约束。

**官方模板预置在代码里，不走 yomi-extensions**（修正理由）：①模板无安装器生态（skills 有 `npx skills add`，templates 只能手动 symlink——预置消灭唯一分发摩擦，P1 开箱即用）；②模板与 kernel 同演进（resolve 语义、内置角色集），随二进制发版天然版本对齐；③先例：Claude Code 内置 agents（Explore/Plan）即产品内嵌。

- **形态**：`crates/kernel/src/agent_tmpl/<name>/ROLE.md`（loader 模块与资产同目录），`include_str!` 编入二进制——纯文件可 review、dogfood 同一格式，不散在 .rs 字符串里。一模板一目录（对齐 skill 的 `<name>/SKILL.md` 先例，目录层为附属文件预留），文件名 `ROLE.md`（模板 = "这个 agent 是谁"的角色定义；与 `SKILL.md`/`AGENTS.md`/`MEMORY.md` 同属单词大写约定）——三层同构：
  - `crates/kernel/src/agent_tmpl/<name>/ROLE.md`（内置）
  - `~/.yomi/agents/<name>/ROLE.md`（全局，跟 `data_dir` 走）
  - `<repo>/.yomi/agents/<name>/ROLE.md`（workspace，同名覆盖）
- **三层合并**（预置当地板不当天花板）：embedded（官方）→ `~/.yomi/agents/` → `.yomi/agents/`（workspace 最高），同名覆盖——用户改官方模板只需放个同名文件；错误路径列表标注 `(builtin)` 来源。**信任边界**：workspace 层随仓库分发（git clone 即带入），信任等级 = 该仓库代码本身；body 可被整体覆盖，review 仓库自带模板如同 review 依赖。

---

## P1.5：记忆层 `.agents/memory/`（agent 自写的学习笔记）

**动机**：现有 `memory/project.rs` 只加载 AGENTS.md/CLAUDE.md——只有"人写的规则"层，缺"agent 自己记的笔记"层（Claude Code 2026.3 才补上的"最大短板"）。定位区分：AGENTS.md = 人规定它怎么做；memory = 它记录自己学到了什么。

**各家参照**：Claude Code（`~/.claude/projects/<project>/memory/`，三层：MEMORY.md 索引截断 200 行/25KB → 主题文件按需 → transcript 可搜索不回读；同一 git repo 共享不碎片）；Cline（仓库内 `memory-bank/` 六文件）；skills 已有 `.agents/skills/` 跨厂商标准，memory 尚无标准。

### 结构

```
<repo>/.agents/memory/     # 项目：本仓库学到的经验
    MEMORY.md              #   索引，截断加载（≤200 行 / 25KB，取小者）
    topics/*.md            #   主题文件，按需 read
# ~/.agents/memory/（全局层）暂缓实施——SP 门控保留检测，创建文件即自愈激活
```

资产布局按"可移植性"分家：跨厂商标准住 `.agents/`（`skills/`），yomi 私有住 `.yomi/`（`agents/` 角色、`memory/` 事实、`tickets/` 意图）。
**分工口诀：规则→AGENTS.md，做法→skill，角色→template，事实→memory，意图→tickets。**

### 决策点

1. **选仓库内**（Cline 流派），而非 Claude Code 的仓库外 keyed：与 skills 的 workspace 约定一致、可 git 可 review；不想进 git 则 `.gitignore` `.agents/memory/`。
2. **git-root resolve 属二期**：一期 SP 门控按 `working_dir`（cwd）检测（与 tickets 一致），子目录启动的 session 看不到仓库根的 memory；二期索引装配时一并改 git-root（`git rev-parse --git-common-dir` 推主仓库根，worktree 共享），失败回退 cwd。
3. **装配分期**：**一期不装索引**——目录约定 + SP 存在性门控指针（prompt builder 检测 `.agents/memory/MEMORY.md` 存在才注入一行指针 + 使用约定；实现 ~20 行代码，SP 侧固定 header + ≤2 行指针；convention in SP、content in files，与 `SKILL_SECTION_HEADER` 同先例），yomi 仓库已 dogfood。**二期再上索引装配**（loader + 截断 + git-root resolve，见下节"装配"），触发条件：观察到空转信号（memory 有事实但 agent 仍重复犯错），或需要"无指针也开箱即用"的体验。**观测说明（评审 Major 3）**：触发信号目前靠人观察（janitor 暂缓期无自动巡检）；后续可在 read/edit 事件流上对 `.agents/memory/` 路径计数，或二期加 CLI/GUI memory 面板时一并给人检入口。
4. **写入纪律**：项目级 agent 自写自由（事实笔记非指令，风险低于模板）；**全局层暂缓**（一期无全局记忆；SP 门控保留全局检测，未来创建 `~/.agents/memory/MEMORY.md` 即自愈激活，届时写入走 janitor/用户确认）。
5. **明确不做**：向量库/embedding 检索（mem0、Letta archival 路线）——文件 + grep + 索引在当前规模足够；事实量大到 grep 吃力再议。**也不需要专门的 memory 搜索工具**：MEMORY.md 的一行一事实设计本身就是检索界面（索引即目录）；参照 CC——连 transcript 层都只配文件 + grep。搜索工具的第一个真实需求场景预计是 session transcripts（"上次那个 bug 怎么修的"），届时为 sessions 做（CLI/GUI 会话搜索），memory 作为文件搭便车。升级信号：topics 多到 grep 信噪比崩、或 agent 反复找不到确实存在的事实。
   - **为什么保留项目层而不是全局-only**：事实是项目级分布的（构建坑/根因/repo 约定），全局层只收跨项目事实；全局-only 会污染所有 session 的注意力并失去 locality 过滤，且丢 git 共享。若只能留一层，留项目层。
6. **防膨胀防线**：SP 段结构免疫（固定 header + ≤2 行指针；二期索引装配另有 200 行/25KB 硬截断）。风险在 MEMORY.md 文件本身——量膨胀（条目淤积）与质膨胀（流水账）。防线按强度：分工口诀（每类信息各有去处，memory 只收 durable 事实）→ 格式纪律（一行一事实）→ janitor 策展（合并/剪枝/清过期；**索引 >150 行则本轮优先策展**，行数即健康指标）。janitor 暂缓期间靠量小 + 人直接改文件撑着，不依赖"agent 自己难受会整理"（它更可能不读 → 空转）。熔断成本为零：删掉/归档 MEMORY.md 即复位，SP 段自动消失。二期可加 CLI/GUI memory 面板（对齐 CC `/memory`），人直接改是最便宜的策展。

### 装配（SystemPromptBuilder 管线，二期）

插入位置：base → AGENTS.md → **# Memory 段** → # Skills → # Environment（先规则、后事实、再技能）。

```
# Memory
You have persistent memory across sessions. Below are indexes of facts learned
from past work — read the referenced topic files for details. Record new durable
facts (user preferences, project gotchas, root causes) by editing MEMORY.md:
one fact per line, keep the index lean.

## Global (~/.agents/memory/MEMORY.md)
<截断后的全局索引>

## Project (<git-root>/.agents/memory/MEMORY.md)
<截断后的项目索引>
```

1. **只装索引**：两个 MEMORY.md 各自硬性截断（≤200 行且 ≤25KB，取小者），行边界截断 + 尾部 `… [truncated — see <path>]`，超限不报错——截断即倒逼索引精炼的压力机制（Claude Code 同款）。主题文件不进 prompt，索引行即指针，按需 `read`。
   - **为什么不是"只告诉目录、agent 自己 grep"**：记忆的价值是被动浮现——agent 不知道事实存在就不会去查（unknown-unknowns），grep-only 会让记忆层变成只写不读的墓地。各家顶层全部装入上下文（CC 装索引、Cline 全量、Letta core 常驻）；grep 适用于"知道自己在找什么"的参考资料（主题文件/归档/transcript），不适用于行为塑形的事实。划界：**行为塑形→装，参考资料→grep**。索引方案的存亡条件是双上限截断 + janitor 策展，防止索引膨胀成语料。
2. **段首写入契约**：短指令说明"是什么、怎么读、往哪写"（项目事实写项目、偏好写全局、一行一事实）——装配不仅是读的注入，也是写的授权，缺了这句 agent 不会知道自己有权写记忆。
3. **空目录零成本**：目录不存在或索引为空 → 整段不注入。
4. **git-root resolve 的 worktree 坑**：`--show-toplevel` 在 linked worktree 返回 worktree 根；共享记忆要用 `git rev-parse --git-common-dir` 推主仓库根，失败回退 `<cwd>/.agents/memory/`。spawn 时执行一次，~2s 超时兜底。
5. **subagent 同样装配**（conductor 对 sub 走同一 builder；模板化 SP 时 memory 段追加在模板 body 之后）——事实类知识正是子 agent 最易踩的坑。
6. **快照一致性**：spawn 时装配，session 中段写入下次 spawn 生效；并发写由 edit 工具的 FileStateStore（改前必读）缓解，索引冲突留给 janitor reconcile。

实现触碰点：`memory/` 新 loader（resolve→读→截断→空则 None）、`utils/path.rs` git-root helper、`prompt/mod.rs` builder 插段；conductor 零改动；可选 `[features]` 开关（对齐 CC `--bare` 语义）。

---

## P2：工单 `.yomi/tickets/`（task-tickets skill，agent 面零内核）

> **价值定位修正（2026-08）**：工单的真实定位是**项目的跨 session 待办板**（意图持久层），不是多 agent 协作基础设施。yomi 的主协调循环已闭合（spawn 当面派单 + input_bus 自动回报 + todo 自跟踪 + session 列表的结构性 fleet 视图），多 agent 场景下 tickets 近似仪式化冗余。真实价值在：①工作到达时间 ≠ 处理时间（飞书留言、cron 产物——先记工单后认领）；②session 易失而意图持久（"上次干到哪"不靠翻旧会话）。**memory 装事实，tickets 装意图。** 据此冻结追加投资（结构化 task 工具、dispatch 外循环、租约回收），等真实场景驱动；单写者为主的现状下文件形态完全适配，claim 竞态基本不会出现。

**动机**：父 agent 并发派 N 个异步子 agent 后，跟踪进度只有"自己记"（占上下文）或"逐个 post_message 问"（打扰）两条路。缺的是共享任务状态。

**为什么不做 DB + task 工具**（推翻过初版方案）：微软 SRE Agent 的证据（专用工具→文件，45%→75%）、"bash 优于 MCP"的社区共识、Gas Town Beads 本质是 git-backed 文件账本。且 subagent 继承父的 working_dir——**工作区目录天然是父+整棵子树的共享空间，连 scope 解析都不用做**。

### 文件约定（定稿；操作规范以 task-tickets skill 为单一事实源）

```
.yomi/tickets/<id>-<slug>.md     # 一个任务一个文件
```

- id：7 位随机字母数字串（如 `t3m9q2x`），`ticket.sh` 铸造——kernel 投影取文件名第一个 `-` 前为 id、其余为 title 兜底（规则刻意无脑，可预期优先）；
- frontmatter（snake_case）：`title`（可省，缺省从 slug 推导）、`status: pending|claimed|done|blocked`、`owner_session_id`、`created_at`；**不写 `updated_at`**——由文件 mtime 派生；
- body：顶部规则块（建单脚本注入的精简状态机/编辑规则，没装 skill 的执行者也能照章编辑——工单自解释）+ 任务描述与验收标准；完成后追加标题恰为 `## Result` 的结果段；
- 完结归档进 `.yomi/tickets/archive/`（子目录不计入活跃板）。
- **已知限制**：tickets 按 `working_dir`（cwd）定位，从子目录启动的 session 会看不到仓库根的 tickets——git-root resolve 推广见开放问题。

### GUI 展示（**已撤下，2026-08**：先验证工单本身好用再上展示；以下为已验证过的实现方案，恢复时可参照）

**原则：agent 写路径永远走文件；GUI 只是读者**——kernel 加只读投影层（解析 + wire 输出），不给 agent 加任何新工具。

- **数据通道**：wire 方法 `list_tickets { session_id }` → kernel 从 session 解析 working_dir → 扫 `.yomi/tickets/*.md` → 容错解析 frontmatter（缺字段给默认、解析失败跳过并以文件名兜底 title）→ 返回 `TicketItem { id, title, status, owner_session_id, created_at, updated_at }`（serde snake_case，前端 TS 直接对齐）。解析逻辑独立小模块 `kernel/src/ticket/`，frontmatter 解析复用 skill loader 的同款做法。
- **侧栏区块**：主侧栏加 Tickets section，跟随当前选中 session 的 working_dir。内容克制：状态计数（pending/claimed/done/blocked，用 `subtle`/`info`/`success`/`warning` 语义色）+ 最近更新的 3–5 条（状态点 + title 单行截断 + owner 短 id）。**空态不显示整个区块**（tickets 目录不存在即不出现——与 SP 门控同款"出现即激活"哲学）。
- **样式（对齐 `crates/gui/DESIGN.md`）**：不用卡片/嵌套容器，层级靠排版与分隔线（workspace over dashboard）；区块标题用 `micro-label` 签名工具类；状态色仅 6–8px 圆点且只表达状态（color communicates meaning，其余全中性）；title 用 UI 字体、id/时间等 metadata 用 IBM Plex Mono + `subtle` 保持安静；行高 28px 级；轮询更新不闪不重排，仅状态变更给短 fade，尊重 `prefers-reduced-motion`；图标只 Lucide 不混 emoji。
- **轮询**：区块可见期间每 ~10s 调一次 `list_tickets`，不可见立即停止；不做文件监听（避免 notify 生命周期管理）。
- **只读**：v1 无任何写操作；想改去改文件或让 agent 改。

### 协议（task-tickets skill + `scripts/ticket.sh` 承载，2026-08 改为派单模型）

- **建单走脚本，状态流转直接改文件**（2026-08-14 精简，原 `set` 子命令移除）：`ticket.sh new` 强制 id/slug/时间戳/frontmatter 合规——格式失败模式集中在建单（id 不合规、日期格式错、YAML 未闭合）；状态流转只是改 `status:` 行 ± `owner_session_id` 行 + 追加 Result 段/备注行，模型直接编辑文件即可，脚本包装徒增"先定位 skill 路径才能调用"的摩擦。状态机规则（`pending→claimed→done|blocked`、`blocked→claimed` 复工、`claimed→pending` 重置、done 终态）由 skill 文档承载。聚合用 grep、归档用 `mv`，同样不值得包脚本；
- **派活为主**：协调者建派工单（一个任务一个文件），spawn 子 agent 时在 prompt 里指明任务文件路径；"自主认领"降级为边缘场景（跨 session 捡活）；
- 执行者先签收再动手（`claimed` + 自己的 session id）；完成置 `done` 写 Result，卡壳置 `blocked` 写明原因；
- 父 agent 聚合进度用 glob + grep，不逐个问；验收可派 `verifier` 模板；
- Ralph 式外循环可直接组合：cron 定时 steer 父 session"看 `.yomi/tickets/`，把 pending 派出去" ≈ 极简版 Gas Town Mayor。

**诚实的代价**：原子 claim 弱（竞态窗口）、结构化查询弱。派单模型下竞态基本不出现（任务是指派的不是抢的），可接受。

### 升级路径（条件触发，不预判）

出现以下痛点再内核化：claim 竞态实际发生 / 父 agent 聚合吃力 / GUI 需要活看板。届时新增 `task` 工具 + `tasks` 表为 source of truth，文件降为人的视图。

---

## P3：Janitor 后台策展循环

> **暂缓（2026-08）**：本期不做。未想透的问题：
> 1. **蒸馏质量的置信度**——janitor 写错一条事实比没有事实更糟（信任崩塌），蒸馏正确性如何保障/验证？
> 2. **证据来源的读取路径**——janitor 读哪些 session、增量如何界定（state.md 追踪到哪儿）、读 transcript 的成本；
> 3. **成本与节奏**——固定 cron vs 空闲检测未决；每轮 token 预算多少算合理，价值未证实前不好定；
> 4. **价值前置条件未满足**——memory 层刚建（P1.5 一期），还没积累出需要策展的规模；先观察资产的自然生长形态再设计策展，比空想策展规则更靠谱。
>
> **复活触发信号**：memory 索引 >150 行 / 观察到空转（有事实但 agent 重复犯错）/ 索引出现错误事实 / 工单归档堆积。暂缓期间记忆层靠"agent 实时自写 + 人直接改文件"维护——纯文件资产的人工策展成本本就最低。

**动机**：Compound Engineering（review 阶段沉淀，让每个工作单元使下一个更容易）+ AutoDream/KAIROS（空闲时后台整理记忆资产）+ Fowler 的 entropy management——同一模式。yomi 已有 daemon + cron，缺的不是机制是一个 skill。

### 调度：随项目走的 janitor 实例，模型无入口

- janitor 不是全局单例，而是**每项目一个实例**：cron job 命名 `janitor:<project_name>`（P0 的 name 唯一 + ensure 语义使重复 ensure 无害），`send_message` 绑专用 session 且 working_dir = 项目目录——每个 janitor 只装一个项目的上下文，避免跨项目串味（把 A 的根因写进 B 的记忆）；
- **激活范围**：daemon 启动时遍历 `projects` 表，只给最近活跃（如 7 天内有 session）的项目 ensure janitor job；开关与频率走 config（`[features]`），模型无创建入口；
- 内核改动点：daemon ensure 循环 + ensure 路径支持按 project 设置专用 session 的 working_dir（当前 `follow` 只从调用方 session 继承，daemon 传 None 会落回默认目录）；
- **工作台与产物分离**：state/journal 放 `~/.yomi/janitor/<project>/`（工作痕迹不是项目资产），策展产物（memory/tickets/模板）留在仓库内。

### 职责（janitor skill 定义）

1. 板上 done 的任务归档，蒸馏"什么做得好/踩了什么坑"；
2. 模板策展：workspace→global 晋升（带证据）、合并近重复、重写 description 保可区分性、按 usage 记录剪枝；
3. **memory 策展（项目级）**：两条写入路径——agent 干活时实时自写（快而脏）+ janitor 离线批理（慢而净）；蒸馏来源 = 该项目近期 sessions + tickets done 项；落地点是项目 `.agents/memory/`（**不是 AGENTS.md**——人写的地图不被 agent 笔记污染）；动作：合并、剪枝、再验证、遗忘归档、索引限长。全局 memory 暂缓后，原"全局策展"职责（双向晋升/层间去重/更严收录）一并推迟，重启时再议；
4. 修复文档/模板漂移（entropy management）；
5. 输出策展报告（事件/digest 可见）。

### 遗忘机制

核心原则：**遗忘的判据不是"旧"，是"不再为真或不再有用"**。纯按时间删除会丢掉最值钱的永恒事实、留下新近琐碎条目——年龄只是触发再验证的信号，不是判决。

- **再验证优先**：超龄条目（last_verified 超过 N 天）做廉价核实——引用的文件还在吗、CI 配置还是这么写的吗、与新条目矛盾吗。验证通过即续命（不论多老），失败才进入遗忘流程。
- **降级阶梯不硬删**：活跃索引 → archive（带时间戳 + 原因）→ 永久留档可 grep。agent/janitor 重新发现归档事实时可**复活**它；复活事件记入 journal——它是"遗忘判据有误"的学习信号。
- **验证账本放 janitor 的 state.md**（事实 → last_verified 映射），不在 MEMORY.md 行里塞日期戳，索引格式保持干净。
- **保守不对称**：错删一条关键事实的代价 ≫ 多留十条无害的——拿不准就留；全局 memory 遗忘门槛高于项目级（影响面）。遗忘的意义是维持索引信噪比与可信赖度：agent 发现索引里有错事实就会不信任整个记忆库，那是比膨胀更快的死法。
- **适用边界**：memory 条目与 topics（再验证+归档）、工单归档（超期摘要后丢弃）、janitor 自己的 journal（按月蒸馏进 state.md 后截断——追加式不管它比 memory 膨胀更快）。session transcripts 归 `storage/gc`，janitor 不越界。

**复用专用 session** 让 janitor 对资产库有连续记忆，策展质量随运行次数提升；上下文累积由 compactor 兜底。

### 记忆模型：私有工作记忆 vs 物化资产

janitor 的记忆分两层，**其他 agent 只消费第二层，不直接访问 janitor**：

**第一层：janitor 私有工作记忆**
- 复用的专用 session transcript = 连续工作记忆（每次 cron 触发落在同一会话）；
- 结构化状态放 `~/.yomi/janitor/<project>/`（随项目实例分目录）：`state.md`（资产库快照、usage 摘要、待办）+ `journal.md`（追加式策展日志）。每次运行开头读 state、结尾更新；
- 为什么有了 session 还要 state.md：compaction 有损，关键状态（"上次策展到哪""哪些模板连续零 usage 待剪枝"）必须落在不受 compaction 影响的文件里——上下文是工作记忆，文件是持久记忆。

**第二层：物化的公共资产（其他 agent 的消费接口，全部是既有通道）**

| janitor 的产出 | 位置 | 消费通道 |
|---|---|---|
| 策展后的模板 | `~/.yomi/agents/` | `agent` 工具 desc + 目录直读（P1 发现机制） |
| 新增/修订的 skill | 全局/项目 skill 目录 | skills 清单 |
| 蒸馏出的事实笔记 | `.agents/memory/`（P1.5） | MEMORY.md 索引加载 + 主题文件按需读 |
| 工单归档 | `.yomi/tickets/archive/` | glob/grep 按需查 |

其他 agent **不需要知道 janitor 存在**——模板变干净、skill 变准、文档漂移被修，都通过现有加载通道自然生效。

**明确不做**：不祝福其他 agent 直接 post_message 查询 janitor——ad-hoc 查询会污染它的工作上下文，且它是批处理角色而非 always-on 服务。需要"未蒸馏的历史"时走 session 搜索，不借道 janitor。通道分工：协作走 tickets（状态）与 post_message（通信），资产演进走 janitor（离线策展），互不交叉。

---

## 质量缺口与对策（2026-08 评审后补）

两个已认领的缺口及最小对策：

1. **harness 回归测量** → `evals/harness-e2e.sh`（主仓）。11 项确定性断言覆盖全部 harness 表面：cron ensure、模板 spawn 落库 + VERDICT 锚点、explorer 只读、memory 门控正反例、ticket 状态机。改 harness 后手动跑（~2 分钟，含 2 次模型调用），无 LLM judge。后续可进 CI。
2. **约定漂移无看护** → 烟雾报警器方案（**不等完整 janitor**）：一个只读周检 cron，让漂移从"被动发现"变"主动可见"：
   ```bash
   yomi cron create --name system:drift-check --schedule "0 9 * * 1" \
     --message "漂移检查（只读，异常才报告）：1) ~/.yomi/agents/ 角色目录内容审查；2) 本仓库 .agents/memory/MEMORY.md 行数 >150；3) .yomi/tickets/ 里 claimed 且 mtime 超 3 天的僵尸工单。"
   ```
   只读、固定节奏、零内核改动、P0 的 ensure 语义保证重复安装无害。完整 janitor（策展/遗忘/蒸馏）仍按 P3 暂缓。

## 观望清单（明确不做 / 条件触发）

| 事项 | 态度 | 触发条件 |
|---|---|---|
| tickets 内核化（task 工具 + tasks 表） | 条件触发 | claim 竞态发生 / 聚合吃力 / GUI 看板需求落地 |
| agent discovery（列举活跃 agent） | 观望 | 跨 session 协作痛点出现 |
| DAG / workflow 引擎、convoy 式重型编排 | **不做** | 反 Ralph 哲学：确定性外循环 + fresh context，不设计流程图 |
| 模板 marketplace / 社区资产包 | 观望 | 格式已保持可移植，生态等自然出现 |
| blackboard（多 agent 共享知识空间） | 观望，纯约定可启动 | 触发：平行 agent 需要看到彼此的**中途发现**（多角度调研 / swarm 调试 / 异步多方共推同一目标）。与现有资产的分工：tickets = 意图（要做什么），blackboard = 进行中发现（战役级、随战役过期），memory = 固化事实。形态草稿：`.yomi/blackboard/` 追加式一条目一文件、可 `superseded_by`、耐久发现晋升 memory。注意：tickets 已不是 blackboard——名字腾出就是为这个概念预留的 |
| skill 市场 / 自有 installer | **不做** | 通用 `npx skills add`（vercel-labs/skills）已是生态事实标准：支持 72+ agent、默认 symlink 进技能目录、私有仓库走本机 git 凭证；yomi 读 `~/.agents/skills` 通用位置天然兼容 |

## 资产分发：yomi-extensions 独立仓库

内核不捆绑任何 skill/template；产品级资产放独立仓库 [`yomi-extensions`](https://github.com/Crescent617/yomi-extensions)（伞概念：skills、agent templates、未来的更多扩展形态——不局限于 skills），按耦合度切分归属：

| 资产 | 耦合度 | 归属 |
|---|---|---|
| `task-tickets` / `janitor` skill | 零~弱耦合（纯约定 + playbook） | yomi-extensions |
| ~~`agent-templates` skill~~ | — | **已删除**（2026-08 收敛）：发现由 `agent` 工具 desc 自足承载（内置一句话用途 + 目录约定），不再需要单独 skill |
| 内置模板 planner/verifier/explorer/reviewer | 强耦合（schema 随 kernel 版本） | **预置进内核**（`crates/kernel/src/agent_tmpl/` + `include_str!`，三层合并的地板层） |
| `feishu-e2e` / `release-it` / `yomi-self` | 维护者工作流 | 留在 yomi 主仓 `.agents/skills/` |

理由：skill 迭代节奏远快于内核发版；`.agents/skills` 是跨厂商标准，约定型 skill 对机器上所有 agent 工具（Claude Code/Cursor/Codex）都有效；独立仓库是社区贡献的最小单位；主仓不背内容 churn。

**安装零自有代码**，用生态通用 CLI（vercel-labs/skills，默认 symlink 进 `~/.agents/skills/`，yomi 原生可读）：

```bash
npx skills add Crescent617/yomi-extensions --list     # 先看有哪些
npx skills add Crescent617/yomi-extensions -g         # 全局安装全部
npx skills add Crescent617/yomi-extensions -g --skill tickets   # 只装单个
```

templates 不属于 skills 生态：官方模板内核预置、无需安装；yomi-extensions 里的实验性/社区模板手动 symlink 到 `~/.yomi/agents/`。

**发现机制终版（2026-08 收敛）**：不需要 skill、不需要系统提示段、不需要索引文件——`agent` 工具 desc 自足：内置角色一句话用途（planner/verifier/explorer/reviewer）+ 目录约定（glob + read 选择，首行即角色陈述）。传不存在的模板名时报错并附当前可用列表（名称+来源）作自纠正兜底。

版本 skew：描述 kernel 行为的 skill 在 frontmatter/README 标注最低 yomi 版本。

## 落地顺序

1. **P1 模板机制**：✅ 已落地——`NewSession` 参数结构体重构 + sessions.template 列（v20；v21 tools_block 列休眠）+ `agent_tmpl` 三层加载（内置 `include_str!` / 全局 / workspace 覆盖）+ `agent` 工具 `template` 参数（实时 resolve、错误列可用表；desc 自足含内置一句话用途与目录约定）+ conductor spawn 应用（body 换 base prompt；model/skills/工具全继承）+ 内置 planner/verifier/explorer/reviewer（对齐 CC）——纯 markdown 无 frontmatter，无单独 skill；
2. **P1.5 记忆层**：一期目录约定 + SP 门控指针（✅ 已落地，仅项目层）；二期索引装配（小内核）等空转信号再动；
3. **P2 tickets**：✅ 文件约定 + task-tickets skill + `ticket.sh` 脚本落地（yomi-extensions，本机 symlink 生效）；GUI 投影曾实现并经 tauri-pilot e2e 验证，后按决策整体撤下（kernel ticket 模块/wire/GUI 组件均移除，git 历史可溯），等工单流程验证好用后再恢复展示层；
4. **P3 janitor**：**暂缓**（见 P3 暂缓说明；复活看触发信号）；
5. P4 按触发条件再议。

## 开放问题

- 模板版本记录粒度（只在 metadata 记名字，还是连内容 hash 一起记，供质量回退归因）；
- `.yomi/tickets/` 的安放位置：workspace 根 vs `.agents/` 下（前者更显眼、易 git 化，后者与 skills/agents 约定统一）；
- **git-root resolve 的推广**：一期 memory/tickets 均按 `working_dir`（cwd）定位；二期统一改 git root（子目录启动的碎片化问题）；
- 全局 memory（暂缓）重启时的晋升路径：与模板同构（janitor 带证据 or 用户确认）？
- janitor 的空闲判定与默认频率（固定 cron vs 检测活跃度）；
