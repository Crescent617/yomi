<script lang="ts">
import { Send, Command, FileText, ChevronRight, ChevronDown, Folder, FolderOpen, File, FileCode, Loader2, Square } from "lucide-svelte";
import * as api from "../../api";
import { sessionState, addUserMessage, getActiveSession, showNotification } from "../../state.svelte";
import { SLASH_COMMANDS } from "../../commands";
import { fsProvider } from "../../fs/factory";
import type { FileEntry } from "../../fs/provider";

let content = $state("");
let textareaRef: HTMLTextAreaElement | null = $state(null);
let composing = $state(false);
let ignoreNextEnter = $state(false);

// ── dir cache for file picker ──
let dirCache = $state<Map<string, FileEntry[]>>(new Map());

// ── command completion ──
let showCommands = $state(false);
let commandFilter = $state("");
let selectedCommandIdx = $state(0);
let commandListRef: HTMLDivElement | null = $state(null);

// ── file picker ──
let showFilePicker = $state(false);
let filePickerAnchor = $state(0);
let fileEntries = $state<FileEntry[]>([]);
let fileExpanded = $state<Set<string>>(new Set());
let selectedFileIdx = $state(0);
let filePickerRoot = $state("");
let fileListRef: HTMLDivElement | null = $state(null);

const activeSession = $derived(getActiveSession());
const isStreaming = $derived(activeSession?.streaming ?? false);

// detect completion triggers
function detectCompletion() {
  if (!textareaRef) return;
  const cursorPos = textareaRef.selectionStart;
  const beforeCursor = content.slice(0, cursorPos);

  // ── command: starts with / ──
  if (content.startsWith("/")) {
    const query = content.slice(1);
    const valid = /^[a-zA-Z0-9_\-:]*$/.test(query);
    if (valid) {
      showCommands = true;
      commandFilter = query;
      selectedCommandIdx = 0;
    } else {
      showCommands = false;
    }
  } else {
    showCommands = false;
  }

  // ── file: last @ before cursor ──
  const lastAt = beforeCursor.lastIndexOf("@");
  if (lastAt >= 0) {
    const afterAt = beforeCursor.slice(lastAt + 1);
    // @ must not be followed by a space (still typing the path)
    if (!afterAt.includes(" ")) {
      filePickerAnchor = lastAt;
      if (!showFilePicker) {
        showFilePicker = true;
        loadFilePickerRoot();
      }
    } else {
      showFilePicker = false;
    }
  } else {
    showFilePicker = false;
  }
}

async function loadFilePickerRoot() {
  const session = getActiveSession();
  const root = session?.projectPath || "";
  filePickerRoot = root;
  if (!root) {
    fileEntries = [];
    return;
  }
  try {
    const cached = dirCache.get(root);
    if (cached) {
      fileEntries = cached;
    } else {
      const list = await fsProvider.listDir(root);
      const sorted = list.sort((a, b) => {
        if (a.isDirectory && !b.isDirectory) return -1;
        if (!a.isDirectory && b.isDirectory) return 1;
        return a.name.localeCompare(b.name);
      });
      dirCache.set(root, sorted);
      fileEntries = sorted;
    }
    selectedFileIdx = 0;
  } catch (e) {
    console.error("Failed to load files:", e);
    fileEntries = [];
  }
}

async function loadDir(path: string) {
  const cached = dirCache.get(path);
  if (cached) return cached;
  try {
    const list = await fsProvider.listDir(path);
    const sorted = list.sort((a, b) => {
      if (a.isDirectory && !b.isDirectory) return -1;
      if (!a.isDirectory && b.isDirectory) return 1;
      return a.name.localeCompare(b.name);
    });
    dirCache.set(path, sorted);
    return sorted;
  } catch (e) {
    console.error("Failed to list dir:", path, e);
    return [];
  }
}

const filteredCommands = $derived.by(() => {
  const q = commandFilter.toLowerCase();
  return SLASH_COMMANDS.filter(([cmd]) =>
    cmd.toLowerCase().includes(q)
  );
});

// ── actions ──
export function setContent(text: string) {
  content = text;
  requestAnimationFrame(autoResize);
}

