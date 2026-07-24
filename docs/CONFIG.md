# Config

支持 TOML 配置文件和环境变量两种配置方式。默认从 `~/.yomi/config.toml` 读取，或通过 `--config` 指定路径。

## Schema 验证

配置文件支持 JSON Schema 验证，可在 `config.toml` 文件顶部添加 schema 指令：

```toml
#:schema https://raw.githubusercontent.com/Crescent617/yomi/main/docs/config-schema.json
```

VS Code 用户可安装 [Even Better TOML](https://marketplace.visualstudio.com/items?itemName=tamasfe.even-better-toml) 扩展以获得自动补全和校验支持。

## 完整配置示例

```toml
#:schema https://raw.githubusercontent.com/Crescent617/yomi/main/docs/config-schema.json

auto_approve = "safe"
data_dir = "~/.yomi"
skill_folders = ["~/.yomi/skills"]
max_checkpoints = 5

[[models]]
name = "default"
provider = "anthropic"
model_id = "claude-3-5-sonnet-20241022"
endpoint = "https://api.anthropic.com"
api_key = "sk-..."
max_tokens = 4096
temperature = 0.7

[models.thinking]
enabled = true
budget_tokens = 16000

[agent]
default_model = "default"
max_iterations = 100
enable_subagent = true

[agent.compactor]
micro_compact_enabled = false
threshold_ratio = 0.8

[env]
KIMI_AGENT_API_KEY = "sk-..."
SERPER_API_KEY = "..."

[tasks]
fast_model = "default"
```

---

### `[[models]]` — 多模型配置

至少配置一个模型。运行时通过 `default_model` 或会话切换选择。

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `name` | string | 模型标识名，如 `default`、`gpt4o` | `"default"` |
| `provider` | string | `openai` / `anthropic` / `openai_response` | `"openai"` |
| `model_id` | string | 实际 API 模型 ID | 必填 |
| `endpoint` | string | API 基础地址 | 必填 |
| `api_key` | string | API 密钥 | 必填 |
| `max_tokens` | integer | 单次最大输出 token | — |
| `temperature` | float | 温度 (0.0–2.0) | — |
| `fallback_model_id` | string | 降级模型 ID | — |
| `sse_timeout_secs` | integer | SSE 流超时（秒） | `30` |
| `context_window` | integer | 上下文窗口大小 | `131072` |
| `headers` | object | 额外 HTTP 请求头 | `{}` |

**Provider 说明：**

| provider | 说明 |
|---|---|
| `openai` | Chat Completions API (`/chat/completions`)，兼容 Kimi 等 OpenAI 协议服务 |
| `openai_response` | Responses API (`/responses`)，适用于 GPT-5 / o 系列推理模型（也接受 `openai-response`） |
| `anthropic` | Messages API (`/messages`) |

`openai_response` 示例：

```toml
[[models]]
name = "gpt5"
provider = "openai_response"
model_id = "gpt-5"
endpoint = "https://api.openai.com/v1"
api_key = "sk-..."
context_window = 272000

[models.thinking]
enabled = true
effort = "medium"  # low | medium | high
```

> 注意：`thinking.enabled = true` 时不会发送 `temperature`（推理模型不支持）。

**Thinking 配置（按模型）：**

| 字段 | 说明 | 适用模型 |
|---|---|---|
| `enabled` | 启用 thinking | Claude / o1 / o3 |
| `budget_tokens` | Thinking token 预算 | Claude 系列 |
| `effort` | Reasoning effort：`low`/`medium`/`high` | o1 / o3 系列 |

---

### `[agent]` — Agent 行为

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `default_model` | string | 默认模型标识名 | `"default"` |
| `max_iterations` | integer | 单次会话最大迭代次数 | `100` |
| `enable_subagent` | boolean | 允许 spawn 子 agent | `true` |
| `system_prompt` | string | 自定义系统提示，省略则用内置默认 | 内置 |
| `tool_blocklist` | string[] | 工具禁用列表（正则） | `[]` |
| `max_tool_output_length` | integer | 最大工具输出长度（字节） | `40000` |

**`[agent.compactor]` — 上下文压缩：**

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `micro_compact_enabled` | boolean | 是否在全量压缩前尝试微压缩；启用后会改写旧工具结果，影响 prompt cache | `false` |
| `threshold_ratio` | float | 触发压缩的上下文比例 (0.0–1.0) | `0.8` |
| `keep_recent_messages` | integer | 全量压缩时保留的最近消息数 | `0` |
| `keep_recent_tool_results` | integer | 微压缩时保留的最近工具结果数 | `5` |
| `summary_max_tokens` | integer | 全量压缩摘要的 token 上限 | `8192` |

---

### 顶层字段

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `auto_approve` | string | 自动批准级别：`safe` / `caution` / `dangerous` | `"safe"` |
| `data_dir` | string | 数据目录，支持 `~` 展开 | `"~/.yomi"` |
| `log_dir` | string | 日志目录，省略则 `<data_dir>/logs` | — |
| `skill_folders` | string[] | 技能文件夹路径 | 标准路径 |
| `max_checkpoints` | integer | 每会话保留的最大检查点数量 | `5` |

---

### `[env]` — 通用环境变量注入

在应用启动时注入到进程环境。键名**不需要** `YOMI_` 前缀，按原样写入。仅当主机环境变量不存在时注入。

```toml
[env]
KIMI_AGENT_API_KEY = "sk-..."
SERPER_API_KEY = "..."
```

---

### `[tasks]` — 轻量任务配置

用于自动标题生成等后台轻量任务。

| 字段 | 类型 | 说明 |
|---|---|---|
| `fast_model` | string | 轻量任务模型标识名，省略则使用会话当前模型 |

---

### `[features]` — 实验特性

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `all` | boolean | 开启所有未被单项显式覆盖的 feature | `false` |
| `update_session_title` | boolean | 使用模型自动生成会话标题（首条消息 fallback 标题不受影响） | 继承 `all` |

---

### `[[channels]]` — 外部平台集成

配置 Telegram、飞书等外部通道，使 Agent 可通过消息平台接收和发送消息。

```toml
[[channels]]
name = "telegram-bot"
enabled = true

[channels.platform]
type = "telegram"
token = "..."

[[channels]]
name = "feishu-bot"
enabled = true

[channels.platform]
type = "feishu"
app_id = "..."
app_secret = "..."
```

| 字段 | 类型 | 说明 | 默认值 |
|---|---|---|---|
| `name` | string | 通道名称 | 必填 |
| `enabled` | boolean | 是否启用 | `false` |
| `platform` | object | 平台配置 | 必填 |
| `allowed_chats` | string[] | 允许的聊天 ID | `[]` |
| `allowed_users` | string[] | 允许的用户 ID | `[]` |
| `blocked_chats` | string[] | 屏蔽的聊天 ID | `[]` |
| `blocked_users` | string[] | 屏蔽的用户 ID | `[]` |
| `require_mention` | boolean | 是否需要 @ 触发 | `true` |
| `reply_in_thread` | boolean | 群聊中回复是否锚定到触发消息的 thread（Feishu 话题回复，Telegram 引用回复）；私聊不受影响 | `false` |
| `auto_approve_level` | string | 通道级别自动批准：`safe`/`caution`/`dangerous` | `safe` |
| `observability` | boolean | 运行可观测性（状态卡片 + 运行回执记录）；关闭后退回"收到确认 + 最终回复"的旧行为 | `true` |
| `tool_trace` | boolean | 最终回复气泡附带运行轨迹（工具调用 + 中间过程文本）：卡片平台（Feishu，需客户端 V7.9+）以可折叠面板呈现，其他平台以纯文本行附在正文后；关闭后最终回复仅为纯文本 | `true` |

**Platform 配置：**

| 类型 | 必需字段 |
|---|---|
| `telegram` | `token` |
| `feishu` | `app_id`, `app_secret` |

---

## 环境变量

所有 `YOMI_` 前缀变量在应用启动时覆盖配置文件中的对应值。此外支持部分无前缀的 provider 特定变量和搜索变量。

### 模型配置

| 变量 | 说明 | 示例 |
|------|------|------|
| `YOMI_PROVIDER` | 模型提供商 | `openai` / `anthropic` |
| `YOMI_API_KEY` | 通用 API 密钥 | `sk-...` |
| `YOMI_MODEL` | 模型 ID | `claude-3-5-sonnet` |
| `YOMI_API_BASE` | 自定义 API 地址 | `https://...` |
| `YOMI_MAX_TOKENS` | 最大输出 token | `4096` |
| `YOMI_TEMPERATURE` | 温度 | `0.7` |

### 应用配置

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `YOMI_DATA_DIR` | 数据目录 | `~/.yomi` |
| `YOMI_AUTO_APPROVE` | 自动批准级别 | `safe` |
| `YOMI_MAX_ITERATIONS` | 最大迭代次数 | `100` |
| `YOMI_ENABLE_SUB_AGENTS` | 启用子 agent | `true` |
| `YOMI_CONTEXT_WINDOW` | 上下文窗口 | `128k` |
| `YOMI_COMPACTOR_RATIO` | 压缩阈值比例 | `0.8` |
| `YOMI_MAX_TOOL_OUTPUT_LENGTH` | 最大工具输出（字节） | `40000` |
| `YOMI_MAX_CHECKPOINTS` | 最大检查点数 | `5` |
| `YOMI_TOOL_BLOCKLIST` | 工具禁用列表（逗号分隔正则） | — |
| `YOMI_LOG_DIR` | 日志目录 | — |
| `YOMI_DEFAULT_MODEL` | 默认模型标识名 | — |
| `YOMI_SKILL_FOLDERS` | 技能文件夹（逗号分隔） | — |

### 搜索

搜索环境变量**不需要** `YOMI_` 前缀。

| 变量 | 说明 | 示例 |
|------|------|------|
| `SEARXNG_URL` | SearXNG 实例地址 | `http://127.0.0.1:8080` |
| `KIMI_AGENT_API_KEY` | Kimi Search API 密钥 | `sk-...` |
| `KIMI_SEARCH_ENDPOINT` | Kimi Search 端点覆盖（可选） | `https://...` |
| `SERPER_API_KEY` | Serper.dev API 密钥 | `sk-...` |
| `BRAVE_API_KEY` | Brave Search API 密钥 | `...` |

### 日志

| 变量 | 说明 |
|------|------|
| `RUST_LOG` | 日志级别：`error`/`warn`/`info`/`debug`/`trace` |
| `YOMI_LOG_DIR` | 日志目录 |

### Thinking / Reasoning

| 变量 | 说明 | 适用模型 |
|------|------|----------|
| `YOMI_THINKING` | 启用 thinking | Claude / o1 / o3 |
| `YOMI_THINKING_BUDGET` | Thinking token 预算 | Claude 系列 |
| `YOMI_THINKING_EFFORT` | Reasoning effort：`low`/`medium`/`high` | o1 / o3 系列 |

---

## 优先级

`YOMI_XXX` > 配置文件 > 默认值

---

## `.env` 文件

GUI 启动时会自动加载 `~/.env`（Windows 为 `%USERPROFILE%\.env`），方便桌面端配置环境变量。

修改后重启 GUI 生效。
