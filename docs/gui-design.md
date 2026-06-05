# Yomi GUI Design Document

## Overview

A cross-platform desktop/mobile GUI for Yomi built on **Tauri v2**, **Svelte 5**, and **TailwindCSS v4**. The GUI connects to the existing Yomi kernel daemon via the native Wire Protocol (Unix Socket / TCP), reusing the same RPC interface that the TUI currently uses.

**Goals**
- **Zero changes to kernel daemon**: The existing `KernelServer` and Wire Protocol V2 are used as-is.
- **Responsive by default**: Single codebase serves desktop (sidebar layout) and mobile (bottom nav + sheets).
- **Session-centric UX**: Easy session switching, forking, history browsing.
- **Skill management UI**: Visual toggling, reload, and discovery of local skills.

---

## Tech Stack

| Layer | Technology | Version / Notes |
|---|---|---|
| Desktop/Mobile Shell | Tauri | v2 (mobile support: iOS/Android) |
| Frontend Framework | Svelte | v5 (runes syntax) |
| Styling | TailwindCSS | v4 (CSS-first configuration) |
| Component Primitives | shadcn-svelte | Latest (v4+), installed via `npx shadcn-svelte@latest init` |
| Component Icons | Lucide Svelte | Consistent iconography |
| State | Svelte Runes | No external state library needed |
| Code Editor | CodeMirror 6 | Lightweight embedded editor with theming, line numbers, vim keymap support |
| Terminal | xterm.js v5 + @xterm/addon-fit | ANSI-compatible terminal emulator |
| PTY Backend | `portable-pty` (Rust) | Cross-platform pseudo-terminal for real shell sessions |
| Markdown | Markdown-it + custom renderer | Safe HTML, plugin-based |
| Build | Vite | Bundler via `@sveltejs/vite-plugin-svelte` |
| Rust Async | Tokio | Reuse kernel crate transports |

---

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│  GUI (Tauri)                                            │
│  ┌─────────────────────────────────────────────────┐  │
│  │  Frontend (Svelte 5 + Tailwind)                  │  │
│  │  - Pages: Chat, Sessions, Skills, Settings        │  │
│  │  - Shared: Layout, Toast, Modal, StreamRenderer │  │
│  └────────────────┬────────────────────────────────┘  │
│                   │ IPC (Tauri Commands + Events)   │
│  ┌────────────────┴────────────────────────────────┐  │
│  │  Rust Bridge (Tauri Commands)                    │  │
│  │  - RemoteCoordinator: reuses kernel::client directly │  │
│  │  - EventBridge: forwards daemon events to FE   │  │
│  │  - StateManager: local app state (window, theme) │  │
│  └────────────────┬────────────────────────────────┘  │
│                   │ Unix Socket / TCP                  │
└───────────────────┼─────────────────────────────────────┘
                    │
┌───────────────────┴─────────────────────────────────────┐
│  Yomi Kernel Daemon (existing, unchanged)                 │
│  - KernelServer (wire.rs / server/mod.rs)                │
│  - Coordinator (app/coordinator.rs)                      │
│  - SQLite sessions, checkpoint rewind, goal mode...      │
└─────────────────────────────────────────────────────────┘
```

### Rust Bridge Design (`crates/gui/src-tauri/src/`)

The GUI is a **separate workspace crate** at `crates/gui/`. It depends on `yomi-kernel` as a path dependency and contains both the Tauri Rust backend and the Svelte frontend.

**Key insight**: The `yomi-kernel` crate **already exports a full IPC client** (`RemoteCoordinator` in `kernel::client`). The GUI Rust layer does NOT reimplement the Wire Protocol — it simply wraps the existing `RemoteCoordinator` in Tauri commands.

```
crates/gui/
├── src-tauri/
│   ├── Cargo.toml              # workspace member, depends on yomi-kernel
│   ├── tauri.conf.json         # v2 mobile/desktop config
│   ├── capabilities/
│   │   └── default.json
│   └── src/
│       ├── main.rs             # Tauri builder + mobile setup
│       ├── lib.rs
│       ├── state.rs            # AppState: holds Arc<RemoteCoordinator>
│       ├── terminal/
│       │   ├── mod.rs
│       │   ├── session.rs      # TerminalSession (portable-pty)
│       │   └── manager.rs      # Multi-tab terminal manager
│       └── commands/
│           ├── mod.rs
│           ├── session.rs      # thin wrappers around CoordinatorApi
│           ├── chat.rs
│           ├── checkpoint.rs
│           ├── skill.rs
│           ├── system.rs
│           └── terminal.rs     # terminal_spawn, write, resize, kill
├── src/                        # Svelte 5 frontend
│   ├── app.html
│   ├── app.css
│   ├── routes/
│   └── lib/
├── index.html
├── vite.config.ts
├── package.json
└── tailwind.config.js
```

**Workspace manifest** (`Cargo.toml` at repo root):

```toml
[workspace]
members = ["crates/kernel", "crates/cli", "crates/tui", "crates/gui/src-tauri"]
resolver = "2"
```

**GUI crate manifest** (`crates/gui/src-tauri/Cargo.toml`):

```toml
[package]
name = "yomi-gui"
version = "0.1.0"
edition = "2021"

[dependencies]
tauri = { version = "2", features = [] }
tokio = { version = "1", features = ["rt-multi-thread", "macros"] }
portable-pty = "0.8"

# Internal crate — zero modifications to kernel
yomi-kernel = { path = "../../kernel" }

[features]
custom-protocol = ["tauri/custom-protocol"]
```

**Tauri State** (`crates/gui/src-tauri/src/state.rs`):

```rust
use std::sync::Arc;
use tokio::sync::Mutex;
use yomi_kernel::client::RemoteCoordinator;
use yomi_kernel::transport::SocketAddr;

pub struct AppState {
    pub coordinator: Arc<Mutex<RemoteCoordinator>>,
}

impl AppState {
    pub async fn new(addr: SocketAddr) -> Self {
        let coordinator = RemoteCoordinator::connect(&addr).await
            .expect("Failed to connect to kernel daemon");
        Self {
            coordinator: Arc::new(Mutex::new(coordinator)),
        }
    }
}
```

**Tauri command example** (`crates/gui/src-tauri/src/commands/session.rs`):

```rust
use tauri::State;
use yomi_kernel::client::CoordinatorApi;
use yomi_kernel::permissions::Level;

#[tauri::command]
pub async fn create_session(
    state: State<'_, crate::state::AppState>,
    project_path: String,
    auto_approve_level: Level,
) -> Result<String, String> {
    let coordinator = state.coordinator.lock().await;
    let session_id = coordinator
        .create_session(project_path.into(), auto_approve_level)
        .await
        .map_err(|e| e.to_string())?;
    Ok(session_id.0)
}
```

**Key Insight**: The Tauri Rust layer acts as a **second client** to the daemon, identical in role to the TUI's `EventPump`. It maintains one active `Subscribe` per viewed session and forwards `Event` payloads to the frontend via `tauri::Emitter`.

---

## IPC Frontend ↔ Rust

### Commands (Frontend → Rust, async)

All commands return a unified `Result<T, GuiError>`.

```typescript
// types/api.ts
interface GuiError {
  code: string;      // e.g. "session_error", "network_error"
  message: string;
  detail?: unknown;
}

// commands
interface SessionApi {
  listSessions(args: ListArgs): Promise<Session[]>;
  createSession(path: string, level: PermissionLevel): Promise<string>; // returns session_id
  restoreSession(id: string, level: PermissionLevel): Promise<string>;
  forkSession(parentId: string, level: PermissionLevel): Promise<string>;
  deleteSession(id: string): Promise<void>;
  shutdownSession(id: string): Promise<void>;
  getSessionMessages(id: string): Promise<Message[]>;
}

interface ChatApi {
  sendMessage(sessionId: string, blocks: ContentBlock[]): Promise<void>;
  subscribe(sessionId: string): Promise<void>;
  unsubscribe(sessionId: string): Promise<void>;
  sendCommand(sessionId: string, cmd: ControlCommand): Promise<void>;
}

interface CheckpointApi {
  getCheckpoints(sessionId: string): Promise<Checkpoint[]>;
  rewind(sessionId: string, messageId: string, target: RewindTarget): Promise<void>;
}

interface SkillApi {
  listSkills(): Promise<Skill[]>;          // loaded in current agent config
  reloadAgentConfig(): Promise<void>;
}
```

### Events (Rust → Frontend, push)

```typescript
// Listen via Tauri event system
interface KernelEventPayload {
  sessionId: string;
  event: Event;   // serialized from kernel::event::Event
}