function queueInput() {
  const session = activeSession;
  if (!session || !content.trim()) return;
  session.queuedInput = content.trim();
  content = "";
  autoResize();
}

function acceptCommand(cmd: string) {
  content = cmd + " ";
  showCommands = false;
  textareaRef?.focus();
  requestAnimationFrame(autoResize);
}

function acceptFile(path: string) {
  const cursorPos = textareaRef?.selectionStart ?? content.length;
  const before = content.slice(0, filePickerAnchor);
  const after = content.slice(cursorPos);
  content = before + "@" + path + " " + after;
  showFilePicker = false;
  textareaRef?.focus();
  requestAnimationFrame(autoResize);
}

async function handleCommand(text: string) {
  const sessionId = sessionState.activeSessionId;
  if (!sessionId) return;

  const lower = text.toLowerCase();
  const parts = text.split(/\s+/);
  const cmd = parts[0].toLowerCase();

  try {
    switch (cmd) {
      case "/cancel":
        await api.cancelSession(sessionId);
        showNotification("Session cancelled", "info", 3000);
        break;
      case "/yolo":
        await api.setPermissionLevel(sessionId, "dangerous");
        showNotification("YOLO mode enabled — all tools will be auto-approved", "info", 5000);
        break;
      case "/safe":
        await api.setPermissionLevel(sessionId, "safe");
        showNotification("Permission level set to Safe", "info", 3000);
        break;
      case "/caution":
        await api.setPermissionLevel(sessionId, "caution");
        showNotification("Permission level set to Caution", "info", 3000);
        break;
      case "/compact":
        await api.compactSession(sessionId);
        showNotification("Session compacted", "info", 3000);
        break;
      case "/reload":
        await api.reloadConfig();
        showNotification("Skills and hooks reloaded", "info", 3000);
        break;
      case "/goal:stop":
        await api.stopGoal(sessionId);
        showNotification("Goal mode stopped", "info", 3000);
        break;
      case "/goal":
        {
          const description = parts.slice(1).join(" ").trim();
          if (!description) {
            showNotification("Please provide a goal description: /goal <description>", "error", 5000);
            return;
          }
          await api.startGoal(sessionId, description);
          showNotification("Goal mode activated — agent will work autonomously", "info", 5000);
        }
        break;
      default:
        // Unknown command — treat as normal message
        await api.sendMessage(sessionId, text);
    }
  } catch (e: any) {
    console.error(`Failed to execute command ${cmd}:`, e?.message ?? e);
    showNotification(`Command failed: ${e?.message ?? ""}`, "error", 5000);
  }
}

async function handleSubmit() {
  if (!content.trim() || !sessionState.activeSessionId) return;

  const sessionId = sessionState.activeSessionId;
  const text = content.trim();
  content = "";
  autoResize();

  if (text.startsWith("/")) {
    await handleCommand(text);
  } else {
    addUserMessage(sessionId, text);
    try {
      await api.sendMessage(sessionId, text);
    } catch (e: any) {
      console.error("Failed to send message:", e?.message ?? e);
    }
  }
}

async function handleCancel() {
  if (!sessionState.activeSessionId) return;
  try {
    await api.cancelSession(sessionState.activeSessionId);
  } catch (e: any) {
    console.error("Failed to cancel:", e?.message ?? e);
  }
}

async function handlePermissionClick() {
  const sessionId = sessionState.activeSessionId;
  if (!sessionId) return;
  const session = getSession(sessionId);
  if (!session) return;
  const levels = ["safe", "caution", "dangerous"];
  const current = levels.indexOf(session.permissionLevel ?? "safe");
  const next = levels[(current + 1) % levels.length];
  try {
    await api.setPermissionLevel(sessionId, next);
    session.permissionLevel = next;
    showNotification(`Permission level: ${next}`, "info", 2000);
  } catch (e: any) {
    console.error("Failed to set permission level:", e?.message ?? e);
    showNotification("Failed to set permission level", "error", 3000);
  }
}

