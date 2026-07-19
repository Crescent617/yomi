# 设计文档：OpenAI Responses API Provider（`openai_response`）

## 背景

当前 kernel 有两个 provider：

- `OpenAIProvider`：对接 OpenAI **Chat Completions API**（`POST {endpoint}/chat/completions`），兼容 Kimi 等 OpenAI 协议的第三方服务。
- `AnthropicProvider`：对接 Anthropic Messages API。

OpenAI 的 **Responses API**（`POST {endpoint}/responses`）是其新一代 API：

1. GPT-5 / o 系列模型的完整 reasoning 能力（`reasoning.effort`、reasoning summary 流式输出）只在 Responses API 上完整支持。
2. 支持 `reasoning.encrypted_content` 回传，多轮工具调用之间可保留思维链（stateless 模式），提升 agentic 场景表现。
3. 请求/响应结构与 Chat Completions 不同：`messages` → `input` items；`choices[].delta` → 语义化 SSE 事件（`response.output_text.delta` 等）；工具调用是独立的 `function_call` item，而非 message 内嵌 `tool_calls`。

由于协议差异大，不适合在现有 `OpenAIProvider` 上打补丁，应新增独立 provider。

---

## 目标

- 新增 `ModelProvider::OpenAIResponse` 变体，配置值为 `openai_response`（遵循项目统一 snake_case；`FromStr` 同时接受 `openai-response` 别名）。
- 新增 `crates/kernel/src/provider/openai_response.rs`，实现 `Provider` trait，输出与现有 provider 完全一致的 `ModelStream`（`ModelStreamItem` 序列）。
- 支持：流式文本、流式 thinking（reasoning summary）、工具调用（含 `ToolCallDelta` 增量）、图片输入、token usage（含 cached tokens）、`ResponseMeta`（response id + finish reason）、idle 超时。
- 复用现有基础设施：全局 `http_client()`、`RetryingProvider`、`ModelConfig`（不加新字段）、`ThinkingConfig`。

### 不做（Out of Scope）

- 不使用服务端会话状态（`previous_response_id` / `store: true`）——kernel 自己管理完整消息历史，请求始终 `store: false`、发送全量 input。
- 不支持 Responses API 的内置工具（`web_search`、`code_interpreter` 等）——kernel 有自己的工具体系。
- 不做 Chat Completions → Responses 的自动迁移/探测，用户显式选择 provider。
- 音频输入暂不支持（现有 openai.rs 也未支持）。

---

## 配置层变更

### 1. `ModelProvider`（`crates/kernel/src/config/mod.rs`）

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]   // lowercase → snake_case，对 openai/anthropic 序列化结果不变
pub enum ModelProvider {
    #[default]
    OpenAI,          // "openai"
    Anthropic,       // "anthropic"
    OpenAIResponse,  // "openai_response"
}
```

- `ModelProvider` 的三个标准环境变量方法（`standard_api_key_env` 等）已移除，仅保留 `YOMI_API_KEY` / `YOMI_MODEL` / `YOMI_API_BASE` 作为环境变量入口。
- `FromStr`：接受 `openai_response` 与 `openai-response`（大小写不敏感）。
- `Display`：输出 `openai_response`。

> 注意：`rename_all = "lowercase"` 会把 `OpenAIResponse` 序列化为 `openairesponse`，因此必须改为 `snake_case` 或对该变体单独 `#[serde(rename = "openai_response")]`。`OpenAI`/`Anthropic` 两个变体在 lowercase 与 snake_case 下结果相同（`openai`/`anthropic`），无兼容性破坏。

### 2. 配置示例（`config.toml`）

```toml
[[models]]
name = "gpt5"
provider = "openai_response"
model_id = "gpt-5"
endpoint = "https://api.openai.com/v1"   # 留空同此默认值
api_key = "sk-..."
context_window = 272000

[models.thinking]
enabled = true
effort = "medium"    # → reasoning.effort
```

### 3. 其他触点

- `docs/config-schema.json`：`provider.enum` 增加 `"openai_response"`。
- `crates/kernel/src/lib.rs` `create_provider_for_model`：
  ```rust
  ModelProvider::OpenAIResponse => Ok(Arc::new(OpenAIResponseProvider::new()?)),
  ```
- `provider/mod.rs`：`pub mod openai_response; pub use openai_response::OpenAIResponseProvider;`

---

## 协议映射设计

### 请求体

```
POST {endpoint 或 https://api.openai.com/v1}/responses
Authorization: Bearer {api_key}
```

