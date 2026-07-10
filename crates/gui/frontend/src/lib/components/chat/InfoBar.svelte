<script lang="ts">
  import { Loader2, Check, Database, Zap } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import {
    getDisplayMessages,
    textFromBlocks,
    hasText,
    findThinking,
  } from "../../state.svelte";
  import { formatElapsed, formatTokens, utf8ByteLength } from "../../utils";
  import * as api from "../../api";
  import type { ModelInfo } from "../../api";
  import { onMount } from "svelte";

  let { session }: { session: SessionState | null } = $props();

  let models = $state<ModelInfo[]>([]);
  let fallbackModelKey = $state<string | null>(null);

  onMount(() => {
    api
      .getModels()
      .then((res) => {
        models = res.models;
      })
      .catch(() => {});
  });

  // Sessions using the default model have model_key = null in the DB;
  // resolve the effective model via get_session_model (which applies the
  // default fallback on the backend).
  $effect(() => {
    const sid = session?.id;
    if (!sid || session?.model_key) {
      fallbackModelKey = null;
      return;
    }
    api
      .getSessionModel(sid)
      .then((key) => {
        // Discard stale responses if the session changed meanwhile
        if (session?.id === sid) fallbackModelKey = key;
      })
      .catch(() => {
        if (session?.id === sid) fallbackModelKey = null;
      });
  });

  const modelKey = $derived(session?.model_key ?? fallbackModelKey);

  const activeModel = $derived.by(() => {
    if (!modelKey) return null;
    return models.find((m) => m.name === modelKey) ?? null;
  });

  const displayMessages = $derived(getDisplayMessages(session?.id ?? ""));

  // ── Timer ──
  let startTime = $state<number | null>(null);
  let elapsed_ms = $state(0);
  let timerInterval: ReturnType<typeof setInterval> | null = null;

  const isRunning = $derived.by(() => {
    if (!session) return false;
    return session.is_running;
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
    if (session.token_usage?.total_tokens != null) {
      return session.token_usage.total_tokens;
    }
    let bytes = 0;
    for (const msg of displayMessages) {
      if (msg.type === "user" || msg.type === "assistant") {
        bytes += utf8ByteLength(textFromBlocks(msg.content));
      }
      if (msg.type === "error") {
        bytes += utf8ByteLength(msg.content);
      }
      if (msg.type === "assistant") {
        const thinking = findThinking(msg.content);
        if (thinking) {
          bytes += utf8ByteLength(thinking.content);
        }
      }
      if (msg.type === "tool") {
        bytes += utf8ByteLength(msg.arguments ?? "");
        bytes += utf8ByteLength(textFromBlocks(msg.result));
      }
    }
    return Math.round(bytes / 4);
  });

  // ── Token estimate: last assistant message only ──
  const streamingTokens = $derived.by(() => {
    if (!session) return 0;
    let lastAssistant: (typeof displayMessages)[0] | null = null;
    for (let i = displayMessages.length - 1; i >= 0; i--) {
      if (displayMessages[i].type === "assistant") {
        lastAssistant = displayMessages[i];
        break;
      }
    }
    if (!lastAssistant || lastAssistant.type !== "assistant") return 0;
    let bytes = utf8ByteLength(textFromBlocks(lastAssistant.content));
    const thinking = findThinking(lastAssistant.content);
    if (thinking) {
      bytes += utf8ByteLength(thinking.content);
    }
    if (lastAssistant.tool_calls) {
      for (const tc of lastAssistant.tool_calls) {
        bytes += utf8ByteLength(tc.arguments ?? "");
      }
    }
    return Math.round(bytes / 4);
  });

  // ── Current running tool ──
  const currentTool = $derived.by(() => {
    if (session?.streaming_tool_name) {
      return { tool_name: session.streaming_tool_name };
    }
    if (session?.phase !== "streaming" && session?.phase !== "executing_tool")
      return null;
    for (let i = displayMessages.length - 1; i >= 0; i--) {
      const msg = displayMessages[i];
      if (msg.type === "tool") {
        if (msg.status === "running") return { tool_name: msg.tool_name };
        continue;
      }
      if (msg.type === "assistant") {
        const thinking = findThinking(msg.content);
        if (thinking || hasText(msg.content)) return null;
        if (msg.tool_calls && msg.tool_calls.length > 0) {
          return { tool_name: msg.tool_calls[msg.tool_calls.length - 1].name };
        }
        return null;
      }
      return null;
    }
    return null;
  });
</script>

{#if session}
  <div
    class="flex items-center justify-between px-3 py-1 text-xs border-b border-border bg-muted/30 min-h-7 font-mono"
  >
    <!-- Left: phase status -->
    <div class="flex items-center gap-1.5 min-w-0">
      {#if session?.phase === "streaming"}
        <Loader2 size={12} class="animate-spin text-primary shrink-0" />
      {:else if session?.phase === "executing_tool"}
        <Zap size={12} class="animate-breathe text-info shrink-0" />
      {:else if session?.phase === "compacting"}
        <Database size={12} class="animate-spin text-info shrink-0" />
      {:else if streamingTokens > 0}
        <Check size={12} class="text-success shrink-0" />
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
        <span class="text-info font-medium truncate italic"
          >· calling {currentTool.tool_name}</span
        >
      {:else if session?.phase === "streaming"}
        <span class="text-muted-foreground/70 shrink-0 italic">· generating</span>
      {:else if session?.phase === "compacting"}
        <span class="text-muted-foreground/70 shrink-0">· compacting</span>
      {/if}
    </div>

    <!-- Right: model + ctx -->
    <div class="flex items-center gap-2 shrink-0">
      {#if activeModel}
        {@const m = activeModel}
        {@const pct = (total_tokens / m.context_window) * 100}
        <span class="text-muted-foreground/60">{m.model_id}</span>
        <span class="text-muted-foreground/40">·</span>
        <span
          class="text-muted-foreground/60"
          class:text-warning={pct >= 70}
          class:text-error={pct >= 90}
        >
          {pct.toFixed(1)}% ({(m.context_window / 1000).toFixed(0)}K)
        </span>
      {:else if modelKey}
        <span class="text-muted-foreground/60">{modelKey}</span>
      {/if}
    </div>
  </div>
{/if}

<style>
  @keyframes breathe {
    0%,
    100% {
      opacity: 1;
      filter: drop-shadow(0 0 2px rgba(14, 165, 233, 0.4));
      transform: scale(1);
    }
    50% {
      opacity: 0.5;
      filter: drop-shadow(0 0 6px rgba(14, 165, 233, 0.9));
      transform: scale(1.15);
    }
  }
  :global(.animate-breathe) {
    animation: breathe 1.5s ease-in-out infinite;
  }
</style>
