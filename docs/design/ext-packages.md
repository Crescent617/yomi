# 设计文档：扩展包（extension packages）—— 安装与生命周期层

取代：无（新增层；不动 `ext.md` 外挂体系的任何运行时语义）。
动机：pi 的扩展"丢进目录即生效、宿主接管生命周期"，yomi 的五套扩展
机制（skill / 外挂 tool / hook / 卡片触发器 / snippet 预留）各自自洽但
**没有打包、安装、回滚的单位**——memory-system 这类"装一次"的扩展目前
靠 agent 读 README 手工执行 bootstrap，不可重放、不可审计、不可卸载。

## 定位：不是第六个端口，也不是伞概念

`ext.md` 拍板过"目录名即语义，不造伞概念"，本设计不推翻它：

- **运行时不变**：hook 扫描、tool 合并、skill 索引、事件表，一行不动。
  注册表的真相仍然是目录，扩展包不在运行链路里出现。
- **扩展包只活在安装侧**：它是"怎么把东西放进注册表 + 怎么干净地拿出
  来"的自动化，是外挂体系之上的**生命周期层**，不是新的能力端口
  （四端口无第五不变）。
- 一个包可以携带多种资源（cron + hook + tool + skill + snippet），
  但每种资源落地后仍受各自既有契约管辖（执行位开关、超时、退出码、
  ensure 语义……）。包不改变资源的运行时行为，只改变资源的到达方式。

## 决策记录

1. **命名：命令 `yomi extension`，目录 `extensions/`，内部模块
   `extension`**（曾用 `pkg`，0.10.55 后改——pkg 太通用）。
   `ext` 已被外挂体系占用（`tools/ext.rs`、`[ext:<名>]` 错误前缀、卡片
   `ext_` 命名空间），再派给包管理会一詞两义。CLI 侧与现有全词命令
   （session/cron/channel/skill）风格一致。
2. **所有权 = symlink 指向，不由数据库记录判定**。记录（source/version/
   资源清单）是审计与展示数据，不参与正确性。任何中断的安装可重跑
   收敛；记录丢失 remove 仍可回滚（state is cache）。
3. **install 对 cron 纯 additive**：ensure 缺才建，已存在不动（不更新
   不删除）——防覆盖用户用 `yomi cron` 手动改过的 job。清扫只在
   remove 发生。manifest 删条目产生的孤儿 job 由 list 的漂移报告暴露，
   不做隐式清理。
4. **挂载冲突即拒绝**（除非槽位已指向本包）：绝不覆盖用户文件或其他
   扩展的挂载。
5. **snippet 是 manifest 外的约定式资源**：`snippets/*.md` 按文件名序
   拼进 system prompt，不挂载、不复制。和 skill（知识进 prompt）同一
   哲学：约定在文件里，kernel 只负责拼。
6. **snippet 对所有会话生效**（含 sub-agent），与 skills 同口径；后续
   若观测到 prompt 成本问题再按会话类型收敛。
7. **install 脚本钩子（init）按需声明，不默认执行**：`ext.init = "scripts/init.sh"`
   声明了才在装完/刷新后从已装目录执行（每次 install/refresh 都跑，
   ensure 哲学，幂等是作者约定）；未声明的扩展零脚本。执行环境 =
   普通 yomi 子进程（注入 `YOMI_DATA_DIR`，PATH 含 `<data_dir>/bin`），
   统一 shell 探测 + 进程树 + 120s 超时。失败即 install 报错。权限不
   设新门槛：bins/hooks 本就执行作者代码，init 只是装时自动跑一次。
   初始化类脚本约定放 `scripts/`，不上 PATH；bin 只收日常命令。
8. **bin 走 `<data_dir>/bin` + PATH prepend，不告诉模型路径**：命令名
   即接口，snippet 只提名字。kernel 改动收在 `inject_child_env` 一处，
   所有子进程（shell/cron/hook/tool）同时获得解析能力。
