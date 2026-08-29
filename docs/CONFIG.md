# Config

支持 TOML 配置文件和环境变量两种方式。默认读取 `~/.yomi/config.toml`，可用 `--config` 或 `YOMI_CONFIG` 指定路径。

文件顶部可加 schema 指令以获得编辑器校验与补全：

```toml
#:schema https://raw.githubusercontent.com/Crescent617/yomi/main/docs/config-schema.json
```

## 配置示例

```toml
auto_approve = "safe"
data_dir = "~/.yomi"

[[models]]
name = "default"
provider = "anthropic"
model_id = "claude-sonnet-4-5"
endpoint = "https://api.anthropic.com"
api_key = "sk-..."

[agent]
default_model = "default"
max_iterations = 100

[env]
KIMI_AGENT_API_KEY = "sk-..."
```

---

## `[[models]]` — 模型

至少一个模型，运行时经 `agent.default_model` 选择。

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `name` | string | 模型标识名 | `"default"` |
| `provider` | string | `openai` / `anthropic` / `openai_response` | `"openai"` |
| `model_id` | string | 实际 API 模型 ID | 空（需配置） |
| `endpoint` | string | API 基础地址 | 空（需配置） |
| `api_key` | string | API 密钥 | 空（需配置） |
| `max_tokens` | integer | 单次最大输出 token | — |
| `temperature` | float | 温度 | — |
| `fallback_model_id` | string | 降级模型 ID | — |
| `sse_timeout_secs` | integer | SSE 流超时（秒） | `30` |
| `context_window` | integer | 上下文窗口大小 | `131072` |
| `headers` | object | 额外 HTTP 请求头 | `{}` |

| provider | 说明 |
|---|---|
| `openai` | Chat Completions API，兼容 Kimi 等 OpenAI 协议服务 |
| `openai_response` | Responses API，适用 GPT-5 / o 系列推理模型 |
| `anthropic` | Messages API |

**Thinking（按模型配置，块为 `[models.thinking]`）：**

| 字段 | 说明 | 适用 |
|---|---|---|
| `enabled` | 启用 thinking | Claude / o 系列 |
| `budget_tokens` | thinking token 预算 | Claude |
| `effort` | `low` / `medium` / `high` | o 系列 |

> `openai_response` 在 `thinking.enabled = true` 时不发送 `temperature`（推理模型不支持）；其余 provider 原样发送。

---

## `[agent]` — Agent 行为

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `name` | string | Agent 身份名，替换 system prompt 的 `{{name}}` | `"Yomi"` |
| `default_model` | string | 默认模型标识名 | `"default"` |
| `max_iterations` | integer | 单轮最大迭代次数 | `100` |
| `enable_subagent` | boolean | 允许 spawn 子 agent | `true` |
| `system_prompt` | string | 自定义系统提示（支持 `{{name}}`） | 内置 |
| `tool_blocklist` | string[] | 工具禁用列表（正则） | `[]` |
| `max_tool_output_length` | integer | 最大工具输出（字节） | `40000` |

**`[agent.compactor]` — 上下文压缩：**

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `micro_compact_enabled` | boolean | 全量压缩前先微压缩（改写旧工具结果，影响 prompt cache） | `false` |
| `threshold_ratio` | float | 触发压缩的上下文比例 | `0.9` |
| `keep_recent_messages` | integer | 全量压缩保留的最近消息数 | `0` |
| `keep_recent_tool_results` | integer | 微压缩保留的最近工具结果数 | `5` |
| `summary_max_tokens` | integer | 压缩摘要 token 上限 | `10240` |

---

## 顶层字段

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `auto_approve` | string | `safe` / `caution` / `dangerous` | `"safe"` |
| `data_dir` | string | 数据目录（支持 `~`） | `"~/.yomi"` |
| `log_dir` | string | 日志目录 | `<data_dir>/logs` |
| `skill_folders` | string[] | 技能目录，按优先级从低到高排列：同名 skill 由靠后的目录胜出 | `~/.agents/skills`、`<data_dir>/skills` |
| `max_checkpoints` | integer | 每会话检查点上限 | `5` |
| `socket_auth_hash` | string | daemon socket 鉴权哈希（`blake3:<hex>`，`yomi daemon auth-hash --generate` 生成）；仅 ws/wss 监听校验，unix socket 靠文件权限；未配置 = 无鉴权 | — |

---

## `[gc]` — 会话回收

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `retention_days` | integer | 会话保留天数（按 `updated_at`） | `90` |
| `keep_pinned` | boolean | 跳过置顶会话 | `true` |
| `sweep_orphans` | boolean | 清扫孤儿文件 | `true` |
| `vacuum` | boolean | 删除后执行 VACUUM | `false` |
| `auto` | boolean | daemon 启动时 + 每日 0 点自动执行（真实删除） | `false` |

`yomi gc` 命令行参数未指定时回落到此配置。

---

## `[env]` — 环境变量注入

启动时注入进程环境（键名按原样写入，**覆盖**主机同名变量）：

```toml
[env]
KIMI_AGENT_API_KEY = "sk-..."
```

---

## `[tasks]` — 轻量任务

| 字段 | 说明 |
|---|---|
| `fast_model` | 后台轻量任务（如标题生成）用的模型标识名，省略则用会话当前模型 |

---

## `[features]` — 实验特性

| 字段 | 说明 | 默认值 |
|---|---|---|
| `all` | 开启所有未被单项覆盖的 feature | `false` |
| `update_session_title` | 模型自动生成会话标题 | 继承 `all` |
| `cron_tool` | 暴露 cron 工具（定时任务） | 继承 `all` |
| `todo_tool` | 暴露 todo 工具 + 提醒拦截器 | 继承 `all` |
| `attachments` | 教 agent `<yomi_attachments>` 附件语法 | `true`（不受 `all` 影响） |

