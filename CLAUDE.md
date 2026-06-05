# AGENTS.md

## Project Overview

Yomi is a Rust-based AI coding assistant CLI featuring an async agent loop, sub-agent support, and a TUI interface built with tuirealm.

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
```

## Architecture Overview

### Crate Structure

- **crates/kernel/** - Core agent system, tools, providers, and business logic
- **crates/cli/** - Command-line interface and main entry point
- **crates/gui/** - Desktop GUI built with Tauri v2
- **crates/tui/** - Terminal UI components using tuirealm

## Key Patterns

- **Tool Execution**: Tools receive `ToolExecCtx` with cancel token and parent messages for context inheritance
- **Streaming**: Providers return `ModelStream` (Pin<Box<dyn Stream>>) for real-time responses
- **State Machine**: Agent uses explicit state transitions with `AgentState` enum
- **Cancellation**: tokio's `CancellationToken` propagates through agent hierarchy
- **Storage**: SQLite for tasks/messages, filesystem for sessions
- **Plugin Loading**: `PluginLoader` loads `.js` plugins from Claude's plugin cache
- **Unicode Handling**: must carefully handling of unicode width in TUI
- **Env Vars**: should follow prefix `kernel::ENV_PREFIX`

## GUI / Tauri IPC Pitfalls

- **IPC 数据命名约定：前端收 camelCase，后端写 snake_case，serde 桥接。** 所有传给前端的事件和 API 响应都必须序列化为 camelCase，但 Rust 内部字段和参数保持 snake_case，用 `#[serde(rename_all = "camelCase")]` 自动转换。前端 TypeScript 类型必须与 serde 输出完全一致。内核核心数据结构（如 `Message`、`ContentBlock`）若同时用于数据库存储，不可全局加 `rename_all`，应通过 GUI 层 wrapper 类型做转换。
- **Cron scheduler reload after mutations.** When adding/updating/deleting cron jobs through direct `CronStore` calls (bypassing `KernelServer`), explicitly call `scheduler.reload()` so the in-process scheduler re-loads its queue from the database.
