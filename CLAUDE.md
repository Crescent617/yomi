# AGENTS.md


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


## Apps
### yomi-gui
using tauri with npm for pkg manager

## Docs

design docs path: ./docs/design


## Key Patterns

- **Tool Execution**: Tools receive `ToolExecCtx` with cancel token and parent messages for context inheritance
- **Streaming**: Providers return `ModelStream` (Pin<Box<dyn Stream>>) for real-time responses
- **State Machine**: Agent uses explicit state transitions with `AgentState` enum
- **Cancellation**: tokio's `CancellationToken` propagates through agent hierarchy
- **Storage**: SQLite for tasks/messages, filesystem for sessions
- **Unicode Handling**: must carefully handling of unicode width in TUI
- **Env Vars**: should follow prefix `kernel::ENV_PREFIX`

## GUI / Tauri IPC Pitfalls

- **统一 `snake_case`。** 全项目（Wire、Tauri IPC、数据库、前端 TypeScript）使用 `snake_case`。Rust 类型用 `#[serde(rename_all = "snake_case")]`；Tauri 命令加 `#[tauri::command(rename_all = "snake_case")]`。前端 TS 接口与 Rust serde 输出直接对齐，不做二次映射。内核核心数据结构（如 `Message`）本身不挂 `rename_all`，默认即 `snake_case`，与数据库一致。仅在需要格式化转换时（如 `GoalStatus` 枚举→字符串），由 GUI wrapper 层处理。
