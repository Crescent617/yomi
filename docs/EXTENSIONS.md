# yomi 外挂

外挂 = **文件系统注册、kernel 以 spawn 驱动、stdio 契约的外部程序**。
没有 socket、没有 SDK、没有注册 RPC：把可执行文件放进数据目录的对应
文件夹，它就开始工作。两个表面，一个引擎：

| 表面 | 目录 | 干什么 |
|---|---|---|
| hook（拦截） | `<data_dir>/hooks/<事件>/` | kernel 事件发生时过闸，可否决 |
| tool（能力） | `<data_dir>/tools/<名>/` | 给 agent 增加可调用的工具 |

通用规则：执行位即开关（`chmod ±x` 即时生效）、无 reload（每次事件
readdir，目录是真相）、隐藏项跳过、符号链接跟随（stow/nix 部署友好）。
每个外挂有自己的持久状态目录（环境变量 `YOMI_STATE_DIR` 指向，daemon
惰性创建）：hook 为 `<data_dir>/state/hooks/<事件>/<脚本名>/`，tool 为
`<data_dir>/state/tools/<名>/`——去重水位、缓存、留档都放那。

子进程统一注入环境变量：`YOMI_EVENT`（事件标识：hook 为 hook point
名，tool 为 `tool`）、`YOMI_DATA_DIR`、`YOMI_STATE_DIR`、
`YOMI_SESSION_ID`（daemon 通知点不注入）。`pre_tool_use` 另有兼容变量
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
**fail-open 放行** + warn 日志——否决必须是显式行为。daemon 事件无
否决语义：退出码只记日志，不影响 daemon 也不中断后续脚本。

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

## 与 skill 的分工

skill 教 agent 怎么做事（知识进 prompt）；外挂是接在 kernel 上的程序
（spawn 执行）。要"模型按规程行事"写 skill；要"确定性执行/拦截"
写外挂。

## 迁移说明（v0.10.26）

v1 扩展（`ext_register`/`ext_pull`/`ext_result` 长连接注册 + config.toml
`[[extensions]]` supervised）已整体删除：wire 协议升至 30，旧 SDK
`examples/yomi_ext.py` 移除。source 路由（`ext_route` RPC）保留。
旧 config.toml 里的 `[[extensions]]` 段会被**静默忽略**（supervised
进程不再拉起），请删除该段并迁移到 `tools/` 目录。
