# 设计文档：ext —— yomi 外挂体系（hooks × tools）

取代：phase1（docs/archive/extension-phase1.md）的 socket+RAII 模型、
extension v2 草案。兼容性：ext 没人用，直接删；`hooks/` 目录原名保留，
零迁移。

## 决策记录（2026-09-06，hrli 拍板）

1. 目录形态：扁平 `hooks/ tools/`（曾含 `signals/`，见 7）。
2. timeout：hook 维持 30s 固定；tool manifest 可调，缺省 60s、
   上限 600s。
3. tool stdout 截断：复用 shell 工具现值，不新设 knob。
4. wire 旧三方法（ext_register/ext_pull/ext_result）：直接删，proto→30，
   不留废弃警告。（原目标 29；核查发现 29 已随 v0.10.24/25 发布——
   `git tag --contains 2e18d993`——破环性变更须再升。）
5. 项目级 `.yomi/{hooks,tools}/`：二期。
6. push 与 `yomi emit`：暂缓（`session send` 逃生舱覆盖）。
7. signals（B 期实现后删除）：曾按 watcher 形态设计并实现 `signals/`
   poll（signaler 循环、无 job 表、整分钟对齐拍、排程耗尽休眠语义），
   review 闭环后拍板删除——cron job + precheck 同构覆盖该场景
   （exit 0 放行 + stdout 追加进消息 / 非零静默跳过），独有增量
   （纯 stdout 消息、deliver 档位）撑不起一个独立概念。
   第一期 = hooks + tools。
8. daemon 生命周期 hook：`daemon_up`/`daemon_down`（hrli 命名拍板）。
   真实时序是 up 之后/down 之前：up 在服务就绪后于后台跑（不等），
   down 在关停流程中等完（且先等可能仍在飞的 up 链收尾——up/down
   不并发）。通知型无否决；常驻进程用户脚本自管（`&`/daemonize），
   不做 daemon 层异步。hook state 目录按事件点隔离：
   `state/hooks/<point>/<脚本名>/`（同名脚本挂两点不共享）。

## 推演与命名

四个已存在/已设计的东西是同一形状：**事件 → 文件系统注册的可执行文件
→ stdin JSON → 退出码策略**——

1. hook（pre_tool_use，v0.10.14）：事件=工具调用前（拦截）；
2. extension v2 草案：事件=agent 调用某具名工具（能力）；
3. memory.md 的三接缝（PreCompaction / CompactionSummary / SessionEnded）：
   事件=会话生命周期；
4. watcher：事件=外部世界出现信号（感知；方向相反，注册形态同构）——
   曾据此实现 `signals/`，后删除（决策 7）。

这类事物在 yomi 的传统里本就叫**外挂**（memory.md 的外挂记忆进程、
phase1 的外挂裁决）。本设计把词正式化：**外挂 = 文件系统注册、kernel
以 spawn 驱动、stdio 契约的外部程序**。用户侧表面 = 数据目录里两个
扁平复数目录（与 `workflows/` `pets/` `channels/` 同约定），目录名即
语义，不造伞概念；代码沿用 `extension` / `ext_route` 模块名，不造新
英文词。与 skill 的分工：skill 教 agent 怎么做事（知识进 prompt），
外挂是接在 kernel 上的程序（spawn 执行）。

差异只在三点：注册位置、触发规则、退出语义 ⇒ 一个 spawn 引擎 + 一张
事件表统吃。phase1 四端口落位：Gate=`hooks/`、Capability=`tools/`、
Sink=通知型 hook point（预留）、Source=ext_route 路由（push 路线预留）
与 `session send` 逃生舱。

## 目录树

```
$YOMI_DATA_DIR/
├── hooks/
│   ├── pre_tool_use/        # 拦截：裸文件或 <名>/run 目录，按条目名字典序
│   │   ├── 10-guard
│   │   └── 20-audit -> /opt/hooks/audit
│   ├── daemon_up/           # 通知：服务就绪后（随 yomi 启动其他进程）
│   └── daemon_down/         # 通知：关停流程中（随 yomi 停止其他进程）
├── tools/
│   └── stock_quote/         # 能力：子目录名 = 工具名
│       ├── tool.json        # manifest：desc/schema/level/timeout_secs
│       └── run              # 入口（执行位 = 开关）
└── state/                   # daemon 分配，YOMI_STATE_DIR 暴露
    ├── hooks/<point>/<脚本名>/   # 按事件点隔离（同名脚本挂两点不共享）
    └── tools/<名>/
```

