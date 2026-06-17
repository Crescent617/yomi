<script lang="ts">
  import { Loader2, CheckCircle2, Database, Zap } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import { getDisplayMessages } from "../../state.svelte";
  import { formatElapsed, formatTokens, utf8ByteLength } from "../../utils";
  import * as api from "../../api";
  import { onMount } from "svelte";

  let { session }: { session: SessionState | null } = $props();

  const displayMessages = $derived(getDisplayMessages(session?.id ?? ""));

  let config = $state<{ model: string; context_window: number } | null>(null);

  onMount(() => {
    api
      .getConfig()
      .then((c) => (config = c))
      .catch(() => {});
  });

  // ── Timer ──
  let startTime = $state<number | null>(null);
  let elapsed_ms = $state(0);
  let timerInterval: ReturnType<typeof setInterval> | null = null;

  const isRunning = $derived.by(() => {
    if (!session) return false;
    return session.phase !== "idle" && session.phase !== "closed";
  });

  $effect(() => {
    if (isRunning) {
      startTime = Date.now();
      elapsed_ms = 0;
      timerInterval = setInterval(() => {
        if (startTime) elapsed_ms = Date.now() - startTime;
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
      elapsed_ms = 0;
    }
  });

  // ── Total tokens: prefer backend real usage, fallback to estimation ──
  const total_tokens = $derived.by(() => {
    if (!session) return 0;
    // Use backend-reported token usage (aligns with TUI status bar)
    if (session.token_usage?.total_tokens != null) {
      return session.token_usage.total_tokens;
    }
    // Fallback to client-side estimation
    let bytes = 0;
    for (const msg of displayMessages) {
      bytes += utf8ByteLength(msg.content);
      if (msg.thinking) bytes += utf8ByteLength(msg.thinking.content);
      for (const tool of msg.tools ?? []) {
        bytes += utf8ByteLength(tool.arguments ?? "");
        bytes += utf8ByteLength(tool.output ?? "");
      }
    }
    return Math.round(bytes / 4);
  });

  // ── Token estimate: last assistant message only ──
  const streamingTokens = $derived.by(() => {
    if (!session) return 0;
    let lastAssistant: (typeof displayMessages)[0] | null = null;
    for (let i = displayMessages.length - 1; i >= 0; i--) {
      if (displayMessages[i].role === "assistant") {
        lastAssistant = displayMessages[i];
        break;
      }
    }
    if (!lastAssistant) return 0;
    let bytes = utf8ByteLength(lastAssistant.content);
    if (lastAssistant.thinking) {
      bytes += utf8ByteLength(lastAssistant.thinking.content);
    }
    for (const tool of lastAssistant.tools ?? []) {
      bytes += utf8ByteLength(tool.arguments ?? "");
    }
    return Math.round(bytes / 4);
  });

  // ── Current running tool ──
  const currentTool = $derived.by(() => {
    if (session?.phase !== "streaming" && session?.phase !== "executing_tool")
      return null;

    // Find latest assistant message with content/thinking
    // If model is already outputting text/thinking, don't show "calling"
    let latestOutputMsg: (typeof displayMessages)[0] | null = null;
    for (let i = displayMessages.length - 1; i >= 0; i--) {
      const msg = displayMessages[i];
      if (msg.role === "assistant") {
        if (msg.content || msg.thinking) {
          latestOutputMsg = msg;
          break;
        }
      }
    }

    // If latest output message has content/thinking, suppress calling indicator
    if (
      latestOutputMsg &&
      (latestOutputMsg.content || latestOutputMsg.thinking)
    ) {
      // But still check if it's the *same* message that contains the running tool
      // Only suppress if the latest message is a pure output (no running tools in it)
      const hasRunningToolInLatest = latestOutputMsg.tools?.some(
        (t) => t.status === "running",
      );
      if (!hasRunningToolInLatest) return null;
    }

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

{#if isRunning || streamingTokens > 0 || total_tokens > 0}
  <div
    class="flex items-center justify-between px-3 py-1 text-xs border-b border-border bg-muted/30 min-h-7 font-mono"
  >
    <!-- Left: phase status -->
    <div class="flex items-center gap-1.5 min-w-0">
      {#if session?.phase === "streaming"}
        <Loader2 size={12} class="animate-spin text-primary shrink-0" />
      {:else if session?.phase === "executing_tool"}
        <Zap size={12} class="animate-pulse text-amber-500 shrink-0" />
      {:else if session?.phase === "compacting"}
        <Database size={12} class="animate-spin text-amber-500 shrink-0" />
      {:else if streamingTokens > 0}
        <CheckCircle2 size={12} class="text-green-500 shrink-0" />
      {/if}

      {#if streamingTokens > 0}
        <span class="text-muted-foreground shrink-0"
          >{formatTokens(streamingTokens)} tokens</span
        >
      {/if}

      {#if isRunning && elapsed_ms > 0}
        <span class="text-muted-foreground/70 shrink-0"
          >· {formatElapsed(elapsed_ms)}</span
        >
      {/if}

      {#if currentTool}
        <span class="text-muted-foreground/70 truncate"
          >· calling {currentTool.tool_name}</span
        >
        {#if currentTool.progress}
          <span class="text-muted-foreground/50 truncate max-w-60"
            >· {currentTool.progress}</span
          >
        {/if}
      {:else if session?.phase === "streaming"}
        <span class="text-muted-foreground/70 shrink-0">· generating</span>
      {:else if session?.phase === "compacting"}
        <span class="text-muted-foreground/70 shrink-0">· compacting</span>
      {/if}
    </div>

    <!-- Right: model + ctx -->
    <div class="flex items-center gap-2 shrink-0">
      {#if config}
        {@const pct = (total_tokens / config.context_window) * 100}
        <span class="text-muted-foreground/60">{config.model}</span>
        <span class="text-muted-foreground/40">·</span>
        <span
          class="text-muted-foreground/60"
          class:text-amber-500={pct >= 70}
          class:text-red-500={pct >= 90}
        >
          {pct.toFixed(1)}% ({(config.context_window / 1000).toFixed(0)}K)
        </span>
      {/if}
    </div>
  </div>
{/if}
