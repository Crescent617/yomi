set shell := ["bash", "-c"]

# Default recipe - show available commands
default:
    @just --list

# Run clippy linting
lint:
    cargo clippy --all-targets --all-features

# Auto-fix clippy warnings where possible
lint-fix:
    cargo clippy --fix --allow-dirty

# Format all code
fmt:
    cargo fmt

# Check formatting without modifying files
fmt-check:
    cargo fmt -- --check

# Full CI check - runs check, clippy, test, and fmt-check
ci: check lint test fmt-check

test:
    cargo test

check:
    cargo check

# Run with tracing debug logging
debug *ARGS:
    RUST_LOG=debug cargo run --bin yomi -- {{ARGS}}

# Run with tracing info logging (less verbose)
info *ARGS:
    RUST_LOG=info cargo run --bin yomi -- {{ARGS}}

build-release:
    cargo build --release

brew-release:
    bash ./scripts/brew-release.sh
