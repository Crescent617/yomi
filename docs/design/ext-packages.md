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

1. **命名：命令 `yomi extension`，目录 `extensions/`，内部模块 `pkg`**。
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
7. **v1 无 install 脚本**：纯声明式。memory-system 作为零脚本验证案例；
   脚本钩子（install/uninstall）等真实需求出现再加（加法是兼容的）。
8. **bin 走 `<data_dir>/bin` + PATH prepend，不告诉模型路径**：命令名
   即接口，snippet 只提名字。kernel 改动收在 `inject_child_env` 一处，
   所有子进程（shell/cron/hook/tool）同时获得解析能力。
9. **v1 资源范围 = cron + hooks + snippets + bin**（2026-10-01 hrli
   拍板）；tools/skill 挂载二期。包格式和所有权规则为此留好了位置。

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

1. **收编** `extensions/<name>`：空 → symlink 到源（默认）或 `--copy`
   实体复制；已指向同一源 → 幂等通过；指向别的源 → 拒绝；实体目录
   → 仅当安装记录证明是上次 `--copy` 复制的才原位刷新（先拷到隐藏
   临时目录再 swap，不留"删完没拷上"的窗口），否则拒绝（提示先
   remove）。
2. **挂载** hooks/bin：逐槽位三种情况——空则建 symlink（创建撞
   AlreadyExists = 并发竞争，退到重判定）；已指向本包则跳过；其他
   （用户文件/他扩展/破损 link）→ 整体拒绝并报占用清单（先建后查，
   撞了不自动回滚已建部分，重跑 install 即收敛——所有权规则保证
   重跑安全）。
3. **收养 cron**：逐条 `create_cron_job`（ensure：同名即返回不动，
   缺才建）。输出区分 `created` / `exists (untouched)`。
4. **写记录**：sqlite upsert（放最后：记录存在 ⇒ 资源大概率在）。

冲突时的原子性：不做跨 sqlite/fs 事务（做不了），靠"所有权可重入"
保证部分失败后重跑收敛到同一终态。sqlite 侧撞名有唯一索引兜底并发。

## remove 语义（精确回滚）

`name` 参数与 install 同一名字规则校验（字母开头 `[a-z0-9-]` ≤32），
`../` 穿越在入口硬拒。读安装记录（记录损坏/缺失则退化：cron 按
`ext:<名>:` 前缀扫、挂载按指向判定）→ 删 cron（store 层前缀查询，
无分页漏删窗口）→ 摘挂载（**仅当** symlink 确认指向本包；被用户换掉
的留下并 warn）→ 删 `extensions/<名>` → 删记录。全程只动能证明属于
自己的东西。处置细则：

- 挂载槽位被用户换成别的 symlink 或实体文件 → 留下，warn。
- `extensions/<名>` 槽位是实体目录时：仅当记录证明是我们 `--copy`
  复制的才 `remove_dir_all`；无记录或 symlink 模式 → 留下，warn
  （用户把自己的目录放进槽位，绝不删用户数据）。
- 用户手动改过的 cron job（仍在 `ext:<名>:` 命名空间）→ 随前缀
  清扫删除：它属于扩展命名空间，改动随包走。

## list 语义

`yomi extension list`：每扩展一行——name、version、source、
installed_at、资源计数（cron/hooks/tools/snippets/skill）、健康状态
（源 symlink 是否存活 = `ok` / `broken source`）。不加漂移检测的
list 先上线；ensure 语义下"manifest 改了但 job 没更新"是设计内行为，
v1 靠 remove+install 全量刷新。

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

**物化注意**：cron 消息文本在 install 时读进 cron 表——源仓
`git pull` 后 hooks/bin/snippets 即时更新，**cron 消息不更新**
（ensure 不覆盖纪律）。改 cron 用 `yomi cron update`，或 remove +
install 全量刷新。

## wire 方法

| 方法 | 参数 | 说明 |
|---|---|---|
| `extension_install` | `{path, copy: bool}` | 返回 name + 每项资源 created/exists/skipped 报告 |
| `extension_list` | `{}` | 安装记录 + 健康状态 |
| `extension_remove` | `{name}` | 返回回滚清单 |

dispatcher / KernelApi / RemoteKernel 三处同加。CLI `yomi extension
install|list|remove` 全走 KernelApi（requires daemon，与 cron 一致）。

## 安装记录 schema（yomi.db，migrations 追加）

```sql
CREATE TABLE ext_installs (
    name         TEXT PRIMARY KEY,
    source       TEXT NOT NULL,          -- 安装时的源路径（绝对）
    mode         TEXT NOT NULL,          -- 'symlink' | 'copy'
    version      TEXT NOT NULL,
    resources    TEXT NOT NULL,          -- JSON：{cron:[], hooks:[], bins:[], snippets:[]}
    installed_at TEXT NOT NULL           -- RFC 3339
);
```

**正确性不依赖此表**（决策 2）：它回答"装了什么、从哪来"，不回答
"能不能删"——后者由指向判定与前缀清扫回答。

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
- symlink 模式下，能持续写扩展源仓的主体持续持有 bin/hook 执行权
  （pull 即生效）——信任边界同手写 hooks，不止是"安装那一刻"。

## 分期落地

- **一期**：install/list/remove + snippet 拼装 + **bin/PATH 层** +
  wire 三方法 + `yomi extension` CLI + memory-system 包
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
- 不做远程 registry / 版本求解：install 吃本地路径；git 更新是源仓库
  自己的事（symlink 模式天然拿到 pull 后的新内容）。
- 不做 install 的事务性回滚：靠所有权可重入收敛，不重跑事务。
- 不做卸载时的用户改动保留合并：remove 只删自己装的，用户改过的
  cron job 若在原位则被删（它属于扩展命名空间）——改动随包走。
- 不碰 AGENTS.md：snippet 是 kernel 拼装来源，不是文件改写目标。
