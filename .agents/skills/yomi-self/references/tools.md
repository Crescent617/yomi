# tool 契约

文件系统注册，零配置文件：目录即注册表，执行位即开关。agent 会话 spawn 时扫描 `$YOMI_DATA_DIR/tools/` 合并进工具表（快照）——新会话 / `/clear` / idle respawn 后生效，无 reload。

## 目录布局

```
$YOMI_DATA_DIR/tools/stock_quote/
├── tool.json    # manifest
└── run          # 入口（执行位 = 开关；符号链接跟随）
```

目录名即工具名（manifest 无 name 字段）。命名约束：字母开头、仅 `[a-zA-Z0-9_-]`、≤ 64 字符——各 provider 的最紧交集。撞内建工具名时外挂让位（warn 日志跳过）。清单坏 / `run` 缺失 / 无执行位：跳过（配置坏 warn、开关关 debug）。

## tool.json

| 字段 | 必填 | 说明 |
|---|---|---|
| `desc` | ✓ | 工具描述，原文进模型工具表 |
| `schema` | ✓ | 参数 JSON Schema，原样透传 provider |
| `level` | | 审批级别 `safe` / `caution` / `dangerous`，缺省 `caution`（走审批卡） |
| `timeout_secs` | | 单次调用超时秒数，缺省 60，上限 600 |

## 调用契约

每次调用 spawn 一次 `run`：

- **stdin**：单行 compact JSON `{"event":"tool","session_id":"sess_…","cwd":"/work/dir","tool_name":"stock_quote","args":{…}}`（`args` = 模型给出的参数原始 JSON）
- **进程 cwd** = 会话工作目录
- **exit 0** → stdout 作工具结果（超出输出预算按 shell 工具同值截断）
- **非零 / 超时 / spawn 失败** → fail-closed：stderr 以 `[ext:<名>]` 前缀作 tool error 喂回 agent（截 2000 字符）

超时按进程组 SIGKILL（setsid，后裔连坐）；stdout/stderr 捕获上限各 64KB。每次调用独立进程，无状态、天然并发；跨调用要留状态用 `YOMI_STATE_DIR`。

## 环境变量

| env | 值 | 用途 |
|---|---|---|
| `YOMI_EVENT` | `tool` | 一脚本多用途时分辨 |
| `YOMI_SESSION_ID` | 同 stdin | 回连 yomi（session cat/send 等） |
| `YOMI_DATA_DIR` | 数据目录 | 定位 yomi 资产 |
| `YOMI_STATE_DIR` | `<data_dir>/state/tools/<名>/` | 持久状态目录（缓存/留档），daemon 惰性创建 |

## 示例

yomi 仓库 `examples/tools/stock_quote/`（python3，约 20 行：读 stdin JSON → 取 `args.symbol` → 输出结果）。