9. **v1 资源范围 = cron + hooks + snippets + bin**（2026-10-01 hrli
   拍板）；tools/skill 挂载二期。包格式和所有权规则为此留好了位置。
10. **统一 copy，安装源默认 GitHub URL**（2026-10-01 hrli 拍板，
    nvim 插件 / npx skills 玩法）：`owner/repo[/子目录][@ref]` 或
    `https://github.com/...`，本地目录仅作开发形态。install 一律
    `git clone --depth 1`（sha 全量 + checkout）到临时目录后实体
    复制进 `extensions/<名>`；重装 = 重新取货 + 原位刷新（更新语义）。
    不再有 symlink/copy 双模式——没有 broken source、模式切换、记录
    与磁盘分叉这些状态面；代价是源仓后续 commit 不会自动生效，要
    更新就重装。
11. **注册表 = 文件系统，不是 sqlite**：install 复制完在包目录里
    盖单文件注册表 `extensions/ext.lock`（Cargo.lock 式
    `[[extensions]]`：来源/版本/hash/资源清单/安装时间），
    `extensions/`
    目录本身就是索引。`ext.toml` 是作者的 manifest，**原封不动**——
    等价 Cargo.toml 对 Cargo.lock 的关系。
    这是对 AGENTS.md「extension state lives in sqlite/config — never
    private」的**有意偏离**：该条的精神是"状态必须可被外部工具读写
    审计"，而 TOML 段比 sqlite 表更外部（cat 即读），且消除了两份
    真相的漂移面（hooks/tools/skills 的注册表真相都是目录）。不设
    extensions/ 级 lock/index 文件：包间无跨包事务，那会再造第二
    真相源。正确性仍不依赖元数据（决策 2 不变：前缀清扫 + 指向
    判定兜底）。
12. **provenance 与内容 hash 落盘**：单文件 `extensions/ext.lock` 记录用户给的原始
    来源字符串与 git resolved commit sha（rev），以及安装时刻的包
    内容 hash（blake3，按相对路径排序喂 路径+内容，单文件 1MB 上限，
    超出按恶意/损坏拒绝 install；symlink 跟随到文件按其内容算）。
    lock 是包目录外的单文件、天然不入各包 hash（lock 是我们的、每次
    重装重写），装完
    即比对必须一致（health=ok 的不变量）；list/doctor 用同算法重算
    比对 → `modified` 健康态暴露本地手改。作者 manifest 里的任何
    内容（哪怕自己写了 `[install]` 表）原样进 hash——不归我们管，
    也伪造不了归属（归属只看包外注册表——作者随包自带 ext.lock
    只是普通包文件，照 hash 照复制、不解析）。

## 包格式

```
<源目录>/                      # git 仓库里的一个子目录
├── ext.toml                   # 唯一 manifest
├── snippets/*.md              # 可选：SP 片段（约定式，文件名排序）
├── prompts/dream.txt          # 可选：cron message_file 引用的素材文件
├── bin/<可执行文件>            # 可选：进 PATH 的命令（见「bin 与 PATH 层」）
└── hooks/<point>/<entry>      # 可选：hook 条目（裸文件或 <名>/run 目录）
```

`tools/`、`SKILL.md`（skill 资源）、`agents/`（模板）二期再加（2026-10-01
hrli 拍板：v1 只支持 cron + hooks + snippets + bin）。

### ext.toml

```toml
[ext]
name = "memory-system"        # 必填。字母开头 [a-z0-9-]，≤32
version = "0.1.0"             # 必填。展示/审计用
description = "..."           # 必填。ext list 展示

[[cron]]
name = "dream"                # 条目名 ≤48；全名 = ext:<扩展名>:<条目名>
schedule = "0 4 * * *"        # 5/6 段，本地时区（cron 子系统既有语义）
message_file = "prompts/dream.txt"   # 与 message 二选一（文件相对包根）
# message = "内联文本"        # 二选一
# work_dir = "..."            # 可选：per-run 会话工作目录
# precheck = "..."            # 可选：传感器闸门命令
```

