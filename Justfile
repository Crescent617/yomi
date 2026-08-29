set shell := ["bash", "-c"]

# Default recipe - show available commands
default:
    @just --list

# Run clippy linting
lint:
    cargo clippy --all-features

# Auto-fix clippy warnings where possible
lint-fix:
    cargo clippy --fix --allow-dirty
    cargo fmt
    cd crates/gui/frontend && npm run lint:fix
    cd crates/gui/frontend && npm run format

# Format all code
fmt:
    cargo fmt
    cd crates/gui/frontend && npm run format

# Check formatting without modifying files
fmt-check:
    cargo fmt -- --check
    cd crates/gui/frontend && npm run format:check

# Full CI check - runs check, clippy, test, and fmt-check
ci: check lint test fmt-check

test:
    cargo test

check:
    cargo check
    cargo check -p kernel --no-default-features
    cargo check -p kernel --no-default-features --features feishu
    cargo check -p kernel --no-default-features --features telegram

# Run with tracing debug logging
debug *ARGS:
    RUST_LOG=debug cargo run --bin yomi -- {{ARGS}}

# Run with tracing info logging (less verbose)
info *ARGS:
    RUST_LOG=info cargo run --bin yomi -- {{ARGS}}

# Start GUI dev mode (Tauri + Svelte 5 single process)
gui-dev:
    cd crates/gui/frontend && npm install
    cd crates/gui && npm install
    cd crates/gui && npx tauri dev --no-watch

# Build GUI release bundle and fix DMG signature
gui-build:
    cd crates/gui/frontend && npm install
    cd crates/gui && npm install
    cd crates/gui && npx tauri build
    bash scripts/fix-dmg-signature.sh

tui-build:
    cargo build --release --bin yomi

# Release TUI to homebrew tap (downloads from GitHub release)
brew-release-tui:
    bash ./scripts/brew-release.sh

# Release GUI to homebrew tap (local build + upload + update cask)
brew-release-gui:
    bash ./scripts/brew-cask-release.sh
