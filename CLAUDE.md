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
- **crates/tui/** - Terminal UI components using tuirealm

## Key Patterns

- **Tool Execution**: Tools receive `ToolExecCtx` with cancel token and parent messages for context inheritance
- **Streaming**: Providers return `ModelStream` (Pin<Box<dyn Stream>>) for real-time responses
- **State Machine**: Agent uses explicit state transitions with `AgentState` enum
- **Cancellation**: tokio's `CancellationToken` propagates through agent hierarchy
- **Storage**: SQLite for tasks/messages, filesystem for sessions
- **Plugin Loading**: `PluginLoader` loads `.js` plugins from Claude's plugin cache
- **Unicode Handling**: careful handling of Unicode in TUI
