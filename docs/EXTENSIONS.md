# yomi 外挂

外挂 = **文件系统注册、kernel 以 spawn 驱动、stdio 契约的外部程序**。
没有 socket、没有 SDK、没有注册 RPC：把可执行文件放进数据目录的对应
文件夹，它就开始工作。三个表面，一个引擎：

| 表面 | 目录 | 干什么 |
|---|---|---|
| hook（拦截） | `<data_dir>/hooks/<事件>/` | kernel 事件发生时过闸，可否决 |
| tool（能力） | `<data_dir>/tools/<名>/` | 给 agent 增加可调用的工具 |
| 卡片触发器（交互） | `<data_dir>/channels/feishu_card_triggers/<名>/` | 用户点飞书卡片按钮时执行 |

通用规则：执行位即开关（`chmod ±x` 即时生效）、无 reload（每次事件
readdir，目录是真相）、隐藏项跳过、符号链接跟随（stow/nix 部署友好）。
每个外挂有自己的持久状态目录（环境变量 `YOMI_STATE_DIR` 指向，daemon
惰性创建）：hook 为 `<data_dir>/state/hooks/<事件>/<脚本名>/`，tool 为
`<data_dir>/state/tools/<名>/`，卡片触发器为
`<data_dir>/state/channels/feishu_card_triggers/<名>/`——去重水位、
缓存、留档都放那。

子进程统一注入环境变量：`YOMI_EVENT`（事件标识：hook 为 hook point
名，tool 为 `tool`，卡片触发器为 `card_trigger`）、`YOMI_DATA_DIR`、
`YOMI_STATE_DIR`、`YOMI_SESSION_ID`（daemon 通知点与卡片触发器不
注入）。`pre_tool_use` 另有兼容变量
`YOMI_HOOK_EVENT`（同 `YOMI_EVENT`；daemon 通知点没有也不继承）。
回连 yomi 走 CLI（如
`yomi session cat "$YOMI_SESSION_ID"`），不碰 socket。

## hook：事件闸与生命周期通知

`<data_dir>/hooks/<事件>/` 下的条目按**条目名字典序**串行执行。条目
两种形态：带执行位的**裸文件**，或含带执行位 `run` 的**目录**（与
tools 同约定——伴生文件放进自己包里，`dirname "$0"` 即得；state 目录
与日志前缀按目录名）。无执行位 `run` 的目录视为开关关。
事件点：

| 事件 | 触发 | 语义 |
|---|---|---|
| `pre_tool_use` | 每次工具调用前 | **闸门**：可否决（exit 2） |
| `turn_start` | agent 锚定一条 user 消息开工时 | 通知：turn 生命周期开始 |
| `turn_end` | turn 关闭时（含取消/失败/rewind） | 通知：turn 生命周期结束 |
| `daemon_up` | daemon 服务就绪**后**（socket 已在服务） | 通知：随 yomi 启动其他进程 |
| `daemon_down` | daemon 关停流程**中**（拆除前） | 通知：随 yomi 停止其他进程 |

```
hooks/pre_tool_use/
├── 10-guard          # 先跑
└── 20-audit -> /opt/hooks/audit
```

**stdin**（单行 JSON，契约只增不改）：

```json
{"session_id":"sess_…","cwd":"/work/dir","hook_event_name":"pre_tool_use",
 "tool_name":"shell","tool_input":{"command":"rm -rf /tmp/x"}}
```

（daemon 事件是精简契约 `{"event":"daemon_up","cwd":"<data_dir>"}`，无
session；`YOMI_SESSION_ID` 不注入。）

**退出码**：`0`=放行；`2`=否决，stderr 即原因（带 `[hook:<文件名>]`
前缀回流给 agent）；其他非零/超时（固定 30s）= hook 自身故障，
**fail-open 放行** + warn 日志——否决必须是显式行为。通知型事件
（`turn_*`、daemon）无否决语义：退出码只记日志，不影响流程也不
中断后续脚本。

