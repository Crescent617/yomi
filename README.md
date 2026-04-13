# Yomi

[![Rust](https://img.shields.io/badge/Rust-1.75+-orange.svg)](https://www.rust-lang.org)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)

A powerful AI coding assistant CLI built in Rust, featuring an async agent loop, sub-agent support, and an elegant TUI interface.

![demo](docs/demo.png)

## ✨ Features

### 🤖 Intelligent Agent System
- **Async Agent Loop** - Event-driven architecture for efficient task processing
- **State Machine** - Robust state management with proper transitions
- **Cancel Token** - Graceful cancellation support for long-running tasks
- **Context Management** - Rich execution context for agents

### 🔄 Sub-Agent Support
- **Parallel Execution** - Spawn multiple sub-agents to work concurrently
- **Sync & Async Modes** - Choose between waiting for results or fire-and-forget
- **Task Delegation** - Break complex tasks into smaller, manageable pieces
- **Result Aggregation** - Collect and merge results from multiple agents

### 🖥️ Beautiful TUI
- **Interactive Interface** - Built with `tuirealm` for smooth navigation
- **Real-time Updates** - Watch task progress live
- **Markdown Rendering** - Beautiful display of AI responses
- **Keyboard Shortcuts** - Efficient workflow with vim-like bindings

### 📦 Modular Architecture
- **Kernel** - Core agent system, task management, and storage
- **CLI** - Command-line interface with comprehensive commands
- **TUI** - Terminal user interface for interactive sessions

### 💾 Persistent Storage
- **SQLite Backend** - Reliable task and session persistence
- **Session Management** - Resume previous sessions anytime
- **Message History** - Full conversation context preservation

### 🔧 Multi-Provider Support
- **OpenAI** - GPT-4, GPT-4o, GPT-3.5-turbo support
- **Anthropic** - Claude 3.5 Sonnet and family
- **Extensible** - Easy to add new providers

### 🛡️ Safety & Control
- **YOLO Mode** - Optional flag to skip all confirmations (`--yolo`)
- **Confirmation Prompts** - Review before executing shell commands
- **Read-Only by Default** - Safe exploration of codebases
- **Git Integration** - Respects your version control workflow

## 🚀 Quick Start

### Prerequisites

- Rust 1.75+ (install via [rustup](https://rustup.rs))
- OpenAI API key (Anthropic not yet supported)


### Configuration

```bash
# Easiest way
export OPENAI_API_KEY=sk-...
export OPENAI_API_BASE=http...
export OPENAI_API_MODEL=gpt-4o
```

### Usage

#### Interactive TUI Mode

```bash
# Launch TUI in current directory
yomi

# Specify working directory
yomi -d ./my-project

# Resume last session for this directory
yomi -r
```

```

#### YOLO Mode ⚡

Skip all confirmations (use with caution!):

```bash
yomi --yolo
```

## 🏗️ Architecture

```
yomi/
├── crates/
│   ├── kernel/         # Core agent system
│   │   ├── agent/      # Agent implementation
│   │   ├── task/       # Task management
│   │   ├── storage/    # SQLite storage layer
│   │   ├── provider/   # LLM provider abstractions
│   │   └── skill/      # Skill/ability system
│   ├── cli/            # Command-line interface
│   └── tui/            # Terminal UI
├── skills/             # Built-in skills
└── docs/               # Documentation
```

### Core Concepts

1. **Agent** - The main execution unit that processes tasks
2. **Task** - A unit of work with lifecycle management
3. **Coordinator** - Orchestrates agents and manages resources
4. **Provider** - Abstraction over different LLM APIs
5. **Skill** - Reusable capabilities (file operations, bash, etc.)

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- Built with [Tokio](https://tokio.rs) async runtime
- TUI powered by [tuirealm](https://github.com/veeso/tuirealm)
- Inspired by [Claude Code](https://claude.ai/code) and similar tools