type Event =
  | { type: 'agent_start' }
  | { type: 'agent_end' }
  | { type: 'model_chunk'; content: string }
  | { type: 'model_thinking'; content: string }
  | { type: 'tool_start'; toolName: string; input: unknown }
  | { type: 'tool_end'; output: ToolOutput }
  | { type: 'permission_request'; reqId: string; toolName: string; input: unknown }
  | { type: 'error'; message: string }
  | { type: 'goal_progress'; iteration: number }
  | { type: 'system_notification'; message: string };
```

Frontend registers a global listener on app mount. Events are routed directly into the per-session reactive state:

```svelte
<script>
  import { listen } from '@tauri-apps/api/event';
  import { onMount } from 'svelte';
  import { sessions } from '$lib/state.svelte';

  onMount(() => {
    const unlisten = listen('kernel:event', (e) => {
      const s = sessions.get(e.payload.sessionId);
      if (s) handleEvent(s, e.payload.event);
    });
    return () => unlisten.then(fn => fn());
  });
</script>
```

---

## Frontend State Design (Svelte 5 Runes)

No external store library. Runes provide fine-grained reactivity.

```typescript
// lib/state.svelte.ts

// --- Global App State ---
export const appState = $state({
  connectionStatus: 'disconnected' as 'connected' | 'disconnected' | 'connecting',
  currentTheme: 'system' as 'light' | 'dark' | 'system',
  sidebarCollapsed: false,     // desktop only: icon-only vs full sidebar
});

// --- Per-Session State ---
export interface SessionState {
  id: string;
  projectPath: string;
  messages: Message[];
  streaming: boolean;
  pendingPermission: PermissionRequest | null;
  thinking: { content: string; elapsedMs: number } | null;
  checkpoints: Checkpoint[];
  todos: Todo[];
  scrollToBottom: boolean;
}

export const sessions = $state<Map<string, SessionState>>(new Map());
export const activeSessionId = $state<string | null>(null);

// --- Derived ---
export const activeSession = $derived(
  activeSessionId ? sessions.get(activeSessionId) ?? null : null
);
```

---

## Page & Layout Design

### Layout Shell (`src/lib/components/Layout.svelte`)

Responsive adaptive shell with **collapsible sidebar**, **bottom terminal panel**, and **tabbed main area**:

```
Desktop (>= 1024px):
┌─────────┬──────────────────────────────────────┐
│ Sidebar │  Main Content Area (tabbed)          │
│ 260px   │  ┌────────────────────────────────┐  │
│         │  │ 💬 Chat  │ 📄 main.rs │ 🖥️ T1 │  │  <- Tabs
│         │  ├────────────────────────────────┤  │
│ Sessions│  │                                │  │
│ ● yomi  │  │  [active tab content]          │  │
│ ○ back  │  │                                │  │
│ ○ docs  │  │                                │  │
│ + New   │  │                                │  │
│         │  └────────────────────────────────┘  │
│ ────────┤                                    │
│ 📁 Files│                                    │
│ 📂 src  │                                    │
│   main.rs│                                    │
│   lib.rs │                                    │
│ 📂 crates│                                    │
│         │                                    │
├─────────┴──────────────────────────────────────┤
│ 🖥️ Terminal (drag handle ───────)  [+] [×]   │  <- Bottom panel, 180px
│ $ cargo build                                  │
│    Compiling yomi v0.1.0                      │
└────────────────────────────────────────────────┘

Tablet (768px - 1023px):
┌──────────────────────────────────────────────┐
│ ≡ │ Yomi              💬 📁 🖥️ ⚙️           │  <- Compact header
├──────────────────────────────────────────────┤
│ Session chips (horizontal scroll)            │
│ [● yomi] [○ backend] [○ docs] [+]            │
├──────────────────────────────────────────────┤
│ Main Content Area (tabbed)                   │
│                                              │
├──────────────────────────────────────────────┤
│ Terminal (collapsible, half-height)          │
└──────────────────────────────────────────────┘

Mobile (< 768px):
┌─────────────────────────────┐
│ Yomi    [💬] [📁] [🖥️] [⚙️]│  <- Header with view switcher
├─────────────────────────────┤
│                             │
│    Main Content Area        │
│  (Chat / File / Terminal)   │
│                             │
├─────────────────────────────┤
│ Bottom Navigation           │
│  💬    📁    🖥️    ⚙️      │
│ Chat  Files  Term  Settings │
└─────────────────────────────┘
```

**Layout Zones**

| Zone | Behavior | Mobile Equivalent |
|---|---|---|
| **Session Band** (sidebar top) | Always visible, lists active sessions with status dots and unread badges | Horizontal scrollable chip bar below header |
| **Explorer** (sidebar middle) | Collapsible file tree, follows `projectPath` of active session | Full-screen "Files" tab with breadcrumb |
| **Main Area** (center) | Tabbed workspace: Chat (pinned) + dynamic file/editor/terminal tabs | Single view with bottom nav switcher |
| **Terminal Panel** (bottom) | Collapsible drag-to-resize panel, multi-tab shell sessions | Full-screen "Terminal" tab |
| **Navigation** (sidebar bottom / mobile nav) | Settings, Skills, Help icons | Bottom nav bar |

**Breakpoints**
- `sm`: 640px
- `md`: 768px → session chips appear, sidebar collapses to icons
- `lg`: 1024px → full sidebar with explorer
- `xl`: 1280px → wider gutters, larger editor font

**Implementation**

```svelte
<!-- Layout.svelte -->
<script>
  let { children } = $props();
  let isDesktop = $derived(typeof window !== 'undefined' && window.innerWidth >= 1024);
  let isTablet = $derived(typeof window !== 'undefined' && window.innerWidth >= 768 && window.innerWidth < 1024);
  let sidebarCollapsed = $state(false);
  let terminalOpen = $state(false);
  let terminalHeight = $state(200);
</script>

