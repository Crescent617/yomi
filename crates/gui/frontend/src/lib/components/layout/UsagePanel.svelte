<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import { sessionState, getActiveSession, getDisplayMessages } from "../../state.svelte";
  import {
    Cpu,
    MemoryStick,
    Hash,
    Layers,
    ArrowUpRight,
    ArrowDownLeft,
    Zap,
    Clock,
    MessageSquare,
    TrendingUp,
  } from "lucide-svelte";

  let config = $state<{ model: string; context_window: number; provider: string } | null>(null);
  let summary = $state<{ prompt_tokens: number; completion_tokens: number; cached_tokens: number; total_tokens: number; request_count: number } | null>(null);
  let daily = $state<{ date: string; prompt_tokens: number; completion_tokens: number; cached_tokens: number; total_tokens: number; request_count: number; models: string[] }[]>([]);
  let loading = $state(true);

  const activeSession = $derived(getActiveSession());
  const displayMessages = $derived(getDisplayMessages(activeSession?.id ?? ""));

  // Current session estimated tokens
  const sessionTokens = $derived.by(() => {
    if (!activeSession) return 0;
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

  const ctxPercent = $derived(
    config && sessionTokens > 0 ? Math.min((sessionTokens / config.context_window) * 100, 100) : 0
  );

  const ctxWarning = $derived(ctxPercent > 80);
  const ctxDanger = $derived(ctxPercent > 95);

  function formatNumber(n: number): string {
    if (n >= 1000000) return `${(n / 1000000).toFixed(2)}M`;
    if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
    return `${n}`;
  }

  function formatDate(d: string): string {
    const date = new Date(d);
    const now = new Date();
    const isToday = date.toDateString() === now.toDateString();
    if (isToday) return "Today";
    return date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  }

  function barHeight(tokens: number, max: number): string {
    if (max === 0) return "0%";
    return `${Math.min((tokens / max) * 100, 100)}%`;
  }

  onMount(async () => {
    try {
      [config, summary] = await Promise.all([
        api.getConfig(),
        api.getUsageSummary(),
      ]);
      daily = await api.getDailyUsage(7);
    } catch (e) {
      console.error("Failed to load usage:", e);
    } finally {
      loading = false;
    }
  });
</script>

<div class="h-full flex flex-col overflow-y-auto">
  <!-- Header -->
  <div class="shrink-0 px-6 py-4 border-b border-border">
    <div class="flex items-center gap-2">
      <TrendingUp class="w-5 h-5 text-primary" />
      <h2 class="text-lg font-semibold">Usage</h2>
    </div>
  </div>

  {#if loading}
    <div class="flex-1 flex items-center justify-center">
      <div class="w-6 h-6 border-2 border-primary border-t-transparent rounded-full animate-spin"></div>
    </div>
  {:else if !config}
    <div class="flex-1 flex items-center justify-center text-muted-foreground">
      Failed to load usage data
    </div>
  {:else}
    <div class="flex-1 p-6 space-y-6">
      <!-- Model & Context -->
      {#if activeSession}
        <div class="rounded-xl border border-border bg-card p-4 space-y-3">
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <Layers class="w-4 h-4 text-muted-foreground" />
              <span class="text-sm font-medium">Current Session</span>
              <span class="text-xs text-muted-foreground">{activeSession.alias ?? activeSession.id.slice(0, 8)}</span>
            </div>
            <span class="text-xs text-muted-foreground">{displayMessages.length} msgs</span>
          </div>

          <div class="space-y-1">
            <div class="flex items-center justify-between text-xs">
              <span class="text-muted-foreground">Context</span>
              <span class="font-mono" class:text-amber-500={ctxWarning} class:text-red-500={ctxDanger}>
                {formatNumber(sessionTokens)} / {formatNumber(config.context_window)}
              </span>
            </div>
            <div class="h-2 rounded-full bg-secondary overflow-hidden">
              <div
                class="h-full rounded-full transition-all duration-500 {ctxDanger ? 'bg-red-500' : ctxWarning ? 'bg-amber-500' : 'bg-primary'}"
                style="width: {ctxPercent}%"
              ></div>
            </div>
          </div>
        </div>
      {/if}

      <!-- Stats Grid -->
      {#if summary}
        <div class="grid grid-cols-2 lg:grid-cols-4 gap-3">
          <div class="rounded-xl border border-border bg-card p-3 space-y-1">
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <ArrowUpRight class="w-3.5 h-3.5" />
              Prompt
            </div>
            <div class="text-lg font-semibold font-mono">{formatNumber(summary.prompt_tokens)}</div>
          </div>
          <div class="rounded-xl border border-border bg-card p-3 space-y-1">
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <ArrowDownLeft class="w-3.5 h-3.5" />
              Completion
            </div>
            <div class="text-lg font-semibold font-mono">{formatNumber(summary.completion_tokens)}</div>
          </div>
          <div class="rounded-xl border border-border bg-card p-3 space-y-1">
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <Zap class="w-3.5 h-3.5" />
              Total
            </div>
            <div class="text-lg font-semibold font-mono">{formatNumber(summary.total_tokens)}</div>
          </div>
          <div class="rounded-xl border border-border bg-card p-3 space-y-1">
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <Hash class="w-3.5 h-3.5" />
              Requests
            </div>
            <div class="text-lg font-semibold font-mono">{formatNumber(summary.request_count)}</div>
          </div>
        </div>
      {/if}

      <!-- Daily Chart -->
      {#if daily.length > 0}
        <div class="rounded-xl border border-border bg-card p-4 space-y-3">
          <div class="flex items-center gap-2">
            <Clock class="w-4 h-4 text-muted-foreground" />
            <span class="text-sm font-medium">Daily Usage</span>
          </div>
          <div class="flex items-end gap-1 h-32 px-2">
            {#each daily as day (day.date)}
              {@const maxTokens = Math.max(...daily.map(d => d.total_tokens)) || 1}
              <div class="flex-1 flex flex-col items-center gap-1 min-w-0">
                <div class="w-full flex gap-0.5 h-24 items-end">
                  <!-- Prompt bar -->
                  <div
                    class="flex-1 bg-primary/60 rounded-t-sm"
                    style="height: {barHeight(day.prompt_tokens, maxTokens)}"
                    title="Prompt: {formatNumber(day.prompt_tokens)}"
                  ></div>
                  <!-- Completion bar -->
                  <div
                    class="flex-1 bg-primary rounded-t-sm"
                    style="height: {barHeight(day.completion_tokens, maxTokens)}"
                    title="Completion: {formatNumber(day.completion_tokens)}"
                  ></div>
                </div>
                <span class="text-[10px] text-muted-foreground truncate w-full text-center">{formatDate(day.date)}</span>
              </div>
            {/each}
          </div>
          <div class="flex items-center gap-4 text-xs text-muted-foreground">
            <div class="flex items-center gap-1">
              <div class="w-2 h-2 rounded-sm bg-primary/60"></div>
              <span>Prompt</span>
            </div>
            <div class="flex items-center gap-1">
              <div class="w-2 h-2 rounded-sm bg-primary"></div>
              <span>Completion</span>
            </div>
          </div>
        </div>
      {/if}

      <!-- Model Info -->
      <div class="rounded-xl border border-border bg-card p-4">
        <div class="flex items-center gap-2 mb-3">
          <Cpu class="w-4 h-4 text-muted-foreground" />
          <span class="text-sm font-medium">Model</span>
        </div>
        <div class="flex items-center justify-between py-2 border-b border-border">
          <span class="text-sm text-muted-foreground">Provider</span>
          <span class="text-sm font-medium">{config.provider}</span>
        </div>
        <div class="flex items-center justify-between py-2 border-b border-border">
          <span class="text-sm text-muted-foreground">Model</span>
          <span class="text-sm font-medium">{config.model}</span>
        </div>
        <div class="flex items-center justify-between py-2">
          <span class="text-sm text-muted-foreground">Context Window</span>
          <span class="text-sm font-medium">{formatNumber(config.context_window)}</span>
        </div>
      </div>

      <!-- Sessions -->
      {#if sessionState.sessions.length > 0}
        <div class="rounded-xl border border-border bg-card overflow-hidden">
          <div class="px-4 py-3 border-b border-border flex items-center gap-2">
            <MessageSquare class="w-4 h-4 text-muted-foreground" />
            <span class="text-sm font-medium">Sessions</span>
          </div>
          <div class="divide-y divide-border">
            {#each sessionState.sessions as session (session.id)}
              <div class="px-4 py-2.5 flex items-center justify-between text-sm">
                <div class="flex items-center gap-2 min-w-0">
                  <span class="truncate">{session.alias ?? session.id.slice(0, 8)}</span>
                  {#if session.streaming}
                    <span class="w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span>
                  {/if}
                </div>
                <span class="text-xs text-muted-foreground shrink-0">{session.messages.length} msgs</span>
              </div>
            {/each}
          </div>
        </div>
      {/if}
    </div>
  {/if}
</div>
