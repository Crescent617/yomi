# Install

Operational runbook for installing yomi — written for humans and agents alike.
Every path ends with the same verification step; run it before declaring done.

## CLI (`yomi`)

### Homebrew (macOS / Linux / Windows, recommended)

```sh
brew update && brew install crescent617/tap/yomi
```

### Prebuilt binary (GitHub Releases)

Asset names: `yomi-<version>-<target>.tar.gz` (`.zip` on Windows):

- `aarch64-apple-darwin`
- `x86_64-unknown-linux-gnu`
- `x86_64-pc-windows-msvc`

Download from <https://github.com/crescent617/yomi/releases>, unpack, put `yomi` on `PATH`.

### From source

Requires Rust 1.90+ (install via [rustup](https://rustup.rs)):

```sh
git clone https://github.com/crescent617/yomi.git
cd yomi
cargo build --release --bin yomi   # binary at target/release/yomi
```

## GUI (`yomi-app`)

### macOS

```sh
brew update && brew install crescent617/tap/yomi-app
```

or download the `.dmg` from the releases page.

### Windows

Download the `.msi` / `.nsis` installer from the releases page
(unsigned — expect a SmartScreen prompt).

## First-run setup

1. Write a minimal config at `~/.yomi/config.toml`:

   ```toml
   [[models]]
   name = "default"
   provider = "anthropic"
   model_id = "claude-sonnet-4-5"
   endpoint = "https://api.anthropic.com"
   api_key = "sk-..."

   [agent]
   default_model = "default"
   ```

   Full field reference: [`docs/CONFIG.md`](docs/CONFIG.md).

2. Verify — the first CLI command auto-spawns the daemon:

   ```sh
   yomi doctor                     # all checks green
   yomi run --yolo "reply with: ok"  # smoke test: one real model call
   ```

   `yomi daemon start` exists but is a foreground internal command — the CLI
   spawns the daemon on demand; you never start it by hand in normal use.

## Upgrade

- Homebrew: `brew update && brew upgrade yomi yomi-app`, then `yomi daemon restart`.
- Manual: replace the binary / re-install the `.dmg`, then `yomi daemon restart`.

Config changes (including `yomi config set`) only take effect after a daemon restart.

## Containers

See [`docker/Dockerfile`](docker/Dockerfile). Readiness probe: `yomi rpc hello`
(exit 0 = the daemon's wire handshake succeeded).
