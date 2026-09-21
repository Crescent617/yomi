# hook 契约

目录即注册表：执行位即开关，无 reload（每次事件 readdir）。

## 目录布局

```
$YOMI_DATA_DIR/hooks/pre_tool_use/   # gate 点（另有 turn_start/turn_end 与 daemon_up/daemon_down 通知点，见末节）
├── 10-guard                         # 带执行位即生效（裸文件形态）
├── 15-pkg/                          # 目录形态：内含带执行位的 run 即生效，
│   ├── run                          #   伴生文件放同目录（dirname "$0" 即包目录）
│   └── patterns.txt
├── 20-audit -> /opt/hooks/audit     # 跟随符号链接（stow/nix 部署）
└── .draft                           # 隐藏文件跳过
```

执行语义：按**条目名**字典序串行；单 call 首个否决短路；多 call 间串行。隐藏文件、无执行位、无 `run`/不可执行的目录跳过；破损符号链接跳过不致命。

## stdin schema

单行 compact JSON，五字段（`snake_case`，稳定契约只增不改）：

| 字段 | 类型 | 说明 |
|---|---|---|
| `session_id` | string | 会话 id；要对话历史用 `yomi session cat "$YOMI_SESSION_ID"` |
| `cwd` | string | session working_dir（脚本进程 cwd 同此） |
| `hook_event_name` | string | 恒 `"pre_tool_use"` |
| `tool_name` | string | yomi 工具名（`shell`/`read`/`write`/`grep`/…） |
| `tool_input` | object | 模型给出的工具参数原始 JSON，内核不校验不裁剪 |

例：

```json
{"session_id":"sess_01M1…","cwd":"/work/dir","hook_event_name":"pre_tool_use","tool_name":"shell","tool_input":{"command":"rm -rf /tmp/x"}}
```

## 环境变量

| env | 值 | 用途 |
|---|---|---|
| `YOMI_HOOK_EVENT` | `pre_tool_use` | 一脚本挂多点时分辨触发点 |
| `YOMI_SESSION_ID` | 同 stdin | 回连 yomi（session cat/send 等） |
| `YOMI_DATA_DIR` | 数据目录 | 定位 yomi 资产 |
| `YOMI_STATE_DIR` | `<data_dir>/state/hooks/<point>/<条目名>` | 持久状态目录（留档/缓存），daemon 惰性创建（v0.10.26 起） |

## 退出码

| 码 | 语义 |
|---|---|
| `0` | 放行（stdout 丢弃） |
| `2` | 否决；stderr 即原因，以 `[hook:<条目名>]` 前缀回流为 tool error 喂回 agent |
| 其他非零 / 超时 30s / spawn 失败 | hook 自身故障 → fail-open 放行 + warn 日志（否决必须是显式行为） |

超时按进程组 SIGKILL（setsid，后裔连坐）。stderr 捕获上限 64KB，回流给 agent 的否决原因截断到 2000 字符。进程在闸与落盘之间被杀时恢复会重过 hook（at-least-once）——有副作用的 hook 须自行幂等。

## 工具过滤

内核不设 matcher：stdin 里有 `tool_name`，不关心的工具直接 `exit 0`：

```sh
#!/bin/sh
# 只拦 shell 里的 rm -rf
jq -e '.tool_name == "shell"' >/dev/null || exit 0
jq -r '.tool_input.command' | grep -q 'rm -rf' || exit 0
echo 'refused: rm -rf' >&2
exit 2
```

要留档再过滤时先 `cat > 文件` 存盘，后续一律从文件读（stdin 只能读一次）。

## 与 Claude Code 的已知差异

无 `transcript_path` 字段；超时固定 30s（CC 默认 60s，30–60s 的慢 guard 语义反转：CC 否决、此处 fail-open）；不支持 CC 的 stdout JSON 高级协议（`permissionDecision`）；非 0/2 退出码的 stderr 不进用户界面（CC 会展示）。

## turn 生命周期 hook

通知型点（v0.10.37 起），无否决语义：退出码只记 warn 日志，fail-open，
不中断后续脚本。turn = agent 锚定一条 user 消息到回到 Idle 的完整周期
（= checkpoint 粒度）。

| 点 | 触发 |
|---|---|
| `turn_start` | 锚定 user 消息开工时（Idle→Streaming，`Turn` 创建成功） |
| `turn_end` | turn 关闭时（任何非 Idle→Idle，外加 rewind 取消与 loop 退出防御） |

