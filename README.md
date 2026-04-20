# Yomi

[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A simple AI coding assistant CLI built in Rust, featuring an async agent loop, sub-agent support, and an elegant TUI interface.

![demo](docs/demo.png)

## Features

### Intelligent Agent System
- **Async Agent Loop** - Event-driven architecture for efficient task processing
- **State Machine** - Robust state management with proper transitions (Idle → Streaming → ExecutingTool → WaitingForInput)
- **Cancel Token** - Graceful cancellation support for long-running tasks with cascading cancellation to sub-agents
- **Context Management** - Rich execution context with message history and tool registry

## Quick Start

### Prerequisites

- Rust 1.75+ (install via [rustup](https://rustup.rs))
- API key from OpenAI or Anthropic

### Installation

```bash
brew update && brew install crescent617/tap/yomi
```

### Configuration

更多配置选项请参见 [config.md](docs/config.md)。

```bash
# OpenAI
export OPENAI_API_KEY=sk-...
export OPENAI_API_MODEL=gpt-4o  # optional, defaults to gpt-4o
export OPENAI_API_BASE=https://xxx
export YOMI_CONTEXT_WINDOW=200k
```

### Usage

#### Interactive TUI Mode

```bash
# Launch TUI in current directory
yomi

# help
yomi -h
```

#### YOLO Mode

Skip all confirmations (use with caution):

```bash
yomi --yolo
yomi -y
```

## Safety

- **Read-Only by Default** - Tools are categorized by safety level
- **Git-Aware** - Respects .gitignore in Glob/Grep operations
- **File State Tracking** - Write/Edit tools require reading files first to prevent conflicts
- **Cancellation Support** - All long-running operations can be cancelled

## License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## Acknowledgments

- Built with [Tokio](https://tokio.rs) async runtime
- TUI powered by [tuirealm](https://github.com/veeso/tuirealm)
- File operations use [ignore](https://crates.io/crates/ignore) crate for git-aware walking
- Inspired by [Claude Code](https://claude.ai/code) and similar AI coding assistants
