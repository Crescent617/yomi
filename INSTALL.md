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

构建 GUI（`cargo build -p yomi-gui` / `tauri build`）需先生成
externalBin sidecar（编译期校验存在性，缺了就报
`resource path binaries/yomi-<triple> doesn't exist`）：

```sh
bash scripts/bundle-cli-sidecar.sh "$(rustc -vV | awk '/^host:/{print $2}')" debug
```

## GUI (`yomi-app`)

v0.10.42 起，GUI 内嵌 CLI sidecar 并在启动时把所在目录 prepend 进
`PATH`——agent 调用内置手册（`yomi doc`）与自管理命令（`yomi
session wait`、`yomi cron`，…）用的是与 GUI 严格同版的 `yomi`，
无需单独安装 CLI。只有当你还想在自己的 terminal 里用 `yomi` 时，
才需要按下文单独装 CLI。

### macOS

```sh
brew update && brew install crescent617/tap/yomi-app
```

The cask links the CLI bundled inside the app into your PATH
（`binary` stanza 指向 `Yomi.app/Contents/MacOS/yomi`）——terminal
里的 `yomi` 与 GUI 严格同版，随 cask 升级。已装 `yomi` formula
会报冲突：只想留 GUI 版就 `brew uninstall yomi`（formula 留给
headless / Linux / 只要 CLI 的场景）。

### Windows

Download the `.msi` installer from the releases page (unsigned —
expect a SmartScreen prompt)。安装包内嵌 CLI sidecar，且 msi 会把
安装目录加进系统 PATH——新开 terminal 即可直接使用 `yomi`（与
GUI 同版）。`.nsis` 安装器不改 PATH；想在 terminal 用可另装 CLI
zip（`yomi-<version>-x86_64-pc-windows-msvc.zip`）。

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
