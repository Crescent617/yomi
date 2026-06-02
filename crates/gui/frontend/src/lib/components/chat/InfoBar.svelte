<script lang="ts">
  import { Loader2, CheckCircle2, Info, AlertTriangle, XCircle, Check } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import { uiState, getDisplayMessages } from "../../state.svelte";
  import { formatElapsed, formatTokens } from "../../utils";
  import * as api from "../../api";
  import { onMount } from "svelte";

  let { session }: { session: SessionState | null } = $props();

  const displayMessages = $derived(getDisplayMessages(session?.id ?? ""));

  let config = $state<{ model: string; context_window: number } | null>(null);

  onMount(() => {
    api.getConfig().then(c => config = c).catch(() => {});
  });

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

  // ── Total token estimate: all messages ──
  const totalTokens = $derived.by(() => {
    if (!session) return 0;
    let chars = 0;
    for (const msg of displayMessages) {
      chars += msg.content.length;
      if (msg.thinking) chars += msg.thinking.content.length;
      for (const tool of msg.tools ?? []) {
        chars += (tool.arguments ?? "").length;
        chars += (tool.result ?? "").length;
      }
    }
    return Math.round(chars / 4);
  });

  // ── Token estimate: last assistant message only ──
  const streamingTokens = $derived.by(() => {
    if (!session) return 0;
    let lastAssistant: typeof displayMessages[0] | null = null;
    for (let i = displayMessages.length - 1; i >= 0; i--) {
      if (displayMessages[i].role === "assistant") {
        lastAssistant = displayMessages[i];
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
    for (let i = displayMessages.length - 1; i >= 0; i--) {
      const msg = displayMessages[i];
      if (msg.role === "assistant" && msg.tools) {
        for (const tool of msg.tools) {
          if (tool.status === "running") return tool;
        }
      }
    }
    return null;
  });
</script>

{#if session?.streaming || streamingTokens > 0 || uiState.notification || totalTokens > 0}
  <div class="flex items-center justify-between px-3 py-1 text-xs border-b border-border bg-muted/30 min-h-[28px] font-mono">
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

    <!-- Right: model + ctx + notification -->
    <div class="flex items-center gap-2 shrink-0">
      {#if config}
        <span class="text-muted-foreground/60">{config.model}</span>
        <span class="text-muted-foreground/40">·</span>
        <span class="text-muted-foreground/60" class:text-amber-500={totalTokens > config.context_window * 0.8}>
          {formatTokens(totalTokens)} / {formatTokens(config.context_window)}
        </span>
      {/if}

      {#if uiState.notification}
        <div class="flex items-center gap-1">
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
  </div>
{/if}