```rust
struct ResponsesRequest {
    model: String,
    input: Vec<InputItem>,               // 见下文消息转换
    instructions: Option<String>,        // system 消息合并至此
    tools: Option<Vec<ResponsesTool>>,   // 扁平格式，见下
    stream: bool,                        // 恒为 true
    store: bool,                         // 恒为 false（stateless）
    max_output_tokens: Option<u32>,      // config.max_tokens.or(Some(8192))
    temperature: Option<f32>,            // 注意：reasoning 模型不接受，见「开放问题」
    reasoning: Option<ReasoningParam>,   // thinking.enabled 时 Some
    include: Option<Vec<String>>,        // thinking 时 ["reasoning.encrypted_content"]
}

struct ReasoningParam {
    effort: String,      // thinking.effort，默认 "medium"
    summary: String,     // "auto"，用于流式输出 thinking 内容
}
```

**工具定义是扁平结构**（与 Chat Completions 的 `{type, function: {...}}` 不同）：

```rust
struct ResponsesTool {
    r#type: String,       // "function"
    name: String,
    description: String,
    parameters: Value,
}
```

### 消息转换：`Vec<Arc<Message>>` → `Vec<InputItem>`

| kernel Message | Responses API input item |
|---|---|
| `Role::System` | 全部拼接进顶层 `instructions` 字段（参考 Anthropic 的 `extract_system_message` 做法） |
| `Role::User`，text | `{type:"message", role:"user", content:[{type:"input_text", text}]}` |
| `Role::User`，image | content 加 `{type:"input_image", image_url: url, detail}` |
| `Role::Assistant`，text | `{type:"message", role:"assistant", content:[{type:"output_text", text}]}` |
| `Role::Assistant`，`tool_calls` | 每个 call 单独一个 item：`{type:"function_call", call_id, name, arguments: String}` |
| `Role::Assistant`，`Thinking` block | Phase 1 **不回传**（丢弃）；Phase 2 若 `signature` 中存有 `encrypted_content`，回传 `{type:"reasoning", encrypted_content, summary:[]}` |
| `Role::Tool`（有 `tool_call_id`） | `{type:"function_call_output", call_id: tool_call_id, output: text_content()}` |
| `Role::Internal` | 过滤，不发送 |

关键差异点：

- **工具调用有两个 ID**：item 级 `id`（`fc_...`）与关联用 `call_id`（`call_...`）。kernel 内统一使用 `call_id` 作为 `ToolCallRequest.id`，这样 `Role::Tool` 消息的 `tool_call_id` 天然对得上，其他模块（agent、UI）无需感知。
- `arguments` 在 Responses API 中是 JSON **字符串**，发送时 `c.arguments.to_string()`（与现有 openai.rs 相同）。

### SSE 流处理

Responses API 的 SSE 有语义化 `event:` 类型，且 `data` JSON 内含相同的 `type` 字段。解析策略：**只按 `data.type` 分发**（忽略 event name，容错更好），未知类型静默跳过（`#[serde(other)]`）。无 `[DONE]` 哨兵，以 `response.completed` / 流关闭为终止。

| SSE `data.type` | 产出 `ModelStreamItem` |
|---|---|
| `response.created` | 记录 `response.id`（`resp_...`）→ 存入 assembler |
| `response.output_text.delta` | `Chunk(ContentChunk::Text(delta))` |
| `response.reasoning_summary_text.delta` | `Chunk(ContentChunk::Thinking { thinking: delta, signature: None })` |
| `response.output_item.added`（item.type == `function_call`） | 记录 partial：`output_index → (call_id, name)`；产出空 `ToolCallDelta`（通知 UI 工具开始） |
| `response.function_call_arguments.delta` | 累积 arguments；产出 `ToolCallDelta { id: call_id, name, arguments_delta }` |
| `response.output_item.done`（item.type == `function_call`） | 解析累积 arguments 为 JSON（失败则 `Value::String` 兜底），产出 `ToolCall(ToolCallRequest)` |
| `response.output_item.done`（item.type == `reasoning`，含 `encrypted_content`） | Phase 2：产出带 `signature: Some(encrypted_content)` 的 thinking 块供持久化 |
| `response.completed` | `TokenUsage` + `ResponseMeta` + `Complete`（见下） |
| `response.incomplete` | 同上，finish_reason 取自 `response.incomplete_details.reason` |
| `response.failed` / `error` | `Err(ProviderError::Sse(...))` |
| 其他（`response.in_progress`、`content_part.*`、`*.done` 文本事件等） | 跳过 |

**与 Chat Completions 的重要简化**：工具调用完成有显式的 `output_item.done` 事件，不需要 openai.rs 里"看到更高 index 推断前一个完成"的启发式。assembler 状态机简单得多：

```rust
struct ResponseAssembler {
    partial_calls: HashMap<u32 /* output_index */, PartialCall>, // call_id, name, arguments
    response_id: Option<String>,
    usage: Option<TokenUsage>,
    finish_reason: Option<FinishReason>,
    saw_function_call: bool,
}
```

