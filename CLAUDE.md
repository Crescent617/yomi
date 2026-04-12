# CLAUDE.md

Yomi - AI coding assistant CLI in Rust. Async agent loop, sub-agents, TUI, event-driven.

## Principles

Simple > Clever. DRY. YAGNI. Clean code with meaningful names (`ctx`, `cfg`, `svc`).

## Rules

| Area | Rule |
|------|------|
| Errors | `anyhow::Result` for app, `thiserror` for libs. No `.unwrap()`. Use `?` + `.context()` |
| Async | `mpsc` for agents, `watch` for state, `RwLock` > `Mutex`. `#[async_trait]` for async traits |
| Traits | Define in `core`. `&dyn Trait` for injection, `Arc<dyn Trait>` for sharing |
| State | Change only via `AgentExecutionContext::transition_to()`. Use `CancelToken` |
| Tests | Inline `#[cfg(test)]`, integration in `tests/`, `tokio::test` for async |
