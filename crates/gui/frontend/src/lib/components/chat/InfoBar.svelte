<script lang="ts">
  import { Loader2, CheckCircle2, Info, AlertTriangle, XCircle, Check } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import { uiState } from "../../state.svelte";

  let { session }: { session: SessionState | null } = $props();

  // ── Timer ──
  let startTime = $state<number | null>(null);
  let elapsedMs = $state(0);
  let timerInterval: ReturnType<typeof setInterval> | null = null;

  $effect(() => {
    if (session?.streaming) {
      startTime = Date.now();
      elapsedMs = 0;
      timerInterval = setInterval(() => {
        if (startTime) elapsedMs = Date.now() - startTime;
      }, 100);
      return () => {
        if (timerInterval) {
          clearInterval(timerInterval);
          timerInterval = null;
        }
        startTime = null;
      };
    } else {
      if (timerInterval) {
        clearInterval(timerInterval);
        timerInterval = null;
      }
      startTime = null;
    }
  });

  // ── Token estimate: last assistant message only ──
  const streamingTokens = $derived.by(() => {
    if (!session) return 0;
    let lastAssistant: typeof session.messages[0] | null = null;
    for (let i = session.messages.length - 1; i >= 0; i--) {
      if (session.messages[i].role === "assistant") {
        lastAssistant = session.messages[i];
        break;
      }
    }
    if (!lastAssistant) return 0;
    let chars = lastAssistant.content.length;
    if (lastAssistant.thinking) {
      chars += lastAssistant.thinking.content.length;
    }
    for (const tool of lastAssistant.tools ?? []) {
      chars += (tool.arguments ?? "").length;
    }
    return Math.round(chars / 4);
  });

  // ── Current running tool ──
  const currentTool = $derived.by(() => {
    if (!session?.streaming) return null;
    for (let i = session.messages.length - 1; i >= 0; i--) {
      const msg = session.messages[i];
      if (msg.role === "assistant" && msg.tools) {
        for (const tool of msg.tools) {
          if (tool.status === "running") return tool;
        }
      }
    }
    return null;
  });

  // ── Format helpers ──
  function formatElapsed(ms: number): string {
    if (ms < 1000) return `${(ms / 1000).toFixed(1)}s`;
    const mins = Math.floor(ms / 60000);
    const secs = Math.floor((ms % 60000) / 1000);
    return `${mins}m${secs.toString().padStart(2, "0")}s`;
  }

  function formatTokens(count: number): string {
    if (count >= 1000) return `~${(count / 1000).toFixed(1)}k`;
    return `~${count}`;
  }
</script>

{#if session?.streaming || streamingTokens > 0 || uiState.notification}
  <div class="flex items-center justify-between px-3 py-1 text-xs border-b border-border bg-muted/30 min-h-[28px]">
    <!-- Left: streaming status -->
    <div class="flex items-center gap-1.5 min-w-0">
      {#if session?.streaming}
        <Loader2 size={12} class="animate-spin text-primary shrink-0" />
      {:else if streamingTokens > 0}
        <CheckCircle2 size={12} class="text-green-500 shrink-0" />
      {/if}

      {#if streamingTokens > 0}
        <span class="text-muted-foreground shrink-0">{formatTokens(streamingTokens)} tokens</span>
      {/if}

      {#if session?.streaming && elapsedMs > 0}
        <span class="text-muted-foreground/70 shrink-0">· {formatElapsed(elapsedMs)}</span>
      {/if}

      {#if currentTool}
        <span class="text-muted-foreground/70 truncate">· calling {currentTool.toolName}</span>
        {#if currentTool.progress}
          <span class="text-muted-foreground/50 truncate max-w-[180px]">· {currentTool.progress}</span>
        {/if}
      {/if}
    </div>

    <!-- Right: notification -->
    {#if uiState.notification}
      <div class="flex items-center gap-1 shrink-0">
        {#if uiState.notification.level === "info"}
          <Info size={12} class="text-blue-500" />
        {:else if uiState.notification.level === "warn"}
          <AlertTriangle size={12} class="text-amber-500" />
        {:else if uiState.notification.level === "error"}
          <XCircle size={12} class="text-red-500" />
        {:else}
          <Check size={12} class="text-green-500" />
        {/if}
        <span class={uiState.notification.level === "error" ? "text-red-600" : uiState.notification.level === "warn" ? "text-amber-600" : "text-muted-foreground"}>
          {uiState.notification.text}
        </span>
      </div>
    {/if}
  </div>
{/if}