校验在 install 时硬拒绝：name 非法、cron 条目名非法、message 与
message_file 同时给或都不给、schedule 无未来触发点（复用
`next_run_from_schedule`）、message_file 读不到。

## 命名与命名空间

| 资源 | 全名规则 | 例 |
|---|---|---|
| 扩展 | `<name>`，全局唯一（extensions/ 槽位）；字母开头 `[a-z0-9-]` ≤32，install/remove 同规则校验 | `memory-system` |
| cron job | `ext:<扩展名>:<条目名>`（全局唯一，cron 表 name 唯一约束兜底） | `ext:memory-system:dream` |
| hook 挂载 | 保留包内条目名，与用户 hook 按字典序混排 | `hooks/pre_tool_use/50-guard` |
| bin 挂载 | 保留包内文件名，进 `<data_dir>/bin/`（PATH 解析，无命名空间） | `bin/recall` |
| snippet | 不命名空间化，按文件名排序拼接 | `snippets/memory.md` |

`ext:` cron 前缀是所有权标记：remove 按前缀清扫；用户用普通
`yomi cron` 能看到、能改这些 job（透明，不藏）。

hook 执行序注意：扩展条目与用户条目混排按字典序，包作者用名字自定位
（用户钩子常占 `10-`/`20-`，扩展约定 `50-` 起），写进扩展编写约定。

## install 语义（幂等，可重放）

执行者：daemon（wire 方法），CLI 不直连 store——与 cron 命令同轨道。

顺序与每步规则：

1. **取货**：GitHub 源 `git clone --depth 1`（`@ref` 为 40 位 sha 时
   全量 clone + checkout）到临时目录，`owner/repo[/子目录][@ref]`；
   本地目录直接用。clone 超时 3 分钟；网络失败不脏任何槽位。取货后
   记录 `git rev-parse HEAD` 为 rev（本地源为 None）。
2. **收编** `extensions/<name>`：一律先拷到隐藏临时目录 `.<name>.tmp`
   再 swap 就位（fresh 与刷新同路径——kill -9 落在复制中途只留
   `.tmp` 孤儿，下次 install 起点清掉，不留半个无记录的槽位，否则
   重跑撞 occupied、remove 按 foreign 拒绝）。可否刷新不由调用方
   预读决定：install 持注册表全局锁后从 `ext.lock` 取本扩展条目
   （锁外预读是陈旧快照），有条目 = 原位刷新，无条目 = fresh（槽位
   必须为空，被占即拒）。其他（用户目录、无归属证明的残留）→ 拒绝
   （提示先 remove）。调用方只负责提供同一个预解析 manifest（防
   两次 parse 之间文件被换名的 TOCTOU）。
3. **清扫旧挂载**（挂载前）：扫 `hooks/`、`bin/` 挂载树，凡 symlink
   精确指向本包、而新包没有声明的挂载一律摘除。**扫树本身，不信
   旧记录名单**——上一次刷新中断留下的挂载可能既不在旧条目
   （provisional 资源沿用旧记录）也不在新包里，按名单清扫会漏成
   永久悬空 symlink（phantom-block 同名槽位 + health 看不见）。
   指向不符（用户动过）留下 warn，由 remove 处置。
4. **挂载** hooks/bin：逐槽位三种情况——空则建 symlink（创建撞
   AlreadyExists = 并发竞争，退到重判定）；已指向本包则跳过；其他
   （用户文件/他扩展/破损 link）→ 整体拒绝并报占用清单（先建后查，
   撞了不自动回滚已建部分，重跑 install 即收敛——所有权规则保证
   重跑安全）。
5. **收养 cron**：逐条 `create_cron_job`（ensure：同名即返回不动，
   缺才建）。输出区分 `created` / `exists (untouched)`。
