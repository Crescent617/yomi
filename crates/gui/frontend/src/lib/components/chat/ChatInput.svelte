<script lang="ts">
import { Send, Command, FileText, ChevronRight, ChevronDown, Folder, FolderOpen, File, FileCode, Loader2, Square, Clock, Paperclip, Image, X } from "lucide-svelte";
import { levelDescription, levelIcon, levelColor, type PermissionLevel } from "../../permission";
import { SvelteSet } from "svelte/reactivity";
import * as api from "../../api";
import { sessionState, getActiveSession, showNotification, loadSessionMessages } from "../../state.svelte";
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
let fileExpanded = new SvelteSet<string>();
let selectedFileIdx = $state(0);
let filePickerRoot = $state("");
let fileListRef: HTMLDivElement | null = $state(null);

// ── history picker ──
let showHistory = $state(false);
let selectedHistoryIdx = $state(0);
let historyListRef: HTMLDivElement | null = $state(null);
let prevSessionId = $state<string | null>(null);

// ── image attachments ──
let imagePlaceholders = $state<Map<string, string>>(new Map());
let imageCounter = $state(0);
let fileInputRef: HTMLInputElement | null = $state(null);

const activeSession = $derived(getActiveSession());
const isStreaming = $derived(activeSession?.streaming ?? false);

