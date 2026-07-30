<script lang="ts">
  import { Bug, ChevronUp, Copy, RefreshCw, WrapText } from "lucide-svelte";
  import SidebarToggle from "./SidebarToggle.svelte";
  import { sessionState, showNotification } from "../../state.svelte";
  import {
    errorMessage,
    listGuiLogs,
    readGuiLog,
    readSessionJsonl,
    type DebugFileChunk,
  } from "../../api";
  import { formatBytes, prependEarlierContent } from "./debug-viewer";

  let { onToggleLeftPanel }: { onToggleLeftPanel?: () => void } = $props();

  type DebugSource = "session" | "logs";
  let source = $state<DebugSource>("session");
  let logs = $state<string[]>([]);
  let selectedLog = $state("");
  let content = $state("");
  let path = $state("");
  let fileSize = $state(0);
  let startOffset = $state(0);
  let endOffset = $state(0);
  let hasEarlier = $state(false);
  let loading = $state(false);
  let loadingEarlier = $state(false);
  let refreshQueued = $state(false);
  let error = $state("");
  let wrapLines = $state(false);
  let loadedKey = $state("");
  let viewer = $state<HTMLPreElement>();

  const currentKey = $derived(
    source === "session"
      ? `session:${sessionState.activeSessionId ?? ""}`
      : `log:${selectedLog}`,
  );

  $effect(() => {
    const key = currentKey;
    if (key !== loadedKey) void refresh();
  });

  async function loadLogs() {
    try {
      logs = await listGuiLogs();
      if (!logs.includes(selectedLog)) {
        selectedLog = logs[0] ?? "";
      }
    } catch (cause) {
      error = errorMessage(cause);
    }
  }

  async function readCurrent(beforeOffset?: number): Promise<DebugFileChunk> {
    if (source === "session") {
      if (!sessionState.activeSessionId) {
        return {
          content: "",
          path: "",
          file_size: 0,
          start_offset: 0,
          end_offset: 0,
          has_earlier: false,
        };
      }
      return readSessionJsonl(
        sessionState.activeSessionId,
        beforeOffset,
        undefined,
      );
    }
    if (!selectedLog) {
      return {
        content: "",
        path: "",
        file_size: 0,
        start_offset: 0,
        end_offset: 0,
        has_earlier: false,
      };
    }
    return readGuiLog(selectedLog, beforeOffset, undefined);
  }

  function applyChunk(chunk: DebugFileChunk) {
    content = chunk.content;
    path = chunk.path;
    fileSize = chunk.file_size;
    startOffset = chunk.start_offset;
    endOffset = chunk.end_offset;
    hasEarlier = chunk.has_earlier;
  }

  async function refresh() {
    if (loading) {
      refreshQueued = true;
      return;
    }
    const key = currentKey;
    loading = true;
    try {
      if (source === "logs") await loadLogs();
      if (currentKey !== key) return;
      const chunk = await readCurrent();
      if (currentKey !== key) return;
      applyChunk(chunk);
      loadedKey = key;
      error = "";
      requestAnimationFrame(() => viewer?.scrollTo(0, viewer.scrollHeight));
    } catch (cause) {
      error = errorMessage(cause);
    } finally {
      loading = false;
      if (refreshQueued) {
        refreshQueued = false;
        void refresh();
      }
    }
  }

  async function loadEarlier() {
    if (loading || loadingEarlier || !hasEarlier) return;
    const key = currentKey;
    loadingEarlier = true;
    try {
      const chunk = await readCurrent(startOffset);
      if (currentKey !== key) return;
      content = prependEarlierContent(chunk.content, content);
      startOffset = chunk.start_offset;
      hasEarlier = chunk.has_earlier;
      error = "";
    } catch (cause) {
      error = errorMessage(cause);
    } finally {
      loadingEarlier = false;
    }
  }

  async function copyContent() {
    try {
      await navigator.clipboard.writeText(content);
      showNotification("Debug content copied", "success");
    } catch (cause) {
      showNotification(`Failed to copy: ${errorMessage(cause)}`, "error");
    }
  }

  function selectSource(next: DebugSource) {
    source = next;
  }
</script>

