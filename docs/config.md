# Config

支持 TOML 配置文件和环境变量两种配置方式。

## 配置文件

默认从 `~/.yomi/config.toml` 读取，或通过 `--config` 指定路径。

```toml
provider = "anthropic"

[model]
api_key = "sk-..."
model_id = "claude-3-5-sonnet-20241022"
endpoint = "https://api.anthropic.com"
max_tokens = 4096
temperature = 0.7

[model.thinking]
enabled = true
budget_tokens = 16000

[agent]
max_iterations = 100
enable_subagent = true

[agent.compactor]
context_window = 128000

sandbox = false
yolo = false
auto_approve = "safe"  # safe | caution | dangerous
data_dir = "~/.yomi"
skill_folders = ["~/.yomi/skills"]
plugin_dirs = ["~/.claude/plugins"]
load_claude_plugins = true
```

## 环境变量

支持 `YOMI_` 前缀的通用变量和 provider 特定变量。

### 核心配置

| 变量 | 说明 | 示例 |
|------|------|------|
| `YOMI_PROVIDER` | 模型提供商 | `openai` / `anthropic` |
| `YOMI_API_KEY` | API 密钥（通用） | `sk-...` |
| `YOMI_MODEL` | 模型 ID | `claude-3-5-sonnet` |
| `YOMI_API_BASE` | 自定义 API 地址 | `https://...` |
| `YOMI_MAX_TOKENS` | 最大 token 数 | `4096` |
| `YOMI_TEMPERATURE` | 温度参数 | `0.7` |

### Provider 特定变量

作为 `YOMI_XXX` 的 fallback：

| OpenAI | Anthropic |
|--------|-----------|
| `OPENAI_API_KEY` | `ANTHROPIC_API_KEY` |
| `OPENAI_API_MODEL` | `ANTHROPIC_MODEL` |
| `OPENAI_API_BASE` | `ANTHROPIC_BASE_URL` |

### 应用配置

| 变量 | 说明 | 默认值 |
|------|------|--------|
| `YOMI_DATA_DIR` | 数据目录 | `~/.yomi` |
| `YOMI_SANDBOX` | 沙箱模式 | `false` |
| `YOMI_YOLO` | 自动批准所有操作 | `false` |
| `YOMI_AUTO_APPROVE` | 自动批准级别 | `safe` |
| `YOMI_MAX_ITERATIONS` | 最大迭代次数 | `100` |
| `YOMI_ENABLE_SUB_AGENTS` | 启用子 agent | `true` |
| `YOMI_CONTEXT_WINDOW` | 上下文窗口 (支持 `128k`/`1m`) | `128k` |

### 技能与插件

| 变量 | 说明 | 格式 |
|------|------|------|
| `YOMI_SKILL_FOLDERS` | 技能文件夹路径 | 逗号分隔 |
| `YOMI_PLUGIN_DIRS` | 插件目录 | 冒号分隔 |
| `YOMI_LOAD_CLAUDE_PLUGINS` | 加载 Claude 插件 | `true`/`false` |

### 日志

| 变量 | 说明 |
|------|------|
| `RUST_LOG` | 日志级别: `error`/`warn`/`info`/`debug`/`trace` |
| `YOMI_LOG_DIR` | 日志目录 |

### Thinking / Reasoning

| 变量 | 说明 | 适用模型 |
|------|------|----------|
| `YOMI_THINKING` | 启用 thinking | Claude / o1 / o3 |
| `YOMI_THINKING_BUDGET` | Thinking token 预算 | Claude 系列 |
| `YOMI_THINKING_EFFORT` | Reasoning effort: `low`/`medium`/`high` | o1 / o3 系列 |

配置示例：
```toml
[model.thinking]
enabled = true
budget_tokens = 16000      # Claude
effort = "medium"          # OpenAI o1/o3
```

## 优先级

`YOMI_XXX` > Provider 特定变量 > 配置文件 > 默认值