6. **init 钩子**（manifest `ext.init` 声明才跑）：从已装目录执行，
   环境 = 标准 yomi 子进程（`YOMI_DATA_DIR` 注入、PATH 含
   `<data_dir>/bin`、cwd = 包目录），统一 shell 探测 + wrap_command +
   spawn_in_new_tree，管道并发读防 deadlock，120s 超时连树收掉。
   幂等 ensure（每次 install/refresh 都跑）；退出非零/超时 → 类型化
   `ExtError::Init` 上抛，install 整体报错。输出尾部（4KB）进报告
   与错误文本。manifest 校验：路径不出包根、不含空白/引号、常规
   文件 ≤1MB。
7. **写注册表 `ext.lock`**：包内容 hash 先算（lock 是包目录外单
   文件、天然不入各包 hash），然后 upsert `extensions/ext.lock` 里
   该扩展的条目并原子整表重写：source、rev、content_hash、资源清单、
   installed_at。单文件 = Cargo.lock 同款心智模型；并发由 install/
   remove 的全局注册表锁串行。全程只有两个落盘点：复制完成立刻
   落一条 **provisional** 条目（资源沿用旧记录，fresh 为空表——
   目录已是事实而挂载/cron 未走完，没它崩溃后重跑撞 occupied）；
   收尾落终值条目。**最终落盘失败 = install 整体报错**：报成功却
   没条目，remove 会按 foreign 拒绝，装了个不可收敛的孤儿。
   中间任一步失败都不落部分记录：provisional 已保证重跑收敛，
   remove 回滚 = 条目旧资源 ∪ 包目录扫描 ∪ cron 前缀清扫。

冲突时的原子性：不做跨 fs/sqlite 事务（做不了）；同一扩展名的
install/remove 由进程内全局注册表锁串行（单文件整表重写下互斥），
跨扩展的残余竞争靠"所有权可重入"
保证部分失败后重跑收敛到同一终态。包内容在碰槽位前先完整 walk
（含单文件 1MB 上限），超限即拒、不留半成品目录。

## remove 语义（精确回滚）

`name` 参数与 install 同一名字规则校验（字母开头 `[a-z0-9-]` ≤32），
`../` 穿越在入口硬拒。读已装目录（ext.lock 损坏/缺失则退化：
cron 按 `ext:<名>:` 前缀扫、挂载按指向判定 + 扫包目录）→ 删 cron
（store 层前缀查询，无分页漏删窗口）→ 摘挂载（**仅当** symlink 确认
指向本包；被用户换掉的留下并 warn）→ 删 `extensions/<名>`（仅当
注册表条目证明是本包装的内容）→ 目录与注册表条目随删除消失，
   注册表零残留（单文件，无 per-name 残留面）。
全程只动能证明属于自己的东西。处置细则：

- 挂载槽位被用户换成别的 symlink 或实体文件 → 留下，warn。
- `extensions/<名>` 槽位是实体目录时：仅当注册表有条目才
  `remove_dir_all`；无 lock（foreign，用户把自己的目录放进槽位）→ 留下，
  warn（绝不删用户数据）。
- 用户手动改过的 cron job（仍在 `ext:<名>:` 命名空间）→ 随前缀
  清扫删除：它属于扩展命名空间，改动随包走。

## list 语义

`yomi extension list`：每扩展一行——name、version、source、rev、
installed_at、资源计数（cron/hooks/bins/snippets）、健康状态：

- `ok`：注册表有条目且内容 hash 与目录现状一致。
- `modified`：lock 在但 hash 对不上（装完本地手改过包内容）。
- `foreign`：目录在但注册表无条目——用户手放，或 ext.toml
  损坏/解析失败（占位 manifest 可见，内容不可信）。
- `unreadable`：hash 重算时包文件读不了（权限/损坏）。
- `oversized`：目录里出现超过单文件上限（1 MiB）的文件——用户往
  包里丢了大文件；与权限/损坏问题的 `unreadable` 分开，给出可行动
  的诊断（移走大文件或重装）。

ensure 语义下"manifest 改了但 job 没更新"是设计内行为，v1 靠
remove+install（或重装同源）全量刷新。

