set shell := ["bash", "-c"]

# Default recipe - show available commands
default:
    @just --list

# Build the project in debug mode
build:
    cargo build

# Build the project in release mode
build-release:
    cargo build --release

# Check code without building (faster)
check:
    cargo check

# Run all tests
test:
    cargo test

# Run tests for a specific crate (e.g., `just test-core`)
test-core:
    cargo test -p yomi-core

test-app:
    cargo test -p yomi-app

test-tui:
    cargo test -p yomi-tui

# Run clippy linting
clippy:
    cargo clippy --all-targets --all-features

# Auto-fix clippy warnings where possible
clippy-fix:
    cargo clippy --fix --allow-dirty

# Format all code
fmt:
    cargo fmt

# Check formatting without modifying files
fmt-check:
    cargo fmt -- --check

# Run the yomi CLI (pass args with `just run -- --directory ./my-project --yolo`)
run *ARGS:
    cargo run --bin yomi -- {{ARGS}}

# Run yomi in YOLO mode for current directory
run-yolo:
    cargo run --bin yomi -- --yolo

# Clean build artifacts
clean:
    cargo clean

# Full CI check - runs check, clippy, test, and fmt-check
ci: check clippy test fmt-check

# Build and run tests in release mode
test-release:
    cargo test --release

# Show dependency tree
tree:
    cargo tree

# Update dependencies
update:
    cargo update

# Run with tracing debug logging
debug *ARGS:
    RUST_LOG=debug cargo run --bin yomi -- {{ARGS}}

# Run with tracing info logging (less verbose)
info *ARGS:
    RUST_LOG=info cargo run --bin yomi -- {{ARGS}}