**turn 生命周期钩子**：turn = agent 锚定一条 user 消息到回到 Idle
的完整周期（= checkpoint 粒度）。**不变量：每次 `turn_start` 恰好配
一次 `turn_end`**；`stop_reason` 区分出路（`completed`/`failed`/
`cancelled`/`shutdown`/`max_iterations`/`rewound`；`unknown` 是内核
路径遗漏的防御兜底，正常不应出现）。进程被 SIGKILL/崩溃是例外：
`turn_end` 不保证到达，攒状态的脚本把 `turn_start` 当持久标记用。
单条 hook **30s 硬顶**，超时按进程组强杀（fail-open）——慢脚本
（如调 LLM 固化记忆的）要把预算控制在 30s 内。同 session 的
hook 链**串行是结构保证**（内核按 session 加锁，跨 agent respawn
也不并发），不同 session 并发——同名 hook 的 state 目录跨 session
共享，落盘请按 `session_id` 分文件。subagent 的 turn 同样触发
（payload 里 `session_id` 区分）。

**收尾窗口对外可见**：turn 收尾（checkpoint + hook 链）期间 session
状态是 `winding_down`（非 Idle，算 running）——daemon 关停会等它
（35s 上界，join_all 并行）、渠道回复与 subagent 转运在 hook 链后才发出、
`Stopped` 事件在 hook 链完成后才到达。已知限制：hook 条数无上限，
`/stop` 后若链总耗时超过约 35s（5s + 一条的上界），conductor 仍
detach respawn——此时 hook 链仍由 session 锁保序，但旧 turn 迟到
的 `Stopped` 可能落进新 turn（病理窗口，hook 保持短小即可避免）。

几个边界语义：

- mid-turn 的 steer 插队**不**新开 turn；idle 态的 steer 会开 turn
  （`is_steer=true`）。
- `user_msg_id` 可重复：`/continue`、rewind 后重做都会以同一锚消息
  再开 turn——去重不能只看 msg id，配 `turn_end` 成对计数。
- `input_preview` 必须取自 payload：锚定消息经 bus 异步落盘，
  `turn_start` 触发瞬间 `session cat` 可能还读不到它。

```json
// turn_start
{"session_id":"sess_…","cwd":"/work/dir","hook_event_name":"turn_start",
 "user_msg_id":"msg_…","is_steer":false,"input_preview":"前 200 字符…"}
// turn_end
{"session_id":"sess_…","cwd":"/work/dir","hook_event_name":"turn_end",
 "user_msg_id":"msg_…","stop_reason":"completed","duration_ms":12345,
 "iterations":7}
```

`error` 字段仅 `stop_reason == "failed"` 时出现。进程 cwd = 会话工作
目录，注入 `YOMI_SESSION_ID`（可回连 CLI 干活，如 turn_end 里
`session cat` 抽记忆）。示例：`examples/hooks/turn_end/10-audit`。

**生命周期钩子的用法**：`daemon_up` 在后台跑、不挡开机，脚本可立即回连
CLI；要常驻进程就在脚本里放后台（`nohup … &`），脚本本身立即返回。
**注意进程组连坐**：脚本和它的后台孩子在同一进程组——脚本拖满 30 秒
被组杀时，后台孩子一起死（所以"拉起服务"的脚本要快去快回；需要等
就绪的自己轮询后退出）。`daemon_down` 会被等待跑完（每条 30s 上限
兜底），关停等待还包含可能在飞的 `daemon_up` 链收尾——这些时间都计
入 `daemon stop` 的 90 秒强杀兜底线，hook 保持短小。两个点都要幂等
（重启 = down 全跑 + up 全跑）。成对示例：
`examples/hooks/daemon_up/10-ollama` 与
`examples/hooks/daemon_down/10-ollama`。

不关心的工具直接 `exit 0`（stdin 里有 `tool_name`，内核不设 matcher）。
有副作用的 hook 自行幂等（at-least-once）。示例：
`examples/hooks/pre_tool_use/10-guard-rm`。

## tool：自定义工具

`<data_dir>/tools/<名>/` 一个目录就是一只工具：