function handleKeydown(e: KeyboardEvent) {
  // Ignore key events while IME is composing or right after composition ends
  if (e.isComposing || composing) {
    return;
  }

  // Command picker navigation
  if (showCommands) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (filteredCommands.length === 0) return;
      selectedCommandIdx = (selectedCommandIdx + 1) % filteredCommands.length;
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (filteredCommands.length === 0) return;
      selectedCommandIdx = (selectedCommandIdx - 1 + filteredCommands.length) % filteredCommands.length;
      return;
    }
    if (e.key === "Tab" || e.key === "Enter") {
      e.preventDefault();
      const match = filteredCommands[selectedCommandIdx];
      if (match) acceptCommand(match[0]);
      return;
    }
    if (e.key === "Escape") {
      showCommands = false;
      return;
    }
  }

  // File picker navigation
  if (showFilePicker) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      selectedFileIdx = Math.min(selectedFileIdx + 1, fileEntries.length - 1);
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      selectedFileIdx = Math.max(selectedFileIdx - 1, 0);
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const entry = fileEntries[selectedFileIdx];
      if (entry) {
        if (entry.isDirectory) {
          // toggle expand
          const next = new Set(fileExpanded);
          if (next.has(entry.path)) next.delete(entry.path);
          else next.add(entry.path);
          fileExpanded = next;
        } else {
          acceptFile(entry.path);
        }
      }
      return;
    }
    if (e.key === "Escape") {
      showFilePicker = false;
      return;
    }
  }

  // Normal input
  if (e.key === "Enter" && !e.shiftKey) {
    // If this Enter is right after IME composition ends, ignore it
    if (ignoreNextEnter) {
      ignoreNextEnter = false;
      e.preventDefault();
      return;
    }
    e.preventDefault();
    if (isStreaming) {
      queueInput();
    } else {
      handleSubmit();
    }
  }
}

function autoResize() {
  if (textareaRef) {
    textareaRef.style.height = "auto";
    textareaRef.style.height = Math.min(textareaRef.scrollHeight, 200) + "px";
  }
}

function getFileIcon(entry: FileEntry) {
  if (entry.isDirectory) return fileExpanded.has(entry.path) ? FolderOpen : Folder;
  const ext = entry.name.split(".").pop()?.toLowerCase();
  if (["rs", "js", "ts", "py", "go", "java", "c", "cpp", "h", "hpp"].includes(ext ?? "")) {
    return FileCode;
  }
  return File;
}

