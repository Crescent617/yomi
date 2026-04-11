---
name: rust-error-handling
description: Guidelines for proper Rust error handling
triggers:
  - error handling
  - anyhow
  - thiserror
---

# Rust Error Handling Guidelines

When handling errors in Rust:

1. Use `anyhow::Result` for application code
2. Use `thiserror` for library errors with custom error types
3. Prefer `?` operator over explicit match for error propagation
4. Add context with `.context()` when crossing module boundaries
5. Never use `.unwrap()` or `.expect()` in production code