```
tools/stock_quote/
├── tool.json     # 清单
└── run           # 入口（可执行）
```

`tool.json`：

```json
{
  "desc": "查询股票伪实时报价（原文进模型工具表）",
  "schema": {"type": "object", "properties": {"symbol": {"type": "string"}}, "required": ["symbol"]},
  "level": "safe",
  "timeout_secs": 60
}
```

- `level`：`safe`（免审批）| `caution`（每次审批，缺省）| `dangerous`。
- `timeout_secs`：缺省 60，上限 600。
- 目录名即工具名：字母开头、仅 `[a-zA-Z0-9_-]`（provider 最紧交集）。
  与内建工具撞名时内建赢（warn 跳过）；`tool_blocklist` 同样生效。

**调用**：agent 每调一次，kernel spawn 一次 `run`（cwd = 会话工作目录）。
stdin 单行 JSON：

```json
{"event":"tool","session_id":"sess_…","cwd":"/work/dir",
 "tool_name":"stock_quote","args":{"symbol":"600519"}}
```

**返回**：`exit 0` → stdout 即工具结果（长度按 shell 工具同口径截断）；
非零/超时/spawn 失败 → stderr（截 2000 字符，前缀 `[ext:<名>]`）作为
tool error 喂回 agent。示例：`examples/tools/stock_quote/`（python，
20 行，无 SDK）。

工具表在会话 spawn 时扫描合并；新会话 / `/clear` / idle respawn 后生效。

## 飞书卡片触发器

`<data_dir>/channels/feishu_card_triggers/<名>` 一个带执行位的文件
就是一只触发器：卡片按钮的 value 写 `{"action":"ext_<名>", ...}`，
用户点击按钮即执行（按名路由，一个按钮一个处理器）。执行位开关与
hook 相同；命名约束字符集同 tools（字母开头 `[a-zA-Z0-9_-]`），长度
上限 128（名字不进模型，非 provider 约束）。点击未注册的名字会回
一条"未知触发器"提示。

发卡不用 yomi 参与：用同一个 bot 的任意方式发卡即可（如 lark-cli
发 interactive 消息），按钮回调按应用投递，天然回到 daemon。

**stdin**（单行 JSON，契约只增不改）：

```json
{"event":"card_trigger","name":"publish","channel":"feishu",
 "operator_open_id":"ou_…","operator_union_id":null,
 "chat_id":"oc_…","message_id":"om_…","token":"c-…",
 "value":{"action":"ext_publish","id":1}}
```

**改卡**：`token` 是回调 token，点击后 30 分钟内可用「延时更新消息
卡片」接口原地改卡；窗口期外用 `message_id` 调「更新消息卡片」接口
（须发卡应用的身份）。凭证不进 stdin，脚本自行经 lark-cli /
OpenAPI 调用。`value` 全文透传——发卡时塞进去的 id、状态都在
里面；要关联会话自己把 sid 塞进 value，脚本再调
`yomi session send`。

**退出码**：通知型无否决，`0` 成功，非零/超时（固定 30s）只记 warn
日志。无会话语义：`YOMI_SESSION_ID` 不注入，cwd 为数据目录。
at-least-once，有副作用的脚本自行幂等。

**权限与安全**：点击不过 channel 用户闸（`blocked_users` /
`allowed_users` 不拦 `ext_`）——权限归触发器脚本自管，拿
`operator_open_id` 自行判断。`value` 是聊天成员可影响的输入，脚本
把它当不可信数据（别直接拼进 shell 命令 / SQL）。

## 与 skill 的分工

skill 教 agent 怎么做事（知识进 prompt）；外挂是接在 kernel 上的程序
（spawn 执行）。要"模型按规程行事"写 skill；要"确定性执行/拦截"
写外挂。

## 迁移说明（v0.10.26）

v1 扩展（`ext_register`/`ext_pull`/`ext_result` 长连接注册 + config.toml
`[[extensions]]` supervised）已整体删除：wire 协议升至 30，旧 SDK
`examples/yomi_ext.py` 移除。source 路由（`ext_route` RPC）保留。