### finish_reason 归一化

| Responses API | `FinishReason` |
|---|---|
| `status == "completed"` 且本轮有 function_call item | `ToolCalls` |
| `status == "completed"` | `Stop` |
| `incomplete_details.reason == "max_output_tokens"` | `MaxTokens` |
| `incomplete_details.reason == "content_filter"` | `ContentFilter` |
| 其他 | `Unknown`（log warn） |

> Responses API 没有 `tool_calls` 这种 finish_reason，需要 assembler 自己根据是否产出过 `ToolCall` 合成，保证 agent 循环（依赖 `FinishReason::ToolCalls` 判断是否继续执行工具）行为与现有 provider 一致。

### usage 映射

`response.completed` 事件中 `response.usage`：

```
input_tokens                          → TokenUsage.prompt_tokens
output_tokens                         → TokenUsage.completion_tokens
input_tokens_details.cached_tokens    → TokenUsage.cached_tokens
```

### 超时与错误

- 完全复用 openai.rs 的模式：`stream::try_unfold` + 2 分钟 `IDLE_TIMEOUT`（content-stall 检测）+ `eventsource_stream`。
- HTTP 非 2xx → `ProviderError::Http(HttpError(status))`，错误体截断 200 字符打 log；重试语义由外层 `RetryingProvider` 处理，无需改动。
- 自定义 headers（`config.headers`）注入逻辑照搬。

---

## 文件与改动清单

| 文件 | 改动 |
|---|---|
| `crates/kernel/src/provider/openai_response.rs` | **新增**：`OpenAIResponseProvider` + 请求/响应类型 + `ResponseAssembler` |
| `crates/kernel/src/provider/openai_response_test.rs` | **新增**：UT（独立测试文件，项目规范） |
| `crates/kernel/src/provider/mod.rs` | +2 行：`pub mod` + `pub use` |
| `crates/kernel/src/config/mod.rs` | `ModelProvider` 加变体；`rename_all` 改 `snake_case`；`FromStr`/`Display`/三个 env 方法各加一个分支 |
| `crates/kernel/src/lib.rs` | `create_provider_for_model` 加一个 match 分支；re-export |
| `crates/kernel/src/config/config_test.rs` | 补 parse/display 测试 |
| `docs/config-schema.json` | provider enum 加 `"openai_response"` |
| `docs/CONFIG.md` | 补充说明 |

GUI/TUI 无需改动：model picker 读的是 `models` 列表的 `name`，不感知 provider 类型。

## 测试计划

单元测试（`openai_response_test.rs`，参考 `openai_test.rs` 的写法，直接测 assembler 与转换函数，不打真实网络）：

1. **消息转换**：system → instructions；user 文本/图片；assistant 带 tool_calls → `function_call` items；tool 结果 → `function_call_output`；Internal 过滤；Thinking 丢弃。
2. **工具定义转换**：扁平格式正确。
3. **assembler**：
   - 纯文本流（`output_text.delta` × n + `completed`）→ Text chunks + usage + meta + Complete。
   - 单工具调用（`output_item.added` → `arguments.delta` × n → `output_item.done`）→ ToolCallDelta 序列 + 完整 ToolCall，`id == call_id`。
   - 并行多工具调用（交错的 output_index）。
   - reasoning summary delta → Thinking chunk。
   - `incomplete`（max_output_tokens）→ `FinishReason::MaxTokens`。
   - 非法 arguments JSON → `Value::String` 兜底。
   - 未知事件类型跳过不报错。
4. **finish_reason 合成**：有 function_call 时 completed → `ToolCalls`。
5. **config**：`"openai_response"` / `"openai-response"` 解析、序列化 round-trip。

## 实施阶段

- **Phase 1（本次）**：上述全部，thinking 只做流式展示（summary delta → Thinking chunk），不回传 reasoning。
- **Phase 2（可选后续）**：`reasoning.encrypted_content` 持久化到 `ContentBlock::Thinking.signature` 并在后续请求回传，提升多轮工具调用的推理连贯性。

## 开放问题

1. **temperature 与 reasoning 模型冲突**：GPT-5 / o 系列不接受 `temperature`，传了会 400。方案 A：`thinking.enabled` 时不发送 temperature；方案 B：永远透传，由用户配置负责。**倾向 A**（thinking.enabled 时静默丢弃并 log debug）。
2. **配置值命名**：本设计采用 `openai_response`（项目 snake_case 规范），`openai-response` 作为解析别名。如果你强烈偏好 `openai-response` 作为规范写法也可以反过来。
3. **max_output_tokens 默认值**：沿用 openai.rs 的 8192 兜底，reasoning 模型可能因思考消耗需要更大值，是否提高默认（如 16384）？
