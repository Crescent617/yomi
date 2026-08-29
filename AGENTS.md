# AGENTS.md

## Design Philosophy

> **The stream is reality.**

1. **Everything is stream** — in as messages, out as events, borne by
   sessions; nothing lives off-stream.
2. **State is cache** — discarded at will, restored at a fold.
3. **Model is suspect** — bounded by design, not by hope.

### Extension: four ports, no fifth

| Port | Chokepoint | Examples (in-proc / out-of-proc) |
|---|---|---|
| Source (messages in) | input bus | cron, channels / webhook bridge, RPC clients |
| Sink (events out) | event bus | obs cards, persistence / events followers |
| Gate (veto) | conductor | permission checker, interceptors / hook scripts |
| Capability (tools) | ToolRegistry | built-in tools / MCP servers, skills |

In-proc and out-of-proc share one contract (the wire protocol is the bus,
projected over a socket). Default to out-of-proc; promote only when usage
proves out. Extension state lives in sqlite/config — never private.

## Build Commands

```bash
# Build the project
cargo build

# Build release
cargo build --release

# Run linting
cargo clippy --all-targets --all-features

# Auto-fix clippy warnings
cargo clippy --fix --allow-dirty

# Format code
cargo fmt

# Check formatting
cargo fmt -- --check

# Harness 回归冒烟（改 prompt 装配/工具 desc/内置模板/conductor/cron 后跑）
evals/harness-e2e.sh

# 通道类改动（slash 命令、卡片、回复行为）：另需真链路验证（测试账号，见 .agents/skills/yomi-e2e）

# running before commit
just ci
```

## Architecture Overview

### Crate Structure

- **crates/kernel/** - Core agent system, tools, providers, and business logic
- **crates/cli/** - Command-line interface and main entry point
- **crates/gui/** - Desktop GUI built with Tauri v2
- **crates/tui/** - Terminal UI components using tuirealm

## Rules
### Protocol
- **统一 `snake_case`。** 全项目（Wire、Tauri IPC、数据库、前端 TypeScript）使用 `snake_case`。Rust 类型用 `#[serde(rename_all = "snake_case")]`；Tauri 命令加 `#[tauri::command(rename_all = "snake_case")]`。前端 TS 接口与 Rust serde 输出直接对齐，不做二次映射。内核核心数据结构（如 `Message`）本身不挂 `rename_all`，默认即 `snake_case`，与数据库一致。仅在需要格式化转换时（如审批级别枚举→显示字符串），由 GUI wrapper 层处理

### Rust
- write ut in separate test file.
    - e.g. a.rs with a_test.rs. use `#[cfg(test)]`

### kernel
- **Env Vars**: should follow prefix `kernel::ENV_PREFIX`
- **Channel features**: 平台适配器按同名 cargo feature 裁剪（`feishu` / `telegram`，默认 `all-channels` 全开）；`PlatformConfig` 等 serde 类型不随 feature 门控，编译外平台在 `build_adapter` 报 Config 错误。内建 `web_search` 工具同理（feature `websearch`，默认开；`WEBSEARCH_TOOL_NAME` 不门控，权限解析对扩展提供的同名工具仍生效）

### gui
- using tauri with npm as pkg manager
- **Design**: Follow [`crates/gui/DESIGN.md`](./crates/gui/DESIGN.md) for GUI visual language and interaction principles.
- **Colors**: 禁止硬编码 Tailwind 颜色值（如 `text-red-500`）和 `dark:` 前缀。一律使用语义化颜色，由 `app.css` 的 `@theme` 变量统一提供。可用语义色包括 `primary`, `secondary`, `destructive`, `success`, `warning`, `error`, `info`, `overlay`, `subtle`, `code-bg` 等。示例：`text-error`（light/dark 自动适配）、`bg-success/10`、`border-subtle`。

### tui
- **Unicode Handling**: must carefully handling of unicode width in TUI

## Docs

- Design: ./docs/design