## snippet 拼装

`SystemPromptBuilder` 增加 data_dir 输入，在**项目 memory（AGENTS.md）
之后、skills 索引之前**追加：

```
extensions/*/snippets/*.md    # 扩展名字典序 → 文件名序；缺目录 = 零成本
```

与 channel rules 同一风格：文件内容原文进 prompt，包一层最小标题
（`# Extension: <名>`）。无 snippet 的扩展零 prompt 成本（existence
gated）。拼装时机 = skills 同款快照（会话 spawn 时读，无运行时 IO）。

## bin 与 PATH 层

问题：扩展带的可执行文件（如 memory-system 的 `recall`）怎么被
agent 找到。答案不是"告诉 LLM 绝对路径"，而是**让命令名直接可解析**：

1. **kernel 改动一处**：`utils::env::inject_child_env`（所有 yomi 子进程
   的唯一入口——shell 工具、cron shell job、hook、外挂 tool 全过它）把
   `<data_dir>/bin` prepend 进 PATH。目录不存在也加（PATH 里有空目录
   无害），与 `prepend_exe_dir_to_path`（sidecar CLI 同版保证）是同一
   哲学的两个层：exe 目录归内核自带二进制，bin 目录归用户与扩展。
2. **install 挂载**：包内 `bin/<文件名>` → symlink 进 `<data_dir>/bin/`，
   所有权规则与 hooks 相同（指向判定、冲突拒绝）。
3. **模型侧发现**：snippet 用自然语言提命令名（"检索记忆跑
   `memory/recall <keyword>`"），PATH 负责解析——与模型用 `yomi`、
   `lark` 完全一样，prompt 里零路径泄漏。bin 目录本身也是可浏览的
   注册表，模型 `ls ~/.yomi/bin` 即可自查有什么命令。

不采用每扩展一个 bin 目录全部塞进 PATH（膨胀、顺序冲突）；单一扁平
bin + 所有权规则与 hooks/tools 挂载哲学一致。PATH prepend 意味着扩展
命令可遮蔽系统同名命令——信任边界与 hook 相同（能装扩展 = 能执行
daemon 子进程代码），文档写明。

## cron 收养的 ensure 细节

`create_cron_job` 同名短路时不校验不改写（既有语义直接复用）。扩展
cron 的 session 模板：不绑定固定 session（per-run 新会话），工作
目录取 manifest 条目的 `work_dir`（缺省数据目录 workspace），权限
等级由 kernel 按 config 重算、下限 caution——与 cron 子系统对所有
调用方的归一化完全一致，不信任包内声明。

**物化注意**：cron 消息文本在 install 时读进 cron 表——源仓后续改动
不影响已装内容（统一 copy，决策 10），**cron 消息也不更新**（ensure
不覆盖纪律）。改 cron 用 `yomi cron update`，或重装全量刷新。

## wire 方法

| 方法 | 参数 | 说明 |
|---|---|---|
| `extension_install` | `{source}` | source = GitHub 简写/URL 或本地目录；返回 name + hash + 每项资源 created/exists/skipped 报告 |
| `extension_list` | `{}` | extensions/ 扫描 + 健康状态 |
| `extension_remove` | `{name}` | 返回回滚清单 |
| `preview_system_prompt` | `{working_dir?, session_id?}` | 预览新会话完整 system prompt（与 spawn 同装配路径，逐字一致）；wire 33 新增 |

dispatcher / KernelApi / RemoteKernel 三处同加。CLI `yomi extension
install|list|remove` 全走 KernelApi（requires daemon，与 cron 一致）。

## 安装元数据：`ext.lock`（决策 11）

install 复制完、hash 计算之后，在 `extensions/<名>/` 里写
`ext.lock`（作者 manifest `ext.toml` 原封不动——等价 Cargo.lock
对 Cargo.toml）：

