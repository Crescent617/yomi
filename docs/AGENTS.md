# Yomi Installation Instructions for Agents

Use these instructions when a user asks you to install, build, configure, or troubleshoot Yomi.

## Operating Rules

- Prefer the simplest installation method that meets the user's goal.
- Prefer an official prebuilt release for normal users; build from source for development or when no compatible release exists.
- Detect the user's operating system and architecture before choosing an artifact or system packages.
- Ask for confirmation before installing global dependencies, using `sudo`, changing system configuration, or starting a persistent service.
- Explain commands with security or networking side effects. Do not expose a local service to the public network unless the user explicitly requests it.
- Never commit API keys or write secrets into the repository.
- Do not invent release artifact names, package names, or commands. Inspect the current release and official documentation when necessary.

Repository: <https://github.com/crescent617/yomi>

## Install a Prebuilt Release

For a normal installation:

1. Open <https://github.com/crescent617/yomi/releases>.
2. Select an artifact matching the user's OS and CPU architecture.
3. Follow the platform's normal installation flow.
4. Verify the installation:

   ```bash
   yomi --help
   ```

If the release contains a standalone Unix binary, make it executable and place it on `PATH` only with the user's approval.

## Build the CLI/TUI from Source

Required dependencies:

- Git
- Rust stable, preferably installed with `rustup`
- The platform's C/C++ build toolchain

Verify them first:

```bash
git --version
rustc --version
cargo --version
```

Build Yomi:

```bash
git clone https://github.com/crescent617/yomi.git
cd yomi
cargo build --release -p cli
```

The executable is written to:

```text
target/release/yomi
```

Run it directly:

```bash
./target/release/yomi --help
```

With confirmation, install it into Cargo's binary directory:

```bash
cargo install --path crates/cli --locked
```

Cargo normally installs binaries under `~/.cargo/bin`.

## Build the GUI from Source

The GUI additionally requires:

- Node.js LTS
- npm
- Tauri v2 platform dependencies

Use npm in this repository. Do not replace it with yarn or pnpm.

Official Tauri prerequisites: <https://v2.tauri.app/start/prerequisites/>

### macOS

Install Xcode Command Line Tools if missing:

```bash
xcode-select --install
```

### Debian or Ubuntu

After receiving confirmation:

```bash
sudo apt update
sudo apt install libwebkit2gtk-4.1-dev \
  build-essential \
  curl \
  wget \
  file \
  libxdo-dev \
  libssl-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev
```

For other Linux distributions, consult the official Tauri prerequisites instead of guessing equivalent package names.

### Windows

Install:

- Microsoft C++ Build Tools with **Desktop development with C++**
- Microsoft Edge WebView2 Runtime
- Rust through `rustup`
- Node.js LTS

### Install and Run

```bash
cd crates/gui
npm ci
npm --prefix frontend ci
npm run tauri dev
```

Build distributable packages with:

```bash
npm run tauri build
```

## Configuration

Yomi reads its main configuration from `~/.yomi/config.toml`. See [`config.md`](./config.md) for the full schema.

The GUI also loads environment variables from:

- macOS/Linux: `~/.env`
- Windows: `%USERPROFILE%\.env`

This is useful when the desktop application is launched without a terminal. Restart the GUI after changing the file.

## Web Search

Yomi's `webSearch` tool supports:

| Provider | Required configuration |
| --- | --- |
| Serper | `SERPER_API_KEY` |
| Brave Search | `BRAVE_API_KEY` |
| SearXNG | `SEARXNG_URL` or TOML configuration |

Examples for a shell or `~/.env`:

```dotenv
SERPER_API_KEY=...
# or
BRAVE_API_KEY=...
# or
SEARXNG_URL=http://127.0.0.1:8080
```

Do not add a `YOMI_` prefix to these examples.

A provider can also be selected in `~/.yomi/config.toml`:

```toml
search_provider = "searxng"
search_provider_url = "http://127.0.0.1:8080"
```

Use the SearXNG base URL, not a URL ending in `/search`.

### Recommend SearXNG

Recommend SearXNG when the user wants self-hosted search or does not want a commercial search API key.

Before installing Docker or starting a persistent container, explain what will happen and ask for confirmation. Bind a local development instance to `127.0.0.1` rather than all network interfaces.

Minimal `compose.yaml`:

```yaml
services:
  searxng:
    image: docker.io/searxng/searxng:latest
    container_name: searxng
    ports:
      - "127.0.0.1:8080:8080"
    volumes:
      - ./searxng:/etc/searxng:rw
    restart: unless-stopped
```

Start it with:

```bash
docker compose up -d
```

Official installation documentation: <https://docs.searxng.org/admin/installation-docker.html>

### SearXNG Must Enable JSON Output

Yomi calls SearXNG with `format=json`. SearXNG often enables only HTML by default. Preserve the user's other settings and ensure `search.formats` in `searxng/settings.yml` includes `json`:

```yaml
search:
  formats:
    - html
    - json
```

Restart the container after changing the configuration:

```bash
docker compose restart searxng
```

Test SearXNG independently before troubleshooting Yomi:

```bash
curl --fail --get 'http://127.0.0.1:8080/search' \
  --data-urlencode 'q=yomi' \
  --data-urlencode 'format=json'
```

The response should be JSON containing a `results` array. HTTP `403 Forbidden` commonly means JSON is not enabled in `search.formats`.

Official format setting: <https://docs.searxng.org/admin/settings/settings_search.html#formats>

## Explain Search Usage

After configuration, tell the user they can ask Yomi naturally, for example:

```text
Search the web for the latest Tauri v2 release notes and summarize the breaking changes.
```

For better results:

- Include product names, versions, exact errors, or dates in the query.
- Fetch full pages for research; snippets are not authoritative.
- Prefer primary and official sources for important claims.
- Verify that SearXNG works with `curl` before changing Yomi configuration.

## Troubleshooting Checklist

1. Capture the exact command and full error output.
2. Confirm OS, architecture, installation method, and relevant tool versions.
3. Verify `~/.yomi/config.toml` and required environment variables.
4. For the GUI, verify that variables are in `~/.env` and restart the app.
5. Test external services such as SearXNG independently.
6. Make the smallest evidence-based correction; do not reinstall every dependency by default.