**不变量：每次 `turn_start` 恰好配一次 `turn_end`**；SIGKILL/崩溃例外
（`turn_end` 不保证，攒状态的脚本把 `turn_start` 当持久标记）。单条
hook **30s 硬顶**（超时按进程组强杀，fail-open）。同 session 的
hook 链串行是结构保证（内核按 session 加锁，跨 agent respawn 也不
并发）；跨 session 并发——同名 hook 的 state 目录共享，落盘按
`session_id` 分文件。subagent 的 turn 同样触发（`session_id` 区分）。

收尾窗口对外可见：turn 收尾期间 session 状态为 `winding_down`
（非 Idle，算 running）——daemon 关停会等（1min 上界）、渠道回复
与 subagent 转运在 hook 链后才发、`Stopped` 在 hook 链完成后才到。
已知限制：`/stop` 后链总耗时超 ~35s 时 conductor 仍 detach respawn，
hook 链仍由锁保序，但旧 turn 迟到的 `Stopped` 可能落进新 turn
（病理窗口，hook 保持短小即可避免）。

stdin（单行 JSON，契约只增不改）：

```json
// turn_start
{"session_id":"sess_…","cwd":"/work/dir","hook_event_name":"turn_start",
 "user_msg_id":"msg_…","is_steer":false,"input_preview":"前 200 字符…"}
// turn_end（error 字段仅 failed 时出现）
{"session_id":"sess_…","cwd":"/work/dir","hook_event_name":"turn_end",
 "user_msg_id":"msg_…","stop_reason":"completed","duration_ms":12345,
 "iterations":7}
```

`stop_reason`：`completed`/`failed`/`cancelled`/`shutdown`/
`max_iterations`/`rewound`（`unknown` = 内核路径遗漏的防御兜底，正常
不应出现）。

边界语义：mid-turn 的 steer 插队**不**新开 turn（idle 态 steer 会，
`is_steer=true`）；`user_msg_id` 可重复（`/continue`、rewind 后重做
以同一锚消息再开 turn——去重不能只看 msg id）；`input_preview` 必须
取自 payload（锚定消息经 bus 异步落盘，`session cat` 此刻可能还读
不到）。进程 cwd = 会话工作目录，注入 `YOMI_SESSION_ID`（可回连
CLI），`YOMI_HOOK_EVENT` 不注入。

配套时序（同版落地）：`Stopped` lifecycle 事件推迟到 turn 全关
（checkpoint + `turn_end` hook 链）之后发射——headless `yomi run`
看到 `Stopped` 即退出，顺序反了 hook 会被进程退出截断；事件消费者
看到的"结束"= 真正全部结束。示例：`examples/hooks/turn_end/10-audit`。

## daemon 生命周期 hook

通知型点，无否决语义：退出码只记 warn 日志，不影响 daemon、不中断后续脚本。

| 点 | 触发 | daemon 等吗 |
|---|---|---|
| `daemon_up` | 服务就绪**后**（socket 已在服务） | 不等（后台跑，不挡开机；脚本可回连 CLI） |
| `daemon_down` | 关停信号触发后、kernel 拆除前（socket 仍在服务，可回连 CLI） | 等（每条 30s 上限兜底；先等可能仍在飞的 up 链收尾，up/down 不并发） |

stdin 是精简契约 `{"event":"daemon_up","cwd":"<data_dir>"}`（无 session）；env 注入 `YOMI_EVENT`/`YOMI_DATA_DIR`/`YOMI_STATE_DIR`，`YOMI_SESSION_ID` 显式移除。脚本 cwd = 数据目录。

常驻进程随 yomi 起落的写法（脚本要立即返回，后台孩子经引擎 setsid 自然存活；两个点都要幂等——重启 = down 全跑 + up 全跑）：

```sh
# hooks/daemon_up/10-ollama —— 幂等 + 所有权标记（只停自己拉起的）
if ! pgrep -x ollama >/dev/null 2>&1; then
    nohup ollama serve >/dev/null 2>&1 &
    touch "$YOMI_STATE_DIR/started-by-hook"
fi
exit 0

# hooks/daemon_down/10-ollama —— 有标记才停（state 按点隔离，跨点寻址）
MARK="$YOMI_DATA_DIR/state/hooks/daemon_up/10-ollama/started-by-hook"
if [ -f "$MARK" ]; then
    pkill -x ollama 2>/dev/null
    rm -f "$MARK"
fi
exit 0
```

state 目录按事件点隔离，同名条目挂两点不共享。