```toml
source = "owner/repo/ext/demo@main"     # 用户给的原始来源字符串
rev = "1a2b3c..."                        # git resolved commit sha（本地源无此行）
content_hash = "..."                     # blake3 十六进制；不含本 lock
installed_at = "2026-10-01T08:00:00Z"

[resources]
cron = ["ext:demo:tick"]                 # cron 全名
hooks = ["pre_tool_use/50-guard"]        # 相对 hooks/ 的路径
bins = ["recall"]                        # 文件名
snippets = ["memory.md"]
```

`extensions/` 目录本身就是注册表与索引，没有 sqlite 表、没有
extensions/ 级总 lock。ext.lock 是审计与展示数据，不参与正确性
（决策 2 不变）——它唯一参与判定的地方是"目录删除的归属证明"
（remove 只删带 lock 的实体目录）与"原位刷新的许可"（install 见
lock 才 allow_replace）。

## 安全与信任边界

- 扩展包的代码（hook/bin 脚本）以 daemon 子进程全权执行，与手写
  外挂等价；hook 可见含密钥的 tool_input；bin 命令经 PATH 可遮蔽系统
  同名命令。信任边界 = **能写 `$YOMI_DATA_DIR` 的主体**（与手写
  hooks 完全一致，包管理不降低也不升高这个门槛）。
- install 本身只写数据目录内路径；包内 `ext.toml` 的相对路径一律
  解析到包内（防 `../../` 逃逸——manifest 路径 join 后必须仍在
  包根之下，校验拒绝越界）。
- 卡片触发器命名空间 `ext_` 与本文无关（那是外挂体系的按钮处理器），
  扩展包 v1 不提供卡片触发器资源；要发卡走脚本回连。
- `extension_install` 的 path 参数是 daemon 侧文件系统路径：能调该
  wire 方法的主体（本机 socket / 持 ws token 者）可让 daemon 读它
  可读而调用方不可读的文件（经 message_file 收进 cron 表）——一个
  文件读取原语。这与 cron create 的既有能力同档，不降低门槛；ws
  暴露时务必配 socket_auth（transport 层已有警告）。
- symlink 模式已废（决策 10）：源仓写者的持续影响力收窄到"你重装
  该扩展那一次"——不重装，已装内容与其后续 commit 无关。这仍是
  信任边界的一部分：装 GitHub 源 = 信任该仓当前内容，文档写明。

## 分期落地

- **一期**：install/list/remove（health 含 ok/modified/foreign/
  unreadable/oversized 五态）+ snippet 拼装 + **bin/PATH 层** +
  wire 四方法（含 `preview_system_prompt`）+ `yomi extension` CLI +
  memory-system 包
  （yomi-extensions 仓，memory-system-setup skill 退休）。hooks 挂载
  同版做；**tools/skill 挂载不做**（2026-10-01 hrli 拍板，二期）。
- **二期（不占版，等真实需求）**：`extension update`（manifest 对比 +
  带确认清扫）、tools/skill 资源挂载、项目级两层（`.yomi/extensions/`
  覆盖全局，语义同 skill 三层）、install/uninstall 脚本钩子、agents/
  模板挂载、卡片触发器资源。
- 顺手（同版）：hook / 外挂 tool / 卡片触发器的 stdin JSON 加
  `"v": 1` 契约版本字段（只增不改承诺的落地）。

## 明确不做

- 不做扩展运行时的统一事件/总线：运行时仍是五个独立注册表。
- 不做依赖解析与扩展间依赖：包互相独立，组合靠约定（与 skill 同哲学）。
- 不做远程 registry / 版本求解：install 吃 GitHub URL（指定 ref 即
  版本）或本地目录；没有依赖图与版本区间，git ref 就是全部版本语言。
- 不做 install 的事务性回滚：靠所有权可重入收敛，不重跑事务。
- 不做卸载时的用户改动保留合并：remove 只删自己装的，用户改过的
  cron job 若在原位则被删（它属于扩展命名空间）——改动随包走。
- 不碰 AGENTS.md：snippet 是 kernel 拼装来源，不是文件改写目标。