通用规则与现 hook 一致：执行位即开关、无 reload（每次事件 readdir）、
隐藏项跳过、符号链接跟随、破损链接不致命。**文件系统即注册表**：
注册、停用、卸任都是目录操作，daemon 无任何注册 RPC。

## 事件表

| 事件 | 目录 | 入口格式 | 触发 | 多重性 | 退出语义 | 故障策略 | 超时 |
|---|---|---|---|---|---|---|---|
| `pre_tool_use` | `hooks/pre_tool_use/` | 裸文件或`<名>/run`目录，按条目名字典序 | 工具调用前 | N，首个否决短路 | 0=放行；2=否决（stderr 回流 agent）；其他=故障 | fail-open 放行+warn | 30s 固定 |
| `daemon_up` | `hooks/daemon_up/` | 裸文件或`<名>/run`目录，按条目名字典序 | 服务就绪后（后台跑，daemon 不等） | N 串行 | 忽略（通知型，无否决） | fail-silent warn | 30s 固定 |
| `daemon_down` | `hooks/daemon_down/` | 裸文件或`<名>/run`目录，按条目名字典序 | 关停流程中（等其跑完再拆） | N 串行 | 忽略（通知型，无否决） | fail-silent warn | 30s 固定 |
| `tool` | `tools/<名>/` | manifest+run | agent 调用该名 | 1/名 | 0=成功（stdout=结果）；非零=tool error（stderr 回流 agent） | fail-closed 报错+warn | manifest，缺省 60，上限 600 |
| `post_tool_use`（预留，本期不实现） | `hooks/post_tool_use/` | 裸文件 | 工具调用后 | N 串行 | 忽略 | fail-silent | 待定 |
| `pre_compact` / `session_end`（预留，接 memory.md） | `hooks/<event>/` | 裸文件 | 会话生命周期 | N 串行 | 事件定 | 事件定 | 事件定 |

新增事件类型的全部工作：定表上一行 + kernel 加一处 dispatch 调用点。

## 公共契约

**stdin**（单行 compact JSON，每事件类型的契约只增不改）：

| 字段 | 说明 |
|---|---|
| `event` | 事件类型标识：`pre_tool_use` / `tool`（hook 另有
`hook_event_name` 字段携带 hook point 名——v0.10.14 契约不动） |
| `tool_name` | 工具名（`pre_tool_use` 与 `tool` 有；一脚本多工具时分辨） |
| `session_id` | 会话 id |
| `cwd` | 进程 cwd |

**env**：`YOMI_EVENT`（事件标识：hook=hook point 名，tool="tool"；
hook 的 stdin 契约另有 `hook_event_name` 字段，v0.10.14 既有契约不动）、
`YOMI_SESSION_ID`（pre_tool_use 与 tool 有；daemon 通知点显式移除）、
`YOMI_DATA_DIR`、
`YOMI_STATE_DIR`（持久目录，惰性创建：hook 为
`<data_dir>/state/hooks/<point>/<脚本名>/`，tool 为
`<data_dir>/state/tools/<名>/`；tools 管缓存、hooks 管留档）。

**spawn 语义**（全事件共用，与现 hook 同机制）：setsid 进程组、超时按组
SIGKILL、tool 调用接取消（与超时同路径组杀，`Captured.cancelled` 区分）、
stderr 捕获上限 64KB、回流 agent 的文本截 2000 字符（前缀 `[ext:<名>]`）、
at-least-once（有副作用自行幂等）。

## tools 专则

- manifest `tool.json`：`desc`（原文进模型工具表）、`schema`、`level`
  （safe|caution|dangerous，缺省 caution 走审批卡）、`timeout_secs`
  （缺省 60，上限 600）。无 name 字段：目录名即工具名。清单坏 / `run`
  缺失 → 跳过+warn（pets invalid-package 先例）。
