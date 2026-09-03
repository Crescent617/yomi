# hook 契约（pre_tool_use）

文件系统闸，零配置文件：目录即注册表，执行位即开关，无 reload（每次事件 readdir）。

## 目录布局

```
$YOMI_DATA_DIR/hooks/pre_tool_use/   # 一期唯一 hook point
├── 10-guard                         # 带执行位即生效
├── 20-audit -> /opt/hooks/audit     # 跟随符号链接（stow/nix 部署）
└── .draft                           # 隐藏文件跳过
```

执行语义：文件名字典序串行；单 call 首个否决短路；多 call 间串行。隐藏文件、无执行位、子目录跳过；破损符号链接跳过不致命。

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

## 退出码

| 码 | 语义 |
|---|---|
| `0` | 放行（stdout 丢弃） |
| `2` | 否决；stderr 即原因，以 `[hook:<文件名>]` 前缀回流为 tool error 喂回 agent |
| 其他非零 / 超时 30s / spawn 失败 | hook 自身故障 → fail-open 放行 + warn 日志（否决必须是显式行为） |

超时按进程组 SIGKILL（setsid，后裔连坐）。进程在闸与落盘之间被杀时恢复会重过 hook（at-least-once）——有副作用的 hook 须自行幂等。

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