---

## `[[channels]]` — IM 通道

```toml
[[channels]]
name = "feishu-bot"
enabled = true

[channels.platform]
type = "feishu"
app_id = "..."
app_secret = "..."
```

| 字段 | 说明 | 默认值 |
|---|---|---|
| `name` | 通道名称 | 必填 |
| `enabled` | 是否启用 | 必填 |
| `platform` | 平台配置：`telegram` 需 `token`；`feishu` 需 `app_id` + `app_secret` | 必填 |
| `allowed_chats` / `allowed_users` | 允许名单；名单外 @ 机器人收到 🙏 婉拒，无回复 | `[]` |
| `blocked_chats` / `blocked_users` | 屏蔽名单；完全静默 | `[]` |
| `require_mention` | 群聊需 @ 触发 | `true` |
| `reply_in_thread` | 群聊回复锚定触发消息的 thread（飞书话题 / Telegram 引用） | `false` |
| `auto_approve_level` | 通道级自动批准：`safe`/`caution`/`dangerous` | `safe` |
| `observability` | 状态卡片 + 运行回执 | `true` |
| `tool_trace` | 最终回复附运行轨迹（飞书可折叠面板，其他平台纯文本行） | `true` |
| `history_context` | 触发时注入的最近聊天记录条数（0 关闭） | `20` |
| `admin_users` | 管理员 `open_id` 列表（`/restart`、`/permits` 等命令与审批按钮鉴权） | `[]` |
| `approval_chat_id` | 飞书云文档权限申请的通知群；未配置则私聊 `admin_users` | — |
| `disabled_events` | 运行时停用的平台事件（飞书支持 `doc_comment`） | `[]` |

运行期命令：`/thread <文本>` 一次性开话题回复；`/threads on|off|reset` 按群覆盖 `reply_in_thread`（admin）。

飞书云文档评论触发（评论 @ 机器人即起会话、回复投递为评论回复）等进阶玩法见 `docs/archive/feishu-doc-comment.md`。

---

## 环境变量

`YOMI_` 前缀变量启动时覆盖配置文件对应值。

### 模型

| 变量 | 说明 |
|---|---|
| `YOMI_PROVIDER` | 模型提供商 |
| `YOMI_API_KEY` | 通用 API 密钥 |
| `YOMI_MODEL` | 模型 ID |
| `YOMI_API_BASE` | 自定义 API 地址 |
| `YOMI_MAX_TOKENS` | 最大输出 token |
| `YOMI_TEMPERATURE` | 温度 |
| `YOMI_THINKING` / `YOMI_THINKING_BUDGET` / `YOMI_THINKING_EFFORT` | thinking 开关 / 预算 / effort |

### 应用

| 变量 | 说明 | 默认值 |
|---|---|---|
| `YOMI_CONFIG` | 配置文件路径 | — |
| `YOMI_DATA_DIR` | 数据目录 | `~/.yomi` |
| `YOMI_AUTO_APPROVE` | 自动批准级别 | `safe` |
| `YOMI_MAX_ITERATIONS` | 最大迭代次数 | `100` |
| `YOMI_ENABLE_SUB_AGENTS` | 启用子 agent | `true` |
| `YOMI_CONTEXT_WINDOW` | 上下文窗口 | `131072` |
| `YOMI_COMPACTOR_RATIO` | 压缩阈值比例 | `0.9` |
| `YOMI_MAX_TOOL_OUTPUT_LENGTH` | 最大工具输出（字节） | `40000` |
| `YOMI_MAX_CHECKPOINTS` | 最大检查点数 | `5` |
| `YOMI_TOOL_BLOCKLIST` | 工具禁用列表（逗号分隔正则） | — |
| `YOMI_DEFAULT_MODEL` | 默认模型标识名 | — |
| `YOMI_SKILL_FOLDERS` | 技能目录（逗号分隔） | — |
| `YOMI_STREAM_MAX_RETRIES` | 单轮 streaming 重试上限 | `20` |
| `YOMI_SOCKET` | daemon socket 地址 | — |
| `YOMI_EXTRA_SOCKET` | daemon 额外监听地址（单值，如 `ws://0.0.0.0:57231` 供反代/远端访问，与主 ws 一样按 `socket_auth_hash` 校验） | — |
| `YOMI_SOCKET_AUTH_HASH` | socket 鉴权哈希（覆盖 `socket_auth_hash`） | — |
| `YOMI_SOCKET_AUTH` | 客户端连 ws/wss daemon 的明文 token | — |
| `YOMI_GC_RETENTION_DAYS` / `YOMI_GC_AUTO` | 同 `[gc]` 配置 | — |
| `RUST_LOG` | 日志级别 | — |

### 搜索（无前缀）

| 变量 | 说明 |
|---|---|
| `SEARXNG_URL` | SearXNG 实例地址 |
| `KIMI_AGENT_API_KEY` / `KIMI_SEARCH_ENDPOINT` | Kimi Search 密钥 / 端点覆盖 |
| `SERPER_API_KEY` | Serper.dev 密钥 |
| `BRAVE_API_KEY` | Brave Search 密钥 |

---

## 优先级

`YOMI_XXX` 环境变量 > 配置文件 > 默认值

## `.env` 文件

GUI 启动时自动加载 `~/.env`（Windows 为 `%USERPROFILE%\.env`），修改后重启 GUI 生效。