// detect completion triggers
function detectCompletion() {
  if (!textareaRef) return;
  if (showHistory) return; // Don't interfere with history search
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

const historyEntries = $derived.by(() => {
  if (!showHistory) return [];
  const session = activeSession;
  if (!session) return [];
  const userMsgs = session.messages
    .filter(m => m.role === "user")
    .map(m => m.content)
    .filter(c => c.trim());
  // Deduplicate and reverse (newest first)
  const seen = new Set<string>();
  const unique: string[] = [];
  for (let i = userMsgs.length - 1; i >= 0; i--) {
    const c = userMsgs[i].trim();
    if (!seen.has(c)) {
      seen.add(c);
      unique.push(c);
    }
  }
  const q = content.toLowerCase();
  if (!q) return unique;
  return unique.filter(c => c.toLowerCase().includes(q));
});

// ── actions ──
export function setContent(text: string) {
  content = text;
  clearImages();
  requestAnimationFrame(autoResize);
}

function queueInput() {
  const session = activeSession;
  if (!session || !content.trim()) return;
  session.queuedInput = content.trim();
  content = "";
  clearImages();
  autoResize();
}

function acceptCommand(cmd: string) {
  if (cmd === "/history") {
    showHistory = true;
    selectedHistoryIdx = 0;
    content = "";
    showCommands = false;
    textareaRef?.focus();
    requestAnimationFrame(autoResize);
    return;
  }
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
      case "/undo":
        {
          const checkpoints = await api.getCheckpoints(sessionId) as any[];
          if (!Array.isArray(checkpoints) || checkpoints.length < 2) {
            showNotification("No checkpoint to undo", "error", 3000);
            return;
          }
          const sorted = [...checkpoints].sort((a: any, b: any) => (a.sequence ?? 0) - (b.sequence ?? 0));
          const target = sorted[sorted.length - 2] as any;
          if (!target?.message_id) {
            showNotification("No checkpoint to undo", "error", 3000);
            return;
          }
          await api.rewind(sessionId, target.message_id as string);
          showNotification("Undo last turn", "info", 3000);
        }
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
        showNotification("Session compaction requested", "info", 3000);
        // Reload messages after a short delay to reflect compacted state
        setTimeout(() => {
          api.getMessages(sessionId).then((msgs) => {
            loadSessionMessages(sessionId, msgs);
          }).catch((e: Error) => console.error("Failed to reload after compact:", e));
        }, 2000);
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
      case "/history":
        {
          showHistory = true;
          selectedHistoryIdx = 0;
          content = "";
          autoResize();
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
  } else if (imagePlaceholders.size > 0) {
    // Message with images: build content blocks
    try {
      const blocks = buildContentBlocks(text);
      await api.sendMessageBlocks(sessionId, blocks);
      clearImages();
    } catch (e: any) {
      console.error("Failed to send message with images:", e?.message ?? e);
      showNotification("Failed to send message", "error", 3000);
    }
  } else {
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

async function handlePermissionSet(level: string) {
  const sessionId = sessionState.activeSessionId;
  if (!sessionId) return;
  const session = getSession(sessionId);
  if (!session) return;
  try {
    await api.setPermissionLevel(sessionId, level);
    session.permissionLevel = level;
    showNotification(`Permission level: ${level}`, "info", 2000);
  } catch (e: any) {
    console.error("Failed to set permission level:", e?.message ?? e);
    showNotification("Failed to set permission level", "error", 3000);
  }
}

// ── image helpers ──

function insertImagePlaceholder(base64Url: string): string {
  imageCounter += 1;
  const placeholder = `[Image #${imageCounter}]`;
  imagePlaceholders.set(placeholder, base64Url);

  // Insert at cursor position
  const cursorPos = textareaRef?.selectionStart ?? content.length;
  const before = content.slice(0, cursorPos);
  const after = content.slice(cursorPos);
  const prefix = before.length > 0 && !before.endsWith(" ") ? " " : "";
  const suffix = after.length > 0 && !after.startsWith(" ") ? " " : "";
  content = before + prefix + placeholder + suffix + after;
  requestAnimationFrame(autoResize);
  return placeholder;
}

async function readFileAsBase64(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => {
      const result = reader.result as string;
      resolve(result);
    };
    reader.onerror = reject;
    reader.readAsDataURL(file);
  });
}

async function handleImageFile(file: File) {
  if (!file.type.startsWith("image/")) {
    showNotification("Only image files are supported", "error", 3000);
    return;
  }
  try {
    const base64Url = await readFileAsBase64(file);
    insertImagePlaceholder(base64Url);
    textareaRef?.focus();
  } catch (e) {
    console.error("Failed to read image:", e);
    showNotification("Failed to read image", "error", 3000);
  }
}

function handleFileSelect(e: Event) {
  const input = e.target as HTMLInputElement;
  const files = input.files;
  if (!files) return;
  for (const file of files) {
    handleImageFile(file);
  }
  input.value = ""; // reset so same file can be selected again
}

async function handlePaste(e: ClipboardEvent) {
  const items = e.clipboardData?.items;
  if (!items) return;

  let hasImage = false;
  for (const item of items) {
    if (item.type.startsWith("image/")) {
      hasImage = true;
      e.preventDefault();
      const file = item.getAsFile();
      if (file) {
        await handleImageFile(file);
      }
    }
  }
}

function removeImagePlaceholder(placeholder: string) {
  imagePlaceholders.delete(placeholder);
  content = content.replace(new RegExp(`\\s?${placeholder.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}\\s?`, "g"), " ");
  content = content.replace(/\s{2,}/g, " ").trim();
  requestAnimationFrame(autoResize);
}

function buildContentBlocks(text: string): unknown[] {
  const blocks: unknown[] = [];
  let currentText = "";
  let remaining = text;

  while (remaining.length > 0) {
    let earliestIdx = -1;
    let earliestPlaceholder = "";
    for (const [ph] of imagePlaceholders) {
      const idx = remaining.indexOf(ph);
      if (idx >= 0 && (earliestIdx === -1 || idx < earliestIdx)) {
        earliestIdx = idx;
        earliestPlaceholder = ph;
      }
    }

    if (earliestIdx === -1) {
      currentText += remaining;
      break;
    }

    currentText += remaining.slice(0, earliestIdx);
    remaining = remaining.slice(earliestIdx + earliestPlaceholder.length);

    // Flush text block
    const trimmed = currentText.trim();
    if (trimmed) {
      blocks.push({ type: "text", text: trimmed });
    }
    currentText = "";

    // Add image block
    const base64Url = imagePlaceholders.get(earliestPlaceholder);
    if (base64Url) {
      blocks.push({
        type: "image_url",
        image_url: { url: base64Url, detail: "auto" },
      });
    }
  }

  const trimmed = currentText.trim();
  if (trimmed) {
    blocks.push({ type: "text", text: trimmed });
  }

  // If no blocks at all, return a single text block
  if (blocks.length === 0) {
    blocks.push({ type: "text", text: text.trim() });
  }

  return blocks;
}

function clearImages() {
  imagePlaceholders = new Map();
  imageCounter = 0;
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
          if (fileExpanded.has(entry.path)) fileExpanded.delete(entry.path);
          else fileExpanded.add(entry.path);
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

  // History picker navigation
  if (showHistory) {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      if (historyEntries.length === 0) return;
      selectedHistoryIdx = (selectedHistoryIdx + 1) % historyEntries.length;
      return;
    }
    if (e.key === "ArrowUp") {
      e.preventDefault();
      if (historyEntries.length === 0) return;
      selectedHistoryIdx = (selectedHistoryIdx - 1 + historyEntries.length) % historyEntries.length;
      return;
    }
    if (e.key === "Tab" || e.key === "Enter") {
      e.preventDefault();
      const entry = historyEntries[selectedHistoryIdx];
      if (entry) {
        content = entry;
        showHistory = false;
        textareaRef?.focus();
        requestAnimationFrame(autoResize);
      } else {
        showHistory = false;
      }
      return;
    }
    if (e.key === "Escape") {
      showHistory = false;
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
    const text = content.trim();
    if (text === "/history" || text.startsWith("/history ")) {
      showHistory = true;
      selectedHistoryIdx = 0;
      content = "";
      autoResize();
      return;
    }
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

$effect(() => {
  if (showHistory && historyListRef) {
    const buttons = historyListRef.querySelectorAll("button");
    const selected = buttons[selectedHistoryIdx];
    if (selected) {
      selected.scrollIntoView({ block: "nearest", inline: "nearest" });
    }
  }
});

$effect(() => {
  const currentId = activeSession?.id ?? null;
  if (prevSessionId !== currentId) {
    showHistory = false;
    prevSessionId = currentId;
  }
});

function toggleDir(path: string) {
  if (fileExpanded.has(path)) fileExpanded.delete(path);
  else fileExpanded.add(path);
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
          {@const Icon = getFileIcon(entry)}
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
            <Icon
              size={14}
              class="shrink-0 {entry.isDirectory ? 'text-primary' : 'text-muted-foreground'}"
            />
            <span class="truncate">{entry.name}</span>
          </button>

          <!-- Nested dir contents (simplified: only one level shown) -->
          {#if entry.isDirectory && fileExpanded.has(entry.path)}
            {#await loadDir(entry.path) then children}
              {#each children as child (child.path)}
                {@const ChildIcon = child.isDirectory ? Folder : File}
                <button
                  class="flex items-center gap-2 w-full pl-8 pr-3 py-1 text-left text-xs transition-colors hover:bg-secondary/50"
                  onclick={() => {
                    if (!child.isDirectory) acceptFile(child.path);
                  }}
                >
                  <ChildIcon
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

  <!-- History picker -->
  {#if showHistory}
    <div bind:this={historyListRef} class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-48 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50">
      {#if historyEntries.length === 0}
        <div class="px-3 py-4 text-sm text-muted-foreground text-center">No matching history</div>
      {:else}
        {#each historyEntries as entry, i (entry)}
          <button
            class="flex items-start gap-2 w-full px-3 py-2 text-left text-sm transition-colors {i === selectedHistoryIdx ? 'bg-secondary' : 'hover:bg-secondary/50'}"
            onclick={() => { content = entry; showHistory = false; textareaRef?.focus(); requestAnimationFrame(autoResize); }}
            title={entry}
          >
            <Clock size={14} class="text-muted-foreground shrink-0 mt-0.5" />
            <span class="truncate">{entry.length > 80 ? entry.slice(0, 80) + "..." : entry}</span>
          </button>
        {/each}
      {/if}
    </div>
  {/if}

  <div class="flex items-end gap-2">
    <!-- Hidden file input -->
    <input
      type="file"
      accept="image/*"
      multiple
      bind:this={fileInputRef}
      onchange={handleFileSelect}
      class="hidden"
    />
    <!-- Attach button -->
    <button
      type="button"
      onclick={() => fileInputRef?.click()}
      class="inline-flex items-center justify-center rounded-lg border border-border text-muted-foreground h-9 w-9 hover:bg-secondary hover:text-foreground shrink-0 transition-colors"
      title="Attach image"
    >
      <Paperclip size={16} />
    </button>
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
      onpaste={handlePaste}
      placeholder={isStreaming ? "Press Enter to queue next message..." : "Ask anything... (Shift+Enter newline, /command, @file, paste image)"}
      rows={1}
      class="flex-1 resize-none rounded-lg bg-background px-3 py-2 text-sm placeholder:text-muted-foreground focus-visible:outline-none min-h-[40px] max-h-[200px]"
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
  {#if activeSession}
    <!-- Image attachments -->
    {#if imagePlaceholders.size > 0}
      <div class="flex items-center gap-2 mt-1.5 px-1 flex-wrap">
        {#each [...imagePlaceholders.entries()] as [placeholder, base64Url] (placeholder)}
          <div class="flex items-center gap-1.5 rounded-lg border border-border bg-secondary px-2 py-1">
            <img src={base64Url} alt="" class="w-6 h-6 rounded object-cover" />
            <span class="text-xs text-muted-foreground">{placeholder}</span>
            <button
              type="button"
              onclick={() => removeImagePlaceholder(placeholder)}
              class="text-muted-foreground hover:text-destructive transition-colors"
              title="Remove"
            >
              <X size={12} />
            </button>
          </div>
        {/each}
      </div>
    {/if}
    <div class="flex items-center mt-1.5 px-1">
      <!-- Permission level -->
      <div class="flex items-center gap-1">
        {#each (["safe", "caution", "dangerous"] as PermissionLevel[]) as level (level)}
          {@const Icon = levelIcon(level)}
          <button
            type="button"
            onclick={() => handlePermissionSet(level)}
            class="p-1 rounded transition-colors {activeSession.permissionLevel === level ? levelColor(level) : 'text-muted-foreground hover:text-foreground'}"
            title={levelDescription(level)}
          >
            <Icon class="w-4 h-4" />
          </button>
        {/each}
      </div>
    </div>
  {/if}
</div>