<div class="h-screen w-screen flex flex-col bg-background text-foreground overflow-hidden">
  {#if isDesktop}
    <div class="flex flex-1 overflow-hidden">
      <!-- Sidebar: Session Band + Explorer + Nav -->
      <aside class="flex flex-col border-r border-border transition-all {sidebarCollapsed ? 'w-16' : 'w-64'}">
        <SessionBand collapsed={sidebarCollapsed} />
        {#if !sidebarCollapsed}
          <ExplorerTree />
        {/if}
        <div class="mt-auto">
          <BottomNavIcons />
        </div>
      </aside>
      
      <!-- Main: Tabs + Content + Terminal -->
      <div class="flex-1 flex flex-col min-w-0">
        <TabBar />
        <main class="flex-1 overflow-hidden">
          {@render children()}
        </main>
        {#if terminalOpen}
          <TerminalPanel bind:height={terminalHeight} />
        {/if}
      </div>
    </div>
  {:else if isTablet}
    <!-- Tablet: compact header + session chips + main + terminal -->
    <TabletHeader bind:sidebarCollapsed />
    <SessionChipBar />
    <main class="flex-1 overflow-hidden">
      {@render children()}
    </main>
    {#if terminalOpen}
      <TerminalPanel bind:height={terminalHeight} />
    {/if}
  {:else}
    <!-- Mobile: header + main + bottom nav -->
    <MobileHeader />
    <main class="flex-1 overflow-y-auto">
      {@render children()}
    </main>
    <MobileBottomNav />
  {/if}
</div>
```

---

### 1. Chat View (`src/routes/chat/+page.svelte`)

The primary interface. Modeled after modern messaging apps.

#### Desktop Chat

```
┌─────────────────────────────────────────────┐
│ Session Name              [YOLO ▼] [⏹] [↩️] │  <- Toolbar
├─────────────────────────────────────────────┤
│                                             │
│  ┌────────────┐                             │
│  │ User msg   │                             │
│  └────────────┘                             │
│           ┌────────────────────────────────┐  │
│           │ Assistant response...          │  │
│           │ ```rust                      │  │
│           │ fn main() {}                 │  │
│           │ ```                          │  │
│           │ [Thinking ▼]                 │  │
│           └────────────────────────────────┘  │
│  ┌────────────┐                             │
│  │ 🔧 shell   │                             │
│  │ ls -la     │                             │
│  │ Output...  │                             │
│  └────────────┘                             │
│                                             │
│  ┌────────────────────────────────────────┐ │
│  │ ⚠️ Permission Request                  │ │
│  │ Tool: write to src/main.rs             │ │
│  │ [View Diff]  [Allow] [Block] [Always]  │ │
│  └────────────────────────────────────────┘ │
│                                             │
├─────────────────────────────────────────────┤
│ ┌─────────────────────────────────────────┐ │
│ │  Ask anything...                    ⬆️  │ │
│ └─────────────────────────────────────────┘ │
└─────────────────────────────────────────────┘
```

#### Mobile Chat

- Same message list, but messages are full-width with bubble styling
- Toolbar collapses into a top-right `⋯` menu sheet
- Input bar sits above bottom nav, with larger touch targets (min 44px)
- Permission requests become bottom sheets that slide up

#### Components

| Component | File | Description |
|---|---|---|
| `MessageList` | `chat/MessageList.svelte` | Scrollable message list. Uses shadcn `ScrollArea`. |
| `UserBubble` | `chat/UserBubble.svelte` | Right-aligned, muted background. |
| `AssistantBubble` | `chat/AssistantBubble.svelte` | Left-aligned, primary surface. Contains `MarkdownRenderer`. |
| `ToolCard` | `chat/ToolCard.svelte` | Collapsible card for tool calls. Uses shadcn `Collapsible`. |
| `ThinkingBlock` | `chat/ThinkingBlock.svelte` | Collapsible block for thinking content with elapsed timer. |
| `ChatInput` | `chat/ChatInput.svelte` | Auto-growing textarea with submit button. shadcn `Textarea` + `Button`. |
| `StreamIndicator` | `chat/StreamIndicator.svelte` | Animated dot indicator when `streaming=true`. |
| `CheckpointBar` | `chat/CheckpointBar.svelte` | Horizontal timeline of checkpoints, clickable to rewind. shadcn `Slider` + `Tooltip`. |
| **`DiffPreview`** | **`diff/DiffPreview.svelte`** | **Fancy multi-file diff viewer with split/unified views, intra-line highlighting, and partial apply. shadcn `Dialog` / `Sheet`.** |
| `TabBar` | `layout/TabBar.svelte` | Main area tabs: Chat (pinned), file previews, terminals. shadcn `Tabs`. |
| `SessionBand` | `layout/SessionBand.svelte` | Sidebar session switcher with status dots and unread badges. |
| `ExplorerTree` | `explorer/ExplorerTree.svelte` | Collapsible file tree. shadcn `Collapsible` + `ScrollArea`. |
| `FilePreview` | `editor/FilePreview.svelte` | Read-only file viewer with Shiki syntax highlight and line numbers. |
| `FileEditor` | `editor/FileEditor.svelte` | CodeMirror 6 embedded editor with save/discard. |
| `TerminalPanel` | `terminal/TerminalPanel.svelte` | Bottom resizable panel wrapping xterm.js. |

**Virtual Scrolling**: Use `svelte-virtual-list` or CSS `content-visibility` for long sessions. Messages are append-only, so simple scroll-anchor to bottom is sufficient.

**Markdown Rendering**:
- Custom Svelte component wrapping `markdown-it`
- Code blocks rendered with Shiki (async, cached)
- All tool call JSON shown in collapsible `<pre>` blocks

```svelte
<!-- AssistantBubble.svelte -->
<script>
  let { message } = $props();
</script>

<div class="flex gap-3 max-w-[85%]">
  <div class="shrink-0 w-8 h-8 rounded-full bg-indigo-500 flex items-center justify-center text-white text-sm font-bold">
    Y
  </div>
  <div class="flex-1 space-y-2">
    <div class="rounded-2xl rounded-tl-sm bg-white dark:bg-neutral-800 px-4 py-3 shadow-sm border border-neutral-200 dark:border-neutral-700">
      <MarkdownRenderer content={message.content} />
    </div>
    {#if message.thinking}
      <ThinkingBlock thinking={message.thinking} />
    {/if}
  </div>
</div>
```

---

### 1.2 Sidebar Session Switcher (`src/lib/components/layout/SessionBand.svelte`)

The **Session Band** lives at the top of the sidebar (or as a horizontal chip bar on mobile/tablet) and provides **instant session context switching** without leaving the current view.

#### Desktop Layout

```
┌─────────────────────────────┐
│  🤖 Yomi                    │
├─────────────────────────────┤
│  Sessions                   │
│  ┌───────────────────────┐ │
│  │ ● yomi-gui        🔔3 │ │  <- active, unread count
│  │ ○ backend-api        0│ │  <- inactive, clean
│  │ ○ docs-site        🔔1│ │  <- has unread messages
│  │ ───────────────────── │ │
│  │ +  New Session        │ │
│  └───────────────────────┘ │
│                             │
```

**Collapsed Sidebar (icon-only mode)**

```
┌──────┐
│ 🤖   │
├──────┤
│ ●y   │  <- active session, first letter
│ ○b   │  <- inactive, tooltip on hover
│ ○d   │
│ +    │
└──────┘
```

#### Session Item States

| State | Visual | Meaning |
|---|---|---|
| **Active** | Left accent border (`border-l-4 border-primary`), subtle bg highlight | Currently viewed session |
| **Inactive** | Neutral, hover shows `bg-secondary` | Background session |
| **Unread** | Badge with count (`bg-destructive text-white`) | New assistant/tool messages since last viewed |
| **Streaming** | Pulsing dot (`animate-pulse bg-primary`) | Agent is actively working in this session |
| **Error** | Left border `border-destructive` | Last operation failed |

#### Interactions

- **Click** → switch session instantly. Chat tab updates, explorer refreshes to new `projectPath`.
- **Right-click / long-press** → context menu: Fork, Rename (local alias), Export, Delete
- **Hover** (collapsed) → shadcn `Tooltip` shows full session name + project path
- **Drag** (desktop) → reorder sessions in sidebar (order persisted in local store)

```svelte
<!-- SessionBand.svelte -->
<script>
  let { collapsed = false } = $props();
  let sessions = $derived([...globalSessions.values()]);
</script>

<div class="flex flex-col gap-1 p-2">
  {#each sessions as session (session.id)}
    <button
      class="group flex items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors
        {session.id === activeSessionId 
          ? 'bg-primary/10 border-l-4 border-primary' 
          : 'hover:bg-secondary border-l-4 border-transparent'}"
      onclick={() => switchSession(session.id)}
      oncontextmenu={(e) => showContextMenu(e, session.id)}
    >
      <div class="relative shrink-0 w-2 h-2 rounded-full 
        {session.streaming ? 'animate-pulse bg-primary' : 
         session.id === activeSessionId ? 'bg-primary' : 'bg-muted-foreground'}"></div>
      {#if !collapsed}
        <span class="flex-1 truncate text-sm font-medium">{session.alias ?? formatShortId(session.id)}</span>
        {#if session.unread > 0}
          <span class="shrink-0 inline-flex items-center justify-center min-w-[1.25rem] h-5 px-1 rounded-full bg-destructive text-destructive-foreground text-xs font-bold">
            {session.unread}
          </span>
        {/if}
      {/if}
    </button>
  {/each}
  <button class="flex items-center gap-2 rounded-lg px-3 py-2 text-sm text-muted-foreground hover:bg-secondary transition-colors">
    <Plus size={16} />
    {#if !collapsed}New Session{/if}
  </button>
</div>
```

---

### 1.3 File Explorer & Editor (`src/lib/components/explorer/` + `src/lib/components/editor/`)

A lightweight **IDE-style file explorer** tied to the active session's `projectPath`. It enables browsing, previewing, and manually editing files — complementing the AI's automated `read`/`write`/`edit` tools.

> **FileSystemProvider abstraction**: All file operations (list, read, write, stat) go through the `FileSystemProvider` interface (`lib/fs/provider.ts`). The current implementation is `LocalFSProvider` which uses Tauri native `fs` APIs for zero-latency local editing. A future `KernelFSProvider` can be swapped in for remote kernel scenarios without touching any UI component.

#### Explorer (Sidebar)

```
┌─────────────────────────────┐
│  📁 Explorer               │
│  ┌───────────────────────┐ │
│  │ 🔍 Filter files...    │ │  <- shadcn Input
│  ├───────────────────────┤ │
│  │ 📂 src                │ │  <- Collapsible folder
│  │   📄 main.rs      ●   │ │  <- file, dot = unsaved edit
│  │   📄 lib.rs           │ │
│  │ 📂 crates             │ │
│  │   📁 kernel           │ │
│  │   📁 cli              │ │
│  │ 📄 Cargo.toml         │ │
│  │ 📄 README.md          │ │
│  │ 📄 .gitignore         │ │  <- gray = gitignored
│  └───────────────────────┘ │
│  [📂 Open Folder] [↻ Ref] │
└─────────────────────────────┘
```

**Features**

| Feature | Implementation |
|---|---|
| **File Tree** | Recursive directory listing via `LocalFSProvider` (wraps Tauri `fs.readDir`). Caches tree in reactive state. |
| **Git Awareness** | Reads `.gitignore` patterns to dim ignored files. Shows git status via shell `git status --short`. |
| **Filter** | shadcn `Input` with real-time fuzzy filtering on filename. |
| **Click File** | Opens file in main area as a **preview tab** (read-only, Shiki highlighted). |
| **Double-click / "Edit"** | Converts preview tab to **editor tab** (CodeMirror 6). |
| **New File / Folder** | Right-click context menu → create → calls `LocalFSProvider.writeFile()` (direct Tauri `fs` API). |
| **Refresh** | Rescans directory via `LocalFSProvider.listDir()`. Auto-refreshes when kernel events indicate file changes. |
| **Drag & Drop** | Reorder sidebar sections. Files can be dragged into ChatInput as attachments. |

#### File Preview Tab

Read-only view with rich formatting:
- **Code files**: Shiki syntax highlight + line numbers gutter
- **Markdown**: Rendered as HTML with same styling as chat markdown
- **Images**: Displayed inline (Tauri converts path to `asset://` URL)
- **Large files**: Virtual scroll + lazy loading (only render visible lines)
- **Binary files**: Hex dump preview

```svelte
<!-- FilePreview.svelte -->
<script>
  let { path, content } = $props();
  let highlighted = $state('');
  
  $effect(() => {
    shiki.highlight(content, { lang: detectLang(path), theme: currentTheme }).then(h => highlighted = h);
  });
</script>

<div class="h-full flex flex-col">
  <!-- Breadcrumb: src > lib > components > Layout.svelte -->
  <div class="flex items-center gap-1 px-4 py-2 border-b border-border text-sm text-muted-foreground">
    {#each breadcrumb(path) as part, i}
      <span class="hover:text-foreground cursor-pointer">{part}</span>
      {#if i < breadcrumb.length - 1}<ChevronRight size={14} />{/if}
    {/each}
    <div class="ml-auto flex gap-2">
      <Button variant="ghost" size="sm" onclick={openInEditor}>
        <Pencil size={14} class="mr-1" /> Edit
      </Button>
      <Button variant="ghost" size="sm" onclick={() => copyToChat(path)}>
        <MessageSquare size={14} class="mr-1" /> Ask AI
      </Button>
    </div>
  </div>
  
  <!-- Content -->
  <ScrollArea class="flex-1">
    <pre class="p-4 text-sm leading-relaxed"><code>{@html highlighted}</code></pre>
  </ScrollArea>
</div>
```

#### File Editor Tab (CodeMirror 6)

Embedded editor for manual code changes:
- **Editor**: CodeMirror 6 with `basicSetup` + `one` dark/light theme extension
- **Language support**: Dynamic loading of `@codemirror/lang-{rust,javascript,typescript,python,...}` via `languageMap`
- **Keymap**: Default + `Ctrl/Cmd+S` to save, `Ctrl/Cmd+Shift+S` to "Stage for AI" (send diff to chat)
- **Line numbers**: Relative or absolute (toggle in status bar)
- **Minimap**: Optional (collapsed by default on <1280px)
- **Status bar**: Line/column, language mode, dirty indicator, save button

**Save Behavior**

| Action | Behavior |
|---|---|
| `Ctrl+S` | Save via `FileSystemProvider` (currently `LocalFSProvider` → Tauri `fs.writeTextFile`). Dirty indicator clears instantly. Kernel sees updated content on next `read`. |
| "Stage for AI" | Computes diff of changes → opens new chat message with "I edited {file}:
```diff
..." pre-filled. User can send to have AI review. |
| Close without save | shadcn `AlertDialog` confirmation if dirty. |

```typescript
// lib/editor/cmSetup.ts
import { EditorView, basicSetup } from 'codemirror';
import { oneDark } from '@codemirror/theme-one-dark';
import { keymap } from '@codemirror/view';

export function createEditor(parent: HTMLElement, doc: string, lang: LanguageSupport) {
  const extensions = [
    basicSetup,
    lang,
    keymap.of([
      { key: 'Ctrl-s', run: saveFile },
      { key: 'Ctrl-Shift-s', run: stageForAI },
    ]),
    EditorView.theme({ /* shadcn color tokens */ }),
  ];
  
  if (currentTheme === 'dark') extensions.push(oneDark);
  
  return new EditorView({ doc, parent, extensions });
}
```

---

### 1.4 Integrated Terminal (`src/lib/components/terminal/`)

A **real shell** embedded in the GUI via xterm.js + Rust PTY. Not a simulation — it runs the user's actual `bash`/`zsh`/`powershell` with full ANSI support.

#### Desktop Layout

```
┌─────────────────────────────────────────────────────────────┐
│ Main Content Area                                           │
│ ...                                                         │
├─────────────────────────────────────────────────────────────┤  <- drag handle
│ 🖥️ Terminal                              [+] [▲] [×]        │
│ ┌────────┬────────────────────────────────────────────────┐ │
│ │ bash   │ $ cargo test                                  │ │
│ │ zsh ✕  │     Finished test [unoptimized + debuginfo]   │ │
│ │ [+]    │     Running unittests src/lib.rs              │ │
│ └────────┴────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────┘
```

**Panel Controls**
- **Drag handle** (top edge): Resize by dragging, min 100px, max 60% viewport
- **Collapse ▲**: Collapse to 40px tall bar showing current command
- **Close ×**: Destroy PTY process, remove panel
- **New tab +**: Spawn new shell in same working directory
- **Tab list** (left): Switch between terminal sessions. Close individual tabs.

#### Data Flow

```
User types in xterm.js
  → onData event (raw bytes, e.g., "ls\r")
  → Tauri invoke('terminal_write', { id: termId, data })
  → Rust: pty_master.write_all(data)
  → Shell process (bash/zsh)
  → stdout/stderr → pty_slave
  → Rust: read loop emits 'terminal:data' event
  → Frontend: xterm.write(data)
```

**Rust PTY Setup** (`crates/gui/src-tauri/src/terminal/`)

```rust
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tauri::{Emitter, Manager};

pub struct TerminalSession {
    id: String,
    pty_pair: Box<dyn portable_pty::PtyPair>,
    writer: Box<dyn std::io::Write + Send>,
    reader_handle: tokio::task::JoinHandle<()>,
}

impl TerminalSession {
    pub fn spawn(id: String, cwd: &std::path::Path, app_handle: tauri::AppHandle) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        
        let cmd = CommandBuilder::new_default_shell();
        let child = pair.slave.spawn_command(cmd)?;
        
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        
        // Spawn read loop that forwards to frontend
        let read_handle = tokio::spawn(async move {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break, // EOF
                    Ok(n) => {
                        let data = std::str::from_utf8(&buf[..n]).unwrap_or("");
                        let _ = app_handle.emit("terminal:data", json!({ "id": id, "data": data }));
                    }
                    Err(_) => break,
                }
            }
        });
        
        Ok(Self { id, pty_pair: pair, writer, reader_handle: read_handle })
    }
    
    pub fn write(&mut self, data: &str) -> Result<()> {
        self.writer.write_all(data.as_bytes())?;
        self.writer.flush()?;
        Ok(())
    }
    
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.pty_pair.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
        Ok(())
    }
}
```

**Frontend xterm.js Setup**

```svelte
<!-- TerminalPanel.svelte -->
<script>
  import { Terminal } from '@xterm/xterm';
  import { FitAddon } from '@xterm/addon-fit';
  import { listen } from '@tauri-apps/api/event';
  import { invoke } from '@tauri-apps/api/tauri';
  
  let { id, cwd } = $props();
  let container: HTMLElement;
  let term: Terminal;
  let fitAddon: FitAddon;
  
  onMount(async () => {
    term = new Terminal({
      fontFamily: 'JetBrains Mono, monospace',
      fontSize: 14,
      theme: currentTheme === 'dark' ? oneDarkTheme : oneLightTheme,
      cursorBlink: true,
      scrollback: 10000,
    });
    
    fitAddon = new FitAddon();
    term.loadAddon(fitAddon);
    term.open(container);
    fitAddon.fit();
    
    // Spawn PTY via Rust
    await invoke('terminal_spawn', { id, cwd, cols: term.cols, rows: term.rows });
    
    // Forward user input to Rust PTY
    term.onData((data) => invoke('terminal_write', { id, data }));
    
    // Receive PTY output
    const unlisten = await listen('terminal:data', (e) => {
      if (e.payload.id === id) term.write(e.payload.data);
    });
    
    // Resize handling
    const resizeObserver = new ResizeObserver(() => {
      fitAddon.fit();
      invoke('terminal_resize', { id, cols: term.cols, rows: term.rows });
    });
    resizeObserver.observe(container);
    
    return () => {
      unlisten();
      resizeObserver.disconnect();
      term.dispose();
      invoke('terminal_kill', { id });
    };
  });
</script>

<div bind:this={container} class="h-full w-full bg-terminal"></div>
```

#### Terminal-AI Integration

The terminal is deeply integrated with the AI workflow:

| Feature | How it works |
|---|---|
| **Run in Terminal** | AI suggests `cargo test` → Chat shows a button "Run in Terminal" → command is sent to active terminal tab, user sees live output |
| **Explain Error** | User selects error text in terminal → "Explain" context menu → selected text sent as new user message |
| **Auto-capture** | When a command fails (non-zero exit), terminal automatically offers "Debug with AI" |
| **Working Directory Sync** | Terminal tabs inherit `projectPath` from the active session. Changing session switches terminal cwd via `cd`. |
| **Command History** | Terminal history is searchable and can be inserted into ChatInput as code blocks |

#### Styling

```css
@theme {
  --color-terminal-bg: #0c0c0c;
  --color-terminal-fg: #cccccc;
  --color-terminal-cursor: #ffffff;
}

/* xterm.js theme mapping */
.xterm-viewport { @apply bg-terminal-bg; }
```

---

### 1.5 Diff Preview System (`src/routes/diff/+page.svelte` & overlay)

A first-class, **fancy diff preview experience** that appears before any file mutation is committed. This is Yomi's killer UX feature — users don't just see "write to src/main.rs yes/no", they see **exactly** what will change, down to individual characters, and can cherry-pick which hunks to apply.

#### Architecture

```
PreToolUse (write/edit)
  ↓
GUI reads original file via existing read tool
  ↓
Front-end computes diff (fast-diff + diff-match-patch for intra-line)
  ↓
DiffPreview modal/sheet opens
  ↓
User reviews → approves all / approves partial / rejects
  ↓
GUI sends permission response (+ optional updated_input with filtered patch)
```

**Key principle**: The diff is computed **entirely in the frontend** so it renders instantly. The kernel daemon does not need a new API.

#### Desktop Layout

```
┌──────────────────────────────────────────────────────────────┐
│ 📝 Diff Preview — 3 files changed              [×]           │
├────────────────┬─────────────────────────────────────────────┤
│                │  src/lib/components/Layout.svelte           │
│ 📄 Layout.svelte├─────────────────────────────────────────────┤
│    12 changes  │  [Split │ Unified]  [○] Old  [●] New         │
│                │  ┌───────────────────────────────────────┐  │
│ 📄 main.rs     │  │  1  │ import { onMount } from 'svelte'; │  │
│     3 changes  │  │  2  │ import { fade } from 'svelte/transition';│  │
│                │  │  3  │                                       │  │
│ 📄 types.ts    │  │  4  │ export interface SessionState {      │  │
│     1 change   │  │     │-  id: string;                       │  │
│                │  │  4  │+  id: string | null;                 │  │
│ ────────────── │  │  5  │  projectPath: string;                │  │
│                │  │  6  │  messages: Message[];                │  │
│ [✓] Hunk 1     │  │     │                                       │  │
│ [✓] Hunk 2     │  │  12 │- function foo() {                    │  │
│ [ ] Hunk 3     │  │  12 │+ function foo(): void {              │  │
│                │  │     │      ^^^^^^^^^^ intra-line highlight  │  │
│                │  │  13 │    return bar;                       │  │
│                │  │  14 │  }                                    │  │
│                │  └───────────────────────────────────────┘  │
│                │                                               │
│                │  [☐ Apply all hunks]                          │
│                │                                               │
├────────────────┴─────────────────────────────────────────────┤
│ [← Prev file]          [Accept Selected (2/3)]  [Reject All]   │
└──────────────────────────────────────────────────────────────┘
```

#### Mobile Layout

- Full-screen bottom sheet that can be dragged to full height
- File list becomes a horizontal scrollable chip bar at top
- Split view is **vertically stacked** (old above new) or hidden in favor of unified
- Hunk checkboxes are larger touch targets (min 44×44px)
- Bottom sticky action bar with "Accept" / "Reject"

#### Features

| Feature | Implementation |
|---|---|
| **Split View** | Side-by-side Old / New panes. Synchronized scroll via shared scroll position ref. |
| **Unified View** | Single column: removed lines (red), added lines (green), context lines (neutral). |
| **Intra-line Diff** | Character-level diff within a changed line. `diff-match-patch` marks exact character ranges. Red strikethrough for removed chars, green underline for added chars. |
| **Syntax Highlight** | Shiki highlights both old and new versions independently (async, cached). Line ranges passed to Shiki for partial rendering. |
| **Partial Apply** | Every hunk has a checkbox. Every individual line can also be toggled. Unchecked hunks are stripped from the final patch before sending to kernel. |
| **Hunk Navigation** | `j`/`k` jump between hunks. Current hunk is softly highlighted with a ring border. `Space` toggles the focused hunk. |
| **Multi-file** | Left sidebar lists all files in the current tool call batch. Badge shows change count per file. Clicking switches the viewer. |
| **Animations** | Hunks enter with `slide` + `fade` (staggered). Line additions glow green briefly. Deletions fade to red. Checkbox state transitions are smooth. |
| **Copy Action** | Hover any line to show "Copy line" / "Copy hunk" buttons. |
| **Search** | `Ctrl+F` to search within the diff content. |

#### Components

| Component | File | Description |
|---|---|---|
| `DiffPreview` | `diff/DiffPreview.svelte` | Main orchestrator. shadcn `Dialog` (desktop) / `Sheet` (mobile). Manages multi-file state and view mode. |
| `DiffFileTree` | `diff/DiffFileTree.svelte` | Left sidebar file list. shadcn `ScrollArea`. |
| `DiffViewer` | `diff/DiffViewer.svelte` | Core renderer. Switches between Split and Unified. |
| `SplitView` | `diff/SplitView.svelte` | Two-pane layout with synchronized scrolling. |
| `UnifiedView` | `diff/UnifiedView.svelte` | Single-column layout. |
| `DiffHunk` | `diff/DiffHunk.svelte` | Renders one hunk header + lines. Manages hunk-level checkbox. Uses shadcn `Checkbox`. |
| `DiffLine` | `diff/DiffLine.svelte` | Single line renderer. Handles line number gutter, gutter icons (+/−/·), intra-line highlighting. |
| `IntraLineDiff` | `diff/IntraLineDiff.svelte` | Character-level diff output. Wraps segments in `<span>` with diff classes. |
| `DiffActionBar` | `diff/DiffActionBar.svelte` | Sticky bottom bar with stats and action buttons. shadcn `Button`. |

#### Data Flow

```typescript
// lib/diff/types.ts
interface FileDiff {
  path: string;
  oldContent: string;
  newContent: string;
  hunks: Hunk[];
}

interface Hunk {
  id: string;           // stable id for keyed rendering
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: DiffLine[];
  applied: boolean;   // checkbox state
}

interface DiffLine {
  type: 'context' | 'add' | 'remove';
  oldLineNum: number | null;
  newLineNum: number | null;
  content: string;
  intraLineSegments?: IntraSegment[]; // only for add/remove
}

interface IntraSegment {
  type: 'equal' | 'remove' | 'add';
  text: string;
}
```

**Diff Computation** (frontend):
1. Run `fast-diff` on oldContent vs newContent to get line-level diff.
2. Group lines into hunks (context: 3 lines padding around changes).
3. For each `add`/`remove` line pair that "looks like a modification" (same line number context), run `diff-match-patch` to generate `IntraSegment[]`.
4. Render via Svelte keyed `{#each}` for animation stability.

#### Interaction with Permission System

The `DiffPreview` opens as an **intercepting overlay** when `PreToolUse` arrives for `write` or `edit` tools. Non-file mutation tools (e.g., `shell`) use a simple inline `AlertDialog` for confirmation.

```svelte
<!-- Simplified flow inside ChatView.svelte -->
{#if pendingPermission?.toolName === 'write' || pendingPermission?.toolName === 'edit'}
  <DiffPreview
    files={pendingPermission.computedDiffs}
    onApproveAll={() => respond(true)}
    onReject={() => respond(false)}
    onApprovePartial={(filteredDiffs) => respond(true, filteredDiffs)}
  />
{/if}
```

When partial apply is selected, the GUI constructs a modified `edit`/`write` payload containing only the approved hunks and sends it as `updated_input` to the kernel (the kernel Wire Protocol already supports `updated_input` via `PreToolDecision`).

#### Styling Tokens

```css
/* Tailwind / shadcn tokens used by diff viewer */
.diff-add       { @apply bg-emerald-500/10 border-l-2 border-emerald-500; }
.diff-remove    { @apply bg-red-500/10 border-l-2 border-red-500; }
.diff-context   { @apply bg-transparent border-l-2 border-transparent; }
.diff-intra-add { @apply bg-emerald-500/30 rounded-sm px-0.5; }
.diff-intra-del { @apply bg-red-500/30 line-through rounded-sm px-0.5; }
.diff-gutter    { @apply text-xs text-muted-foreground select-none text-right pr-3 w-12 shrink-0; }
```

---

---

### 2. Skill Manager (`src/routes/skills/+page.svelte`)

Visual management of the skill system.

```
┌─────────────────────────────────────────────┐
│ Skills                            [↻ Reload]│
├─────────────────────────────────────────────┤
│                                             │
│  Active Skills (3)                          │
│  ┌────────────────────────────────────────┐ │
│  │ 🛡️ security-guard                      │ │
│  │    Hooks: PreToolUse (shell, write)    │ │
│  │    [On]                                │ │
│  └────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────┐ │
│  │ ✨ auto-format                         │ │
│  │    Hooks: PostToolUse (edit, write)    │ │
│  │    [On]                                │ │
│  └────────────────────────────────────────┘ │
│                                             │
│  Available Skills (5)                       │
│  ┌────────────────────────────────────────┐ │
│  │ 📦 rust-optimization                     │ │
│  │    Triggers: perf, optimize, slow      │ │
│  │    [Enable →]                          │ │
│  └────────────────────────────────────────┘ │
│                                             │
│  [Open Skill Folder]                        │
└─────────────────────────────────────────────┘
```

**Note**: Yomi kernel loads skills from disk at startup. The GUI shows the currently loaded skills via `listSkills` and can trigger `ReloadAgentConfig` to pick up new skill files. Skill enable/disable is initially "reload-based" (no runtime toggle in kernel yet), but the GUI can maintain a local allowlist filter.

---

### 3. Settings (`src/routes/settings/+page.svelte`)

```
┌─────────────────────────────────────────────┐
│ Settings                                    │
├─────────────────────────────────────────────┤
│                                             │
│  Connection                                 │
│  ┌────────────────────────────────────────┐ │
│  │ Daemon Socket        unix:///run/yomi  │ │
│  │ Status: ● Connected                    │ │
│  └────────────────────────────────────────┘ │
│                                             │
│  Appearance                                 │
│  ┌────────────────────────────────────────┐ │
│  │ Theme [○ Light ● Dark ○ System]        │ │
│  │ Font Size [14px ▼]                     │ │
│  │ Compact Mode [On]                      │ │
│  └────────────────────────────────────────┘ │
│                                             │
│  Behavior                                   │
│  ┌────────────────────────────────────────┐ │
│  │ Auto-approve level [Safe ▼]            │ │
│  │ Desktop notifications [On]               │ │
│  │ Auto-scroll to bottom [On]               │ │
│  └────────────────────────────────────────┘ │
│                                             │
│  Danger Zone                                │
│  [Clear All Sessions] [Reset App Data]        │
└─────────────────────────────────────────────┘
```

---

## Responsive Design Strategy

### Tailwind Config (v4) + shadcn Theme

```css
/* app.css - Tailwind v4 entry + shadcn theme variables */
@import "tailwindcss";

@theme {
  /* Breakpoints */
  --breakpoint-sm: 640px;
  --breakpoint-md: 768px;
  --breakpoint-lg: 1024px;
  --breakpoint-xl: 1280px;

  /* Font */
  --font-sans: "Inter", system-ui, sans-serif;

  /* Radius */
  --radius-lg: 0.75rem;
  --radius-xl: 1rem;

  /* shadcn color tokens (maps to CSS variables used by shadcn-svelte components) */
  --color-background: var(--background);
  --color-foreground: var(--foreground);
  --color-card: var(--card);
  --color-card-foreground: var(--card-foreground);
  --color-popover: var(--popover);
  --color-popover-foreground: var(--popover-foreground);
  --color-primary: var(--primary);
  --color-primary-foreground: var(--primary-foreground);
  --color-secondary: var(--secondary);
  --color-secondary-foreground: var(--secondary-foreground);
  --color-muted: var(--muted);
  --color-muted-foreground: var(--muted-foreground);
  --color-accent: var(--accent);
  --color-accent-foreground: var(--accent-foreground);
  --color-destructive: var(--destructive);
  --color-destructive-foreground: var(--destructive-foreground);
  --color-border: var(--border);
  --color-input: var(--input);
  --color-ring: var(--ring);

  /* Custom Yomi brand colors */
  --color-brand-50: #eef2ff;
  --color-brand-500: #6366f1;
  --color-brand-600: #4f46e5;
  --color-brand-900: #312e81;
}

/* Light theme (default) */
:root {
  --background: 0 0% 100%;
  --foreground: 240 10% 3.9%;
  --card: 0 0% 100%;
  --card-foreground: 240 10% 3.9%;
  --popover: 0 0% 100%;
  --popover-foreground: 240 10% 3.9%;
  --primary: 239 84% 67%;
  --primary-foreground: 0 0% 100%;
  --secondary: 240 4.8% 95.9%;
  --secondary-foreground: 240 5.9% 10%;
  --muted: 240 4.8% 95.9%;
  --muted-foreground: 240 3.8% 46.1%;
  --accent: 240 4.8% 95.9%;
  --accent-foreground: 240 5.9% 10%;
  --destructive: 0 72.2% 50.6%;
  --destructive-foreground: 0 0% 98%;
  --border: 240 5.9% 90%;
  --input: 240 5.9% 90%;
  --ring: 239 84% 67%;
}

/* Dark theme */
.dark {
  --background: 240 10% 3.9%;
  --foreground: 0 0% 98%;
  --card: 240 10% 3.9%;
  --card-foreground: 0 0% 98%;
  --popover: 240 10% 3.9%;
  --popover-foreground: 0 0% 98%;
  --primary: 239 84% 67%;
  --primary-foreground: 0 0% 100%;
  --secondary: 240 3.7% 15.9%;
  --secondary-foreground: 0 0% 98%;
  --muted: 240 3.7% 15.9%;
  --muted-foreground: 240 5% 64.9%;
  --accent: 240 3.7% 15.9%;
  --accent-foreground: 0 0% 98%;
  --destructive: 0 62.8% 30.6%;
  --destructive-foreground: 0 0% 98%;
  --border: 240 3.7% 15.9%;
  --input: 240 3.7% 15.9%;
  --ring: 239 84% 67%;
}
```

### Mobile-First Patterns

| Pattern | Mobile (< md) | Desktop (>= md) |
|---|---|---|
| Navigation | Bottom tab bar (4 items) | Left sidebar (fixed 240px) |
| Chat bubbles | Full width, 12px margin | 70% width, aligned left/right |
| Input bar | Single-line + expand button | Auto-grow textarea |
| Session list | Cards with swipe actions | Compact rows with hover menus |
| Modals | Bottom sheets (slide up) | Centered dialog (max-width) |
| Touch target | Min 44×44px | Standard pointer targets |
| Font size | Base 16px (prevents zoom) | Base 14px |
| Padding | 16px gutters | 24px gutters |

### Dark Mode

- Use `dark:` variant throughout
- System preference detection via `window.matchMedia('(prefers-color-scheme: dark)')`
- Persist preference in Tauri `localStorage` / `store` plugin
- No flicker: apply class in `<html>` on load via a blocking inline script

---

## Mobile App Packaging (Tauri v2)

Tauri v2 enables iOS and Android builds from the same codebase.

### Platform-Specific Adaptations

**iOS**
- Safe area insets via `env(safe-area-inset-*)` in CSS
- Home indicator aware: add `pb-[env(safe-area-inset-bottom)]` to bottom nav
- Haptic feedback on permission requests (Tauri `haptics` plugin)
- Status bar style synced with dark mode

**Android**
- Edge-to-edge with `android:windowLayoutInDisplayCutoutMode="shortEdges"`
- Back button handler: navigate back in view stack, exit app only from Chat
- Keyboard avoidance: input bar uses `windowSoftInputMode="adjustResize"`

**Shared Mobile**
- Use Tauri `biometric` plugin for optional "YOLO mode" authentication (face/touch ID to skip confirmations)
- Share sheet for exporting sessions
- Deep links: `yomi://session/{id}` to resume a session from notification/widget

---

## Project Structure

```
yomi/                         # workspace root
├── Cargo.toml                # workspace manifest (add crates/gui/src-tauri)
├── crates/
│   ├── kernel/               # existing, ZERO modifications
│   ├── cli/                  # existing
│   ├── tui/                  # existing
│   └── gui/                  # NEW: Tauri v2 project
│       ├── src-tauri/
│       │   ├── Cargo.toml    # yomi-gui crate, depends on yomi-kernel
│       │   ├── tauri.conf.json
│       │   ├── capabilities/
│       │   │   └── default.json
│       │   └── src/
│       │       ├── main.rs
│       │       ├── lib.rs
│       │       ├── state.rs                # AppState: Arc<Mutex<RemoteCoordinator>>
│       │       ├── terminal/
│       │       │   ├── mod.rs
│       │       │   ├── session.rs
│       │       │   └── manager.rs
│       │       └── commands/
│       │           ├── mod.rs
│       │           ├── session.rs
│       │           ├── chat.rs
│       │           ├── checkpoint.rs
│       │           ├── skill.rs
│       │           ├── system.rs
│       │           └── terminal.rs
│       ├── src/                # Frontend (Svelte 5 + Tailwind)
│       │   ├── app.html
│       │   ├── app.css
│       │   ├── app.d.ts
│       │   ├── routes/
│       │   │   ├── +layout.svelte
│       │   │   ├── +page.svelte
│       │   │   ├── chat/
│       │   │   ├── skills/
│       │   │   └── settings/
│       │   ├── lib/
│       │   │   ├── components/
│       │   │   │   ├── ui/
│       │   │   │   ├── layout/
│       │   │   │   ├── chat/
│       │   │   │   ├── explorer/
│       │   │   │   ├── editor/
│       │   │   │   ├── terminal/
│       │   │   │   ├── diff/
│       │   │   │   └── ...
│       │   │   ├── diff/
│       │   │   ├── editor/
│       │   │   ├── fs/
│       │   │   ├── terminal/
│       │   │   ├── state.svelte.ts
│       │   │   ├── api.ts
│       │   │   ├── utils.ts
│       │   │   └── constants.ts
│       │   └── assets/
│       │       └── logo.svg
│       ├── index.html
│       ├── vite.config.ts
│       ├── svelte.config.js
│       ├── tsconfig.json
│       └── package.json
├── docs/
├── examples/
└── ...
```

**Workspace manifest** (`Cargo.toml` at repo root):

```toml
[workspace]
members = ["crates/kernel", "crates/cli", "crates/tui", "crates/gui/src-tauri"]
resolver = "2"
```

---
│   ├── routes/
│   │   ├── +layout.svelte          # Root layout with ThemeProvider + EventListener
│   │   ├── +page.svelte            # Redirect to /chat
│   │   ├── chat/
│   │   │   ├── +page.svelte        # Chat page shell
│   │   │   ├── ChatView.svelte     # Message list + input composition
│   │   │   ├── MessageList.svelte  # Scrollable list with virtual scroll
│   │   │   ├── UserBubble.svelte
│   │   │   ├── AssistantBubble.svelte
│   │   │   ├── ToolCard.svelte
│   │   │   ├── ThinkingBlock.svelte
│   │   │   ├── ChatInput.svelte
│   │   │   └── CheckpointTimeline.svelte
│   │   ├── skills/
│   │   │   ├── +page.svelte
│   │   │   ├── SkillList.svelte
│   │   │   └── SkillCard.svelte
│   │   └── settings/
│   │       └── +page.svelte
│   ├── lib/
│   │   ├── components/
│   │   │   ├── ui/                         # shadcn-svelte components (auto-generated via CLI)
│   │   │   │   ├── button/
│   │   │   │   ├── textarea/
│   │   │   │   ├── scroll-area/
│   │   │   │   ├── collapsible/
│   │   │   │   ├── sheet/
│   │   │   │   ├── alert-dialog/
│   │   │   │   ├── tooltip/
│   │   │   │   ├── slider/
│   │   │   │   ├── sonner/
│   │   │   │   ├── avatar/
│   │   │   │   ├── tabs/
│   │   │   │   ├── checkbox/
│   │   │   │   ├── dropdown-menu/
│   │   │   │   └── ...                   # add more as needed
│   │   │   ├── layout/
│   │   │   │   ├── Layout.svelte
│   │   │   │   ├── Sidebar.svelte
│   │   │   │   ├── SessionBand.svelte      # Sidebar session switcher
│   │   │   │   ├── ExplorerTree.svelte     # File tree sidebar
│   │   │   │   ├── TabBar.svelte           # Main area tab bar
│   │   │   │   ├── BottomNav.svelte
│   │   │   │   ├── TitleBar.svelte
│   │   │   │   └── MobileHeader.svelte
│   │   │   ├── chat/
│   │   │   │   ├── ChatView.svelte
│   │   │   │   ├── MessageList.svelte
│   │   │   │   ├── UserBubble.svelte
│   │   │   │   ├── AssistantBubble.svelte
│   │   │   │   ├── ToolCard.svelte
│   │   │   │   ├── ThinkingBlock.svelte
│   │   │   │   ├── ChatInput.svelte
│   │   │   │   └── CheckpointTimeline.svelte
│   │   │   ├── explorer/
│   │   │   │   ├── FileTree.svelte         # Recursive file tree
│   │   │   │   └── FileTreeItem.svelte     # Single file/folder row
│   │   │   ├── editor/
│   │   │   │   ├── FilePreview.svelte      # Read-only Shiki viewer
│   │   │   │   ├── FileEditor.svelte       # CodeMirror 6 editor
│   │   │   │   └── EditorStatusBar.svelte  # Line/col, dirty, save
│   │   │   ├── terminal/
│   │   │   │   ├── TerminalPanel.svelte    # Bottom panel wrapper
│   │   │   │   ├── TerminalTab.svelte      # Single xterm.js instance
│   │   │   │   └── TerminalTabBar.svelte   # Multi-tab switcher
│   │   │   ├── diff/
│   │   │   │   ├── DiffPreview.svelte
│   │   │   │   ├── DiffFileTree.svelte
│   │   │   │   ├── DiffViewer.svelte
│   │   │   │   ├── SplitView.svelte
│   │   │   │   ├── UnifiedView.svelte
│   │   │   │   ├── DiffHunk.svelte
│   │   │   │   ├── DiffLine.svelte
│   │   │   │   ├── IntraLineDiff.svelte
│   │   │   │   └── DiffActionBar.svelte
│   │   │   ├── MarkdownRenderer.svelte
│   │   │   └── ...                         # shadcn `Tabs`, `AlertDialog`, `Sheet`, `Sonner`, `Avatar` used inline
│   │   ├── diff/                           # Diff engine & types
│   │   │   ├── types.ts
│   │   │   ├── engine.ts
│   │   │   └── utils.ts
│   │   ├── editor/                           # Editor utilities
│   │   │   ├── cmSetup.ts                    # CodeMirror 6 configuration
│   │   │   ├── languageMap.ts                # File ext → CM6 language
│   │   │   └── highlight.ts                # Shiki async highlight utility
│   │   ├── fs/                               # FileSystemProvider abstraction
│   │   │   ├── provider.ts                   # FileSystemProvider interface
│   │   │   ├── localProvider.ts              # LocalFSProvider (Tauri fs API)
│   │   │   └── factory.ts                    # createFSProvider() based on kernel address
│   │   ├── terminal/                         # Terminal utilities
│   │   │   └── xtermTheme.ts                 # Dark/light theme tokens
│   │   ├── state.svelte.ts                   # Global reactive state (runes)
│   │   ├── api.ts                            # Tauri command wrappers + types
│   │   ├── utils.ts                          # formatters, id shortener, etc.
│   │   └── constants.ts                      # Breakpoints, timeouts
│   └── assets/
│       └── logo.svg
├── src-tauri/
│   ├── Cargo.toml                          # yomi-gui crate manifest
│   ├── tauri.conf.json
│   ├── capabilities/
│   │   └── default.json
│   └── src/
│       ├── main.rs
│       ├── lib.rs
│       ├── state.rs                        # AppState: Arc<Mutex<RemoteCoordinator>>
│       ├── terminal/                       # PTY management
│       │   ├── mod.rs
│       │   ├── session.rs                  # TerminalSession struct
│       │   └── manager.rs                  # Multi-tab terminal manager
│       └── commands/                       # thin Tauri wrappers around CoordinatorApi
│           ├── mod.rs
│           ├── session.rs
│           ├── chat.rs
│           ├── checkpoint.rs
│           ├── skill.rs
│           ├── system.rs
│           └── terminal.rs                 # terminal_spawn, write, resize, kill
├── index.html
├── vite.config.ts
├── svelte.config.js
├── tsconfig.json
└── package.json
```

---
│       │   ├── skill.rs
│       │   ├── system.rs
│       │   └── terminal.rs                 # terminal_spawn, write, resize, kill
│       └── state.rs                        # AppState: Arc<Mutex<RemoteCoordinator>>
├── static/
│   └── favicon.png
├── vite.config.ts
├── svelte.config.js
├── tsconfig.json
└── package.json
```

---

## shadcn-svelte Component Setup

After scaffolding the project and running `npx shadcn-svelte@latest init`, install the components needed for this design:

```bash
# Layout & Navigation
npx shadcn-svelte@latest add scroll-area
npx shadcn-svelte@latest add collapsible
npx shadcn-svelte@latest add tooltip

# Forms & Input
npx shadcn-svelte@latest add button
npx shadcn-svelte@latest add textarea
npx shadcn-svelte@latest add input
npx shadcn-svelte@latest add switch
npx shadcn-svelte@latest add select
npx shadcn-svelte@latest add slider

# Dialogs & Overlays
npx shadcn-svelte@latest add sheet
npx shadcn-svelte@latest add alert-dialog
npx shadcn-svelte@latest add dialog
npx shadcn-svelte@latest add dropdown-menu

# Feedback
npx shadcn-svelte@latest add sonner
npx shadcn-svelte@latest add badge

# Data Display
npx shadcn-svelte@latest add avatar
npx shadcn-svelte@latest add checkbox
```

**Additional frontend dependencies:**

```bash
# Diff computation (Google's diff algorithm for intra-line highlighting)
npm install diff-match-patch
npm install -D @types/diff-match-patch

# Fast line-level diff (lighter alternative / companion)
npm install fast-diff

# Code Editor (CodeMirror 6)
npm install codemirror @codemirror/theme-one-dark
# Language packs (load on demand)
npm install @codemirror/lang-rust @codemirror/lang-javascript @codemirror/lang-typescript @codemirror/lang-python @codemirror/lang-json @codemirror/lang-markdown

# Terminal (xterm.js)
npm install @xterm/xterm @xterm/addon-fit

# Shiki (syntax highlighting for preview + diff)
npm install shiki
```

**Additional Rust dependencies** (`crates/gui/src-tauri/Cargo.toml`):

```toml
[dependencies]
# ... existing deps ...
portable-pty = "0.8"          # Cross-platform PTY for embedded terminal
tokio = { version = "1", features = ["process", "io-util"] }
```

---

## Implementation Phases

### Phase 1: Bridge & Chat (Week 1-2)
- [ ] Tauri v2 project scaffold with Svelte 5 + Tailwind v4
- [ ] Rust `RemoteCoordinator` connection: reuse `kernel::client` directly
- [ ] Frontend `api.ts`: type-safe wrappers around all Tauri commands
- [ ] Global event listener: route `kernel:event` directly into reactive state
- [ ] **Chat page**: MessageList, ChatInput, basic AssistantBubble
- [ ] **Session sidebar**: list, create, switch
- [ ] Dark mode + responsive layout shell

**Success criteria**: Can create session, send message, receive streaming response, see tool cards.

### Phase 2: Explorer, Editor & Sidebar Session Switcher (Week 3)
- [ ] **Sidebar SessionBand**: vertical session list with status dots, unread badges, drag reorder
- [ ] **File Explorer**: recursive file tree via `LocalFSProvider` (wraps Tauri `fs`), gitignore awareness, fuzzy filter
- [ ] **File Preview**: read-only Shiki viewer with breadcrumbs, "Edit" / "Ask AI" actions
- [ ] **File Editor**: CodeMirror 6 embedded editor with syntax highlighting, Ctrl+S save, dirty indicator
- [ ] **Main area tabs**: Chat (pinned) + dynamic file preview/editor tabs
- [ ] Mobile adaptation: session chips, Files tab, editor touch targets

**Success criteria**: Can browse project files, preview code, make edits, and save directly. Session switching is one click in sidebar.

### Phase 3: Terminal & Diff Preview (Week 4)
- [ ] **Integrated Terminal**: xterm.js + Rust `portable-pty`, bottom drag-resize panel, multi-tabs
- [ ] Terminal-AI integration: "Run in Terminal" buttons, error explain, working directory sync
- [ ] **Diff Preview v1**: Unified view, basic hunks, syntax highlight via Shiki
- [ ] **Diff Preview v2**: Split view, intra-line highlighting (`diff-match-patch`), partial apply
- [ ] Partial-apply integration: send `updated_input` back to kernel
- [ ] Fork / Restore / Delete session flows

**Success criteria**: Can open real shell in GUI, run AI-suggested commands, see live output. Can preview diffs before write/edit and cherry-pick hunks to apply.

### Phase 4: Checkpoints, Skills & Polish (Week 5)
- [ ] Checkpoint timeline visualization in chat
- [ ] Rewind flow (with confirmation)
- [ ] Skill manager page
- [ ] Config reload integration
- [ ] Settings page with persistence
- [ ] **Diff Preview v3**: Multi-file batch diff, keyboard shortcuts (`j`/`k`/`y`/`n`), animations
- [ ] Shiki code highlighting with theme sync
- [ ] Copy-to-clipboard on code blocks, diff lines, and terminal output
- [ ] Diff search: `Ctrl+F` within diff preview
- [ ] Drag-and-drop files into chat (images → base64 blocks)
- [ ] Desktop notifications via Tauri `notification` plugin
- [ ] CI builds for macOS, Windows, Linux, iOS, Android

**Success criteria**: All features work on desktop and mobile. Diff preview feels like a native Git GUI. Terminal handles real shell workflows. Builds pass on all targets.

---

## Key Design Decisions

1. **No kernel changes**: The GUI is purely a new client. All feature requests that need kernel support (e.g., runtime skill toggling) are deferred or shimmed in the GUI layer.

2. **Single session subscription at a time**: The GUI only `Subscribe`s to the active session to keep daemon load minimal. Switching sessions unsubscribes the old one.

3. **Local aliases**: Session renaming is stored in Tauri's local store (`~/.yomi/gui-state.json`) because the kernel session store doesn't have display names. This keeps kernel changes zero.

4. **Tailwind v4 with shadcn theme**: shadcn-svelte's theming relies on CSS variables. We configure the color tokens in `app.css` using `@theme` (Tailwind v4) so that shadcn components and our custom components share one theme source. No separate `tailwind.config.js` needed.

5. **shadcn-svelte over raw Bits UI**: shadcn-svelte gives us beautifully designed, accessible, copy-paste components out of the box while retaining full Tailwind customization. It sits on top of Bits UI primitives, so we get accessibility + sensible defaults + zero styling fights. We add components via `npx shadcn-svelte@latest add <component>` and tweak them in `src/lib/components/ui/`.

6. **GUI is an isolated workspace crate**: `crates/gui/` is a new workspace member. Its `src-tauri/Cargo.toml` depends on `yomi-kernel` as a path dependency. The `kernel`, `cli`, and `tui` crates remain completely untouched. This eliminates protocol drift — when Wire Protocol bumps, GUI breaks at compile time, not runtime.

7. **Local-first via FileSystemProvider abstraction**: All file system operations (read, write, list, stat) go through a `FileSystemProvider` interface. The current implementation is `LocalFSProvider` which uses Tauri native `fs` APIs for zero-latency editing. A future `KernelFSProvider` (using kernel RPC) can be swapped in for remote daemon scenarios without touching frontend components.

8. **Terminal is a real PTY, not simulation**: `portable-pty` spawns the user's actual shell (`bash`/`zsh`/`powershell`). This ensures 100% compatibility with shell aliases, interactive programs (`git log`, `htop`), and color output. xterm.js handles all ANSI sequences natively. For remote scenarios, terminal would connect via SSH directly rather than tunneling through kernel.

9. **Main area is tabbed, not page-based**: Unlike traditional SPAs with route-based pages, the main workspace uses tabs. Chat is always pinned; files and terminals open as dynamic tabs. This matches IDE mental models and preserves context when switching tasks.

---

## Appendix: Wire Protocol Reuse

The `yomi-kernel` crate already exports a production-grade IPC client (`RemoteCoordinator` in `kernel::client`). The GUI Rust layer reuses it directly — no new Wire Protocol client is written.

**What the GUI gets for free** (from `RemoteCoordinator`):

| Feature | How it works |
|---|---|
| **Lazy connect + retry** | First API call triggers connect; retries for 10s if daemon not ready |
| **Heartbeat** | Auto ping/pong every 2s; disconnects after 6s without response |
| **Auto-reconnect** | Connection drop → reconnects and re-subscribes active sessions |
| **Request ID tracking** | `RequestIdGenerator` ensures every RPC has a unique id |
| **Event routing** | `broadcast::Sender<Event>` per session; survives reconnects |
| **Protocol handshake** | `Hello` on connect checks Wire Protocol version |

**GUI Tauri commands** are thin wrappers:

```rust
use tauri::State;
use yomi_kernel::client::CoordinatorApi;

#[tauri::command]
pub async fn list_sessions(state: State<'_, AppState>) -> Result<Vec<SessionInfo>, String> {
    let guard = state.coordinator.lock().await;
    guard.list_sessions(Default::default()).await.map_err(|e| e.to_string())
}
```

The only "new" Rust code in the GUI crate is:
- **`state.rs`**: holds `Arc<Mutex<RemoteCoordinator>>` in Tauri managed state
- **`commands/`**: Tauri command wrappers that call `CoordinatorApi` methods
- **`terminal/`**: `portable-pty` integration (not in kernel crate, legitimately new)
- **Event bridge**: converts `broadcast::Receiver<Event>` → Tauri `Emitter` for frontend push

No Wire Protocol framing, no frame parsing, no heartbeat logic, no reconnect handling — all inherited from `yomi-kernel::client::RemoteCoordinator`.

The event-forwarding channel uses `tokio::mpsc` internally, and a Tauri-managed thread emits to the frontend via `app_handle.emit("kernel:event", payload)`.
