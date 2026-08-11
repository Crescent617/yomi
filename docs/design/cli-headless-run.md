# 设计文档：`yomi run` — Headless 一次性运行

## 目标

CLI 目前没有真正的 headless run：跑一次 prompt、拿到结果、按退出码判断成败，方便脚本/CI 使用。现有 `session send` + `events` 组合能近似实现，但需要 daemon、手动关联 session、自己判断结束时机。

`yomi run` 把「建/恢复会话 → 发问 → 等结束 → 格式化结果 → 退出码」打包成一次调用。**纯 CLI 层组合现有 `KernelApi`，不新增 wire 方法，wire 协议版本不变。**

## 命令形态

```bash
yomi run [PROMPT...] [OPTIONS]

yomi run "总结这个仓库的架构"
git diff | yomi run "review 这些改动"        # prompt + stdin 时 stdin 包在 ``` 块里
```

| Flag | 说明 |
|---|---|
| `-m, --model <MODEL_KEY>` | 指定模型（写入 session 的 `model_key`，同 TUI `--model`） |
| `-r, --resume [<SESSION_ID>]` | 继续上次/指定会话（复用 `SessionArg` 三态语义） |
| `-f, --fork [<SESSION_ID>]` | fork 会话后运行 |
| `--format <text\|json\|stream-json>` | 输出格式，默认 `text` |
| `-y, --yolo` | 全部自动批准（`Level::Dangerous`） |
| `--auto-approve <safe\|caution\|dangerous>` | 覆盖配置的批准阈值 |
| `--timeout <SECONDS>` | 墙钟超时，到期 cancel 并 exit 124 |
| `--bg` / `--fg` | 全局三态：强制 daemon（不存在则 spawn，hello 严格校验）/ 强制本地前台 |
| `--ephemeral` | 不把 session 记为当前目录的 last session |
| `-v, --verbose` | 工具调用/重试进度打到 stderr（stdout 只放结果） |

## Kernel 选择（全局三态，`run`/`tui` 共享）

- **默认 auto**：探测到活 daemon 且 hello 握手（含协议版本）通过就用 daemon；没有 daemon 在跑则本地 in-process kernel。**auto 模式不 spawn daemon**（CI 场景不留后台进程）；daemon 活着但 hello 失败是硬错误，**绝不静默 fallback 本地**。
- `--bg`：强制后台 daemon，不存在则 spawn，连接必须通过 hello 校验，否则报错。
- `--fg`：强制本地前台 in-process，完全无视 daemon。

## 执行流程

1. 建 kernel（按上述规则）
2. `resolve_session`（New / Resume / Fork，`CreateSessionInput { model_key, auto_approve_level, working_dir }`）
3. **先** `subscribe_session_events` **再** `send_message`（避免快事件竞态丢失）
4. 事件循环直到 `AgentEvent::Lifecycle { Stopped { reason } }`：
   - `ModelEvent::End` → 收集最终 assistant 文本
   - `ModelEvent::TokenUsage` → 累加 token
   - **`PermissionRequest` → 非 yolo 时客户端立即回 `approved=false`**（现有请求无人应答会挂 2 分钟才超时拒绝，headless 下每个被拒工具白等 2 分钟；客户端即时拒绝即可，无需改 kernel）
   - `AskUserQuestion` → 立即回空 answers（同理）
   - `Error { is_recoverable: false }` → 记录错误
5. Ctrl-C → 先 `cancel()` 再退出（exit 130）
6. 默认 `app_storage.save_session(working_dir, session_id)`（之后可 `yomi --resume` 继续）；`--ephemeral` 跳过

## 退出码

| Code | 含义 |
|---|---|
| 0 | `StopReason::Completed` |
| 1 | 启动失败（连不上、session 不存在、prompt 为空且 stdin 是 TTY） |
| 2 | `StopReason::Failed` |
| 3 | `StopReason::MaxIterations` |
| 124 | `--timeout` 到期（对齐 GNU timeout） |
| 130 | Ctrl-C |

## 输出格式

**`text`（默认）**：stdout 只输出最终 assistant 文本，方便 `$(yomi run ...)` 捕获；诊断走 stderr。

**`json`**：运行结束后输出单个对象：

```json
{
  "session_id": "01K...",
  "status": "completed",
  "result": "最终 assistant 文本",
  "model": "claude-sonnet-4",
  "num_turns": 3,
  "duration_ms": 12345,
  "usage": { "prompt_tokens": 1200, "completion_tokens": 350, "total_tokens": 1550 },
  "error": null
}
```

`status`: `completed | failed | max_iterations | cancelled | timeout`，与退出码对应。

**`stream-json`**：NDJSON，复用 `Envelope` serde 输出（与 `yomi events` 一致，可接 `jq`），末尾追加一行 `{"type":"result", ...}`。

## 实现要点

- 新文件 `crates/cli/src/commands/run.rs` + `run_test.rs`（AGENTS.md 测试惯例）
- 复用：`TuiArgs::build_initial_message`（prompt+stdin 合并）、`resolve_session`/`SessionArg`、`daemon::try_connect`/`spawn_daemon`、`send_with_retry`、`utils::{resolve_working_dir, load_config}`
- 单测覆盖：消息解析、输出组装、退出码映射
