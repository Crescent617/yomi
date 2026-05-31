<script lang="ts">
  import { Send, Command, FileText, ChevronRight, ChevronDown, Folder, FolderOpen, File, FileCode } from "lucide-svelte";
  import * as api from "../../api";
  import { sessionState, addUserMessage, getActiveSession } from "../../state.svelte";
  import { SLASH_COMMANDS } from "../../commands";
  import { fsProvider } from "../../fs/factory";
  import type { FileEntry } from "../../fs/provider";

  let content = $state("");
  let textareaRef: HTMLTextAreaElement | null = $state(null);

  // ── command completion ──
  let showCommands = $state(false);
  let commandFilter = $state("");
  let selectedCommandIdx = $state(0);

  // ── file picker ──
  let showFilePicker = $state(false);
  let filePickerAnchor = $state(0);
  let fileEntries = $state<FileEntry[]>([]);
  let fileExpanded = $state<Set<string>>(new Set());
  let selectedFileIdx = $state(0);
  let filePickerRoot = $state("");

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
    const root = session?.projectPath || "/home/hrli";
    filePickerRoot = root;
    try {
      const list = await fsProvider.listDir(root);
      fileEntries = list.sort((a, b) => {
        if (a.isDirectory && !b.isDirectory) return -1;
        if (!a.isDirectory && b.isDirectory) return 1;
        return a.name.localeCompare(b.name);
      });
      selectedFileIdx = 0;
    } catch (e) {
      console.error("Failed to load files:", e);
      fileEntries = [];
    }
  }

  async function loadDir(path: string) {
    try {
      const list = await fsProvider.listDir(path);
      return list.sort((a, b) => {
        if (a.isDirectory && !b.isDirectory) return -1;
        if (!a.isDirectory && b.isDirectory) return 1;
        return a.name.localeCompare(b.name);
      });
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

  async function handleSubmit() {
    if (!content.trim() || !sessionState.activeSessionId) return;

    const sessionId = sessionState.activeSessionId;
    const text = content.trim();
    content = "";
    autoResize();

    addUserMessage(sessionId, text);

    try {
      await api.sendMessage(sessionId, text);
    } catch (e: any) {
      console.error("Failed to send message:", e?.message ?? e);
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    // Command picker navigation
    if (showCommands) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        selectedCommandIdx = (selectedCommandIdx + 1) % filteredCommands.length;
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
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
      e.preventDefault();
      handleSubmit();
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

  function toggleDir(path: string) {
    const next = new Set(fileExpanded);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    fileExpanded = next;
  }
</script>

<div class="border-t border-border p-3 relative">
  <!-- Command completion dropdown -->
  {#if showCommands && filteredCommands.length > 0}
    <div class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-48 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50">
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
    <div class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-56 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50">
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
      placeholder="Ask anything... (Shift+Enter newline, /command, @file)"
      rows={1}
      class="flex-1 resize-none rounded-lg border border-input bg-background px-3 py-2 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:cursor-not-allowed disabled:opacity-50"
    ></textarea>
    <button
      onclick={handleSubmit}
      disabled={!content.trim() || !sessionState.activeSessionId}
      class="inline-flex items-center justify-center rounded-lg bg-primary text-primary-foreground h-9 w-9 hover:bg-primary/90 disabled:opacity-50 shrink-0"
    >
      <Send size={16} />
    </button>
  </div>
</div>