- 命名约束：字母开头 `[a-zA-Z0-9_-]`（provider 最紧交集）。目录名唯一
  ⇒ 外挂撞外挂不可能；撞内建 → 内建赢、warn 跳过；`tool_blocklist`
  regex 照常生效。
- 工具表在会话 spawn 时扫描合并（快照）；新会话 / `/clear` / idle
  respawn 后生效——口径与旧版一致。
- stdout 上限：复用 shell 工具的结果截断值。
- 并发天然：每次调用独立进程，无单 worker 限制。

## hooks 专则

`pre_tool_use` 语义逐项保留现 hook 行为：字典序串行、单 call 首个否决
短路、多 call 间串行、exit 2 的 stderr 作否决原因回流、故障 fail-open、
30s 固定超时。对 custom tool 的调用同样过闸（现行为不变，融合后只是
同一管线显式化）。未来 hook point（post_tool_use、pre_compact、
session_end……）以同级事件目录加入，接 memory.md 的接缝需求。

`daemon_up`/`daemon_down` 是首个通知型点（设计上的 Sink 端口落位）：
无否决语义、退出码只留痕、不接取消（down 必须跑完）、stdin 精简契约
（`{"event","cwd"}`，无 session，`YOMI_SESSION_ID` 显式移除）。语义
同步串行（同一引擎行为，无新概念）；差异只在触发位置——up 在服务
就绪后于后台任务里跑（daemon 不等，不挡开机，脚本可回连 CLI），
down 在关停流程中 await（运行时退出会回收子进程，不等则清理不可靠）。
常驻进程由用户脚本自行 `&`/daemonize（引擎 setsid，脚本返回后后台
孩子自然存活）。

## 管线示例

agent 调用 `stock_quote {"symbol":"600519"}`：

1. readdir `hooks/pre_tool_use/` → 字典序 spawn 各处理器（stdin 含
   `event=pre_tool_use`、`tool_name=stock_quote`）；任一 exit 2 → 否决，
   stderr 回流 agent；
2. 放行且过审批（manifest level）→ spawn `tools/stock_quote/run`，stdin
   含 `args`；exit 0 → stdout 作工具结果；非零/超时 → stderr 作 tool
   error 喂回 agent。

## 删除清单

- wire：`ext_register` / `ext_pull` / `ext_result` 删除（proto → 30，
  不留废弃警告）；`ext_route` 保留（push 路线预留）。
- kernel：ExtensionRegistry 全套（连接账本/RAII sweep/pull 队列/55s 心跳/
  in-flight 表/单 worker/连接归属校验）；`[[extensions]]` config 段；
  hook spawner 与 ext 进程代码合并为一个 spawn 引擎。
- `examples/yomi_ext.py` → 换两个示例（sh guard + python tool，各约 20
  行）。SDK 不复存在——这是特性不是缺失。
- 文档：EXTENSIONS.md 重写为《yomi 外挂》（用户文档，两个表面一个
  引擎）；yomi-self skill 的 references/hook.md 同步。

## 分期落地

- **A（一版）**：spawn 引擎统一（hook spawner 抽共用）+ `tools/` 上线
  （扫描合并进工具表）+ 删 ext v1 全套（proto → 30）。
- **B（已删）**：`signals/` poll 实现后经 review 闭环删除（决策 7），
  不进任何版本。
- 后续（不占版）：push + `yomi emit`（等真实需求）；项目级两层合并
  （语义同 skill：工作区赢）；预留 hook point 按 memory.md 落地节奏接入。

每版按既有轨道：对抗 review → 隔离 daemon 真链路 → tag → CI → tap。

## 明确不做

- `signals/` 信号目录：与 cron job + precheck 同构（决策 7），不重复造。
- push 模式与 `yomi emit`（本期；外部注入走 `session send` 逃生舱）。
- serve 模式（常驻进程/热状态）：留给 MCP 路线，不重建连接生命周期。
- `tool` 多处理器/fallback 链：一名一处理器。
- stdout 富内容（content parts）：契约只增不改，有真实需求再加。
- 预留 hook point（post_tool_use / pre_compact / session_end）本期不实现，
  只占事件表的行。
- HTTP webhook 入站：留给渠道适配器路线。
