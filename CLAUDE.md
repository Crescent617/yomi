# AGENTS.md

## Project Overview

Yomi is a production-grade AI coding assistant CLI tool written in Rust. It features an async agent loop with sub-agent support, a TUI with custom markdown rendering, and an event-driven architecture.

## Coding Guidelines

### Constitution

- Occam's Razor: Prefer the simplest solution that works
- SOLID: Follow SOLID principles for maintainable code
- DRY: Avoid code duplication; extract common logic into functions or traits
- KISS: Keep it simple; prefer straightforward solutions over clever ones
- YAGNI: Don't implement features until they're needed
- Testable: Write code that's easy to test; prefer pure functions and clear interfaces
- Idiomatic: Follow Rust conventions and best practices for readability and maintainability
- Clean Code: Prioritize readability and clarity; use meaningful and short names
- AGENTS.md: follow the guidelines in AGENTS.md in each module

### Error Handling

- Use `anyhow::Result` for application code, `thiserror` for library errors
- Prefer `?` over `match` for error propagation
- Include context with `.context()` when crossing module boundaries
- Never use `.unwrap()` or `.expect()` in production code

### Async Patterns

- Use `tokio::sync::mpsc` for agent communication
- Use `tokio::sync::watch` for state broadcasting
- Use `tokio::sync::RwLock` for shared state, `Mutex` only when necessary
- Always use `async-trait` for trait methods that need async

### Trait Design

- Define traits in `core`, implement behind feature flags
- Use `#[async_trait]` for async methods
- Prefer `&dyn Trait` for injection, `Arc<dyn Trait>` for shared ownership
- Keep trait methods focused; compose rather than bloat

### State Management

- Agent state changes only through `AgentExecutionContext::transition_to()`
- Use `CancelToken` for cooperative cancellation
- Prefer immutable data structures; clone when crossing async boundaries

### Testing

- Unit tests inline with `#[cfg(test)]`
- Integration tests in `tests/` directory
- Use `tokio::test` for async tests
- Mock traits for unit tests, use real implementations for integration