<div class="flex h-full min-w-0 flex-1 flex-col bg-background">
  <header
    class="flex h-12 shrink-0 items-center gap-2 border-b border-border px-3"
  >
    {#if onToggleLeftPanel}
      <SidebarToggle class="lg:hidden" onclick={onToggleLeftPanel} />
    {/if}
    <Bug class="h-4 w-4 text-primary" />
    <h1 class="text-sm font-medium">Debug</h1>
    <div class="ml-3 flex rounded-md bg-secondary/60 p-0.5">
      <button
        type="button"
        class="rounded px-2.5 py-1 text-xs transition-colors {source ===
        'session'
          ? 'bg-background text-foreground shadow-sm'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => selectSource("session")}
      >
        Session JSONL
      </button>
      <button
        type="button"
        class="rounded px-2.5 py-1 text-xs transition-colors {source === 'logs'
          ? 'bg-background text-foreground shadow-sm'
          : 'text-muted-foreground hover:text-foreground'}"
        onclick={() => selectSource("logs")}
      >
        GUI Logs
      </button>
    </div>

    <div class="ml-auto flex items-center gap-1.5">
      {#if source === "logs"}
        <select
          class="max-w-48 rounded border border-border bg-background px-2 py-1 text-xs text-foreground outline-none focus:border-primary"
          aria-label="GUI log file"
          bind:value={selectedLog}
        >
          {#if logs.length === 0}
            <option value="">No GUI logs</option>
          {/if}
          {#each logs as log (log)}
            <option value={log}>{log}</option>
          {/each}
        </select>
      {/if}
      <button
        type="button"
        class="rounded p-1.5 transition-colors {wrapLines
          ? 'bg-secondary text-foreground'
          : 'text-muted-foreground hover:bg-secondary hover:text-foreground'}"
        title={wrapLines ? "Disable line wrapping" : "Enable line wrapping"}
        aria-label={wrapLines
          ? "Disable line wrapping"
          : "Enable line wrapping"}
        aria-pressed={wrapLines}
        onclick={() => (wrapLines = !wrapLines)}
      >
        <WrapText class="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        class="rounded p-1.5 text-muted-foreground hover:bg-secondary hover:text-foreground disabled:opacity-50"
        title="Copy visible content"
        aria-label="Copy visible content"
        disabled={!content}
        onclick={copyContent}
      >
        <Copy class="h-3.5 w-3.5" />
      </button>
      <button
        type="button"
        class="rounded p-1.5 text-muted-foreground hover:bg-secondary hover:text-foreground disabled:opacity-50"
        title="Refresh"
        aria-label="Refresh"
        disabled={loading}
        onclick={refresh}
      >
        <RefreshCw class="h-3.5 w-3.5 {loading ? 'animate-spin' : ''}" />
      </button>
    </div>
  </header>

  <div
    class="flex h-8 shrink-0 items-center gap-3 border-b border-border bg-card/50 px-3 font-mono text-[10px] text-muted-foreground"
  >
    <span class="min-w-0 flex-1 truncate" title={path}>
      {path ||
        (source === "session"
          ? sessionState.activeSessionId
            ? "Session history not written yet"
            : "Select a session"
          : "No GUI log file")}
    </span>
    <span>{formatBytes(fileSize)}</span>
    {#if fileSize > 0}
      <span>{startOffset}–{endOffset}</span>
    {/if}
  </div>

  {#if error}
    <div
      class="border-b border-error/30 bg-error/10 px-3 py-2 text-xs text-error"
    >
      {error}
    </div>
  {/if}

  <div class="relative min-h-0 flex-1 bg-code-bg">
    {#if hasEarlier}
      <button
        type="button"
        class="absolute left-1/2 top-2 z-10 flex -translate-x-1/2 items-center gap-1 rounded-full border border-border bg-popover px-3 py-1 text-[11px] text-popover-foreground shadow-md hover:bg-secondary disabled:opacity-50"
        disabled={loading || loadingEarlier}
        onclick={loadEarlier}
      >
        <ChevronUp class="h-3 w-3" />
        Load earlier
      </button>
    {/if}
    <pre
      bind:this={viewer}
      class="h-full overflow-auto p-4 font-mono text-[11px] leading-5 text-foreground selection:bg-primary/25 {wrapLines
        ? 'whitespace-pre-wrap break-words'
        : 'whitespace-pre'}">{content}</pre>
  </div>
</div>