$effect(() => {
  if (showCommands && commandListRef) {
    const buttons = commandListRef.querySelectorAll("button");
    const selected = buttons[selectedCommandIdx];
    if (selected) {
      selected.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  }
});

$effect(() => {
  if (showFilePicker && fileListRef) {
    const buttons = fileListRef.querySelectorAll("button");
    const selected = buttons[selectedFileIdx];
    if (selected) {
      selected.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  }
});

function toggleDir(path: string) {
  const next = new Set(fileExpanded);
  if (next.has(path)) next.delete(path);
  else next.add(path);
  fileExpanded = next;
}

function getSession(sessionId: string) {
  return sessionState.sessions.find((s) => s.id === sessionId) ?? null;
}
</script>

<div class="border-t border-border p-3 relative">
  <!-- Command completion dropdown -->
  {#if showCommands && filteredCommands.length > 0}
    <div bind:this={commandListRef} class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-48 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50">
      {#each filteredCommands as [cmd, desc], i (cmd)}
        <button
          class="flex items-center gap-2 w-full px-3 py-2 text-left text-sm transition-colors {i === selectedCommandIdx ? 'bg-secondary' : 'hover:bg-secondary/50'}"
          onclick={() => acceptCommand(cmd)}
        >
          <Command size={14} class="text-muted-foreground shrink-0" />
          <span class="font-mono text-primary shrink-0">{cmd}</span>
          <span class="text-muted-foreground text-xs truncate">{desc}</span>
        </button>
      {/each}
    </div>
  {/if}

  <!-- File picker dropdown -->
  {#if showFilePicker}
    <div bind:this={fileListRef} class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-56 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50">
      <div class="px-3 py-1.5 text-xs text-muted-foreground border-b border-border flex items-center gap-1.5">
        <FileText size={12} />
        <span class="truncate">{filePickerRoot}</span>
      </div>
      {#if fileEntries.length === 0}
        <div class="px-3 py-4 text-sm text-muted-foreground text-center">No files found</div>
      {:else}
        {#each fileEntries as entry, i (entry.path)}
          <button
            class="flex items-center gap-2 w-full px-3 py-1.5 text-left text-sm transition-colors {i === selectedFileIdx ? 'bg-secondary' : 'hover:bg-secondary/50'}"
            onclick={() => {
              if (entry.isDirectory) {
                toggleDir(entry.path);
              } else {
                acceptFile(entry.path);
              }
            }}
          >
            {#if entry.isDirectory}
              {#if fileExpanded.has(entry.path)}
                <ChevronDown size={14} class="shrink-0 text-muted-foreground" />
              {:else}
                <ChevronRight size={14} class="shrink-0 text-muted-foreground" />
              {/if}
            {:else}
              <span class="w-3.5 shrink-0"></span>
            {/if}
            <svelte:component
              this={getFileIcon(entry)}
              size={14}
              class="shrink-0 {entry.isDirectory ? 'text-primary' : 'text-muted-foreground'}"
            />
            <span class="truncate">{entry.name}</span>
          </button>

          <!-- Nested dir contents (simplified: only one level shown) -->
          {#if entry.isDirectory && fileExpanded.has(entry.path)}
            {#await loadDir(entry.path) then children}
              {#each children as child (child.path)}
                <button
                  class="flex items-center gap-2 w-full pl-8 pr-3 py-1 text-left text-xs transition-colors hover:bg-secondary/50"
                  onclick={() => {
                    if (!child.isDirectory) acceptFile(child.path);
                  }}
                >
                  <svelte:component
                    this={child.isDirectory ? Folder : File}
                    size={12}
                    class="shrink-0 {child.isDirectory ? 'text-primary' : 'text-muted-foreground'}"
                  />
                  <span class="truncate {child.isDirectory ? 'text-primary' : ''}">{child.name}</span>
                </button>
              {/each}
            {/await}
          {/if}
        {/each}
      {/if}
    </div>
  {/if}

  <div class="flex items-end gap-2">
    <textarea
      bind:this={textareaRef}
      bind:value={content}
      oninput={() => { detectCompletion(); autoResize(); }}
      onkeydown={handleKeydown}
      onfocus={detectCompletion}
      onblur={() => { /* dropdowns close via item clicks or Escape */ }}
      oncompositionstart={() => composing = true}
      oncompositionend={() => {
        composing = false;
        ignoreNextEnter = true;
        setTimeout(() => ignoreNextEnter = false, 100);
      }}
      placeholder={isStreaming ? "Press Enter to queue next message..." : "Ask anything... (Shift+Enter newline, /command, @file)"}
      rows={1}
      class="flex-1 resize-none rounded-lg bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 min-h-[40px] max-h-[200px]"
    ></textarea>
    {#if isStreaming}
      <button
        type="button"
        onclick={handleCancel}
        class="inline-flex items-center justify-center rounded-lg bg-destructive text-destructive-foreground h-9 w-9 hover:bg-destructive/90 active:scale-95 transition-all shrink-0"
        title="Cancel"
      >
        <Square class="w-4 h-4 fill-current" />
      </button>
    {:else}
      <button
        type="button"
        onclick={handleSubmit}
        disabled={!content.trim() || !sessionState.activeSessionId}
        class="inline-flex items-center justify-center rounded-lg bg-primary text-primary-foreground h-9 w-9 hover:bg-primary/90 disabled:opacity-50 shrink-0"
      >
        <Send size={16} />
      </button>
    {/if}
  </div>
  {#if activeSession?.permissionLevel}
    <div class="flex items-center justify-end gap-2 mt-1.5 px-1">
      <button
        type="button"
        onclick={handlePermissionClick}
        class="text-[10px] uppercase tracking-wider font-medium rounded px-1.5 py-0.5 transition-colors
               {activeSession.permissionLevel === 'dangerous' ? 'text-red-500 bg-red-500/10 hover:bg-red-500/20'
                 : activeSession.permissionLevel === 'caution' ? 'text-amber-500 bg-amber-500/10 hover:bg-amber-500/20'
                 : 'text-green-500 bg-green-500/10 hover:bg-green-500/20'}"
        title="Click to cycle permission level"
      >
        {activeSession.permissionLevel}
      </button>
    </div>
  {/if}
</div>
