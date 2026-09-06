# hook 契约（pre_tool_use / daemon_up / daemon_down）

文件系统闸，零配置文件：目录即注册表，执行位即开关，无 reload（每次事件 readdir）。

## 目录布局

```
$YOMI_DATA_DIR/hooks/pre_tool_use/   # gate 点（另有 daemon_up/daemon_down 通知点，见末节）
├── 10-guard                         # 带执行位即生效（裸文件形态）
├── 15-pkg/                          # 目录形态：内含带执行位的 run 即生效，
│   ├── run                          #   伴生文件放同目录（dirname "$0" 即包目录）
│   └── patterns.txt
├── 20-audit -> /opt/hooks/audit     # 跟随符号链接（stow/nix 部署）
└── .draft                           # 隐藏文件跳过
```

执行语义：按**条目名**字典序串行；单 call 首个否决短路；多 call 间串行。隐藏文件、无执行位、无 `run`/不可执行的目录跳过；破损符号链接跳过不致命。state 目录与日志前缀按条目名（目录形态按目录名）。

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
| `2` | 否决；stderr 即原因，以 `[hook:<文件名>]` 前缀回流为 tool error 喂回 agent |
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

## daemon 生命周期 hook（daemon_up / daemon_down，v0.10.26 起）

通知型点，无否决语义：退出码只记 warn 日志，不影响 daemon、不中断后续脚本。同一目录约定（`hooks/<point>/` 下可执行文件、文件名字典序串行、执行位即开关）。

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

state 目录按事件点隔离：同一条目名挂两个点各占 `state/hooks/<point>/<条目名>/`，不共享。
