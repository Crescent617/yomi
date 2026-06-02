<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import {
    Cpu,
    Hash,
    ArrowUpRight,
    ArrowDownLeft,
    Zap,
    Clock,
    TrendingUp,
    Calendar,
  } from "lucide-svelte";

  let config = $state<{ model: string; context_window: number; provider: string } | null>(null);
  let summary = $state<{ prompt_tokens: number; completion_tokens: number; cached_tokens: number; total_tokens: number; request_count: number } | null>(null);
  let daily = $state<{ date: string; prompt_tokens: number; completion_tokens: number; cached_tokens: number; total_tokens: number; request_count: number; models: string[] }[]>([]);
  let loading = $state(true);
  let daysRange = $state(7);

  const DAYS_OPTIONS = [
    { label: "7 days", value: 7 },
    { label: "30 days", value: 30 },
    { label: "All time", value: 3650 },
  ];

  const filteredSummary = $derived.by(() => {
    if (daily.length === 0) return summary;
    return daily.reduce(
      (acc, d) => ({
        prompt_tokens: acc.prompt_tokens + d.prompt_tokens,
        completion_tokens: acc.completion_tokens + d.completion_tokens,
        cached_tokens: acc.cached_tokens + d.cached_tokens,
        total_tokens: acc.total_tokens + d.total_tokens,
        request_count: acc.request_count + d.request_count,
      }),
      { prompt_tokens: 0, completion_tokens: 0, cached_tokens: 0, total_tokens: 0, request_count: 0 }
    );
  });

  function formatNumber(n: number): string {
    if (n >= 1000000) return `${(n / 1000000).toFixed(2)}M`;
    if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
    return `${n}`;
  }

  function cacheRate(summary: { prompt_tokens: number; cached_tokens: number }): string {
    if (summary.prompt_tokens === 0) return "0%";
    return `${Math.round((summary.cached_tokens / summary.prompt_tokens) * 100)}%`;
  }

  function formatTokens(d: { prompt_tokens: number; completion_tokens: number; cached_tokens: number; total_tokens: number; request_count: number }): string {
    const actualTotal = d.prompt_tokens + d.completion_tokens + d.cached_tokens;
    return `Prompt: ${formatNumber(d.prompt_tokens)} · Cached: ${formatNumber(d.cached_tokens)} · Completion: ${formatNumber(d.completion_tokens)} · Total: ${formatNumber(actualTotal)} · Requests: ${formatNumber(d.request_count)}`;
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

  async function loadData() {
    loading = true;
    try {
      [config, summary] = await Promise.all([
        api.getConfig(),
        api.getUsageSummary(),
      ]);
      daily = await api.getDailyUsage(daysRange);
    } catch (e) {
      console.error("Failed to load usage:", e);
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    loadData();
  });

  async function changeRange(value: number) {
    daysRange = value;
    loading = true;
    try {
      daily = await api.getDailyUsage(value);
    } catch (e) {
      console.error("Failed to load daily usage:", e);
    } finally {
      loading = false;
    }
  }
</script>

<div class="flex-1 flex flex-col min-w-0 overflow-y-auto">
  <!-- Header -->
  <div class="shrink-0 px-6 py-4 border-b border-border">
    <div class="flex items-center justify-between">
      <div class="flex items-center gap-2">
        <TrendingUp class="w-5 h-5 text-primary" />
        <h2 class="text-lg font-semibold">Usage</h2>
      </div>
      <div class="flex items-center gap-1 rounded-lg border border-border bg-background p-0.5">
        {#each DAYS_OPTIONS as opt (opt.value)}
          <button
            type="button"
            class="px-3 py-1 text-xs font-medium rounded-md transition-colors
                   {daysRange === opt.value
                     ? 'bg-primary text-primary-foreground'
                     : 'text-muted-foreground hover:text-foreground hover:bg-secondary/50'}"
            onclick={() => changeRange(opt.value)}
          >
            {opt.label}
          </button>
        {/each}
      </div>
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
      <!-- Stats Grid -->
      {#if filteredSummary}
        <div class="grid grid-cols-2 lg:grid-cols-5 gap-3">
          <div class="rounded-xl border border-border bg-card p-3 space-y-1">
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <ArrowUpRight class="w-3.5 h-3.5" />
              Prompt
            </div>
            <div class="text-lg font-semibold font-mono">{formatNumber(filteredSummary.prompt_tokens)}</div>
            <div class="text-[10px] text-muted-foreground">
              cached {formatNumber(filteredSummary.cached_tokens)} ({cacheRate(filteredSummary)})
            </div>
          </div>
          <div class="rounded-xl border border-border bg-card p-3 space-y-1">
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <ArrowDownLeft class="w-3.5 h-3.5" />
              Completion
            </div>
            <div class="text-lg font-semibold font-mono">{formatNumber(filteredSummary.completion_tokens)}</div>
          </div>
          <div class="rounded-xl border border-border bg-card p-3 space-y-1">
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <Zap class="w-3.5 h-3.5" />
              Total
            </div>
            <div class="text-lg font-semibold font-mono">{formatNumber(filteredSummary.total_tokens)}</div>
          </div>
          <div class="rounded-xl border border-border bg-card p-3 space-y-1">
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <Hash class="w-3.5 h-3.5" />
              Requests
            </div>
            <div class="text-lg font-semibold font-mono">{formatNumber(filteredSummary.request_count)}</div>
          </div>
          <div class="rounded-xl border border-border bg-card p-3 space-y-1">
            <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <Cpu class="w-3.5 h-3.5" />
              Cache Rate
            </div>
            <div class="text-lg font-semibold font-mono {filteredSummary.cached_tokens > 0 ? 'text-green-500' : 'text-muted-foreground'}">
              {cacheRate(filteredSummary)}
            </div>
          </div>
        </div>
      {/if}

      <!-- Daily Chart -->
      {#if daily.length > 0}
        {@const chartTotal = daily.reduce((a, d) => a + d.prompt_tokens + d.completion_tokens + d.cached_tokens, 0)}
        {@const chartCached = daily.reduce((a, d) => a + d.cached_tokens, 0)}
        {@const chartPrompt = daily.reduce((a, d) => a + d.prompt_tokens, 0)}
        {@const chartAvgCache = chartPrompt > 0 ? Math.round((chartCached / chartPrompt) * 100) : 0}
        <div class="rounded-xl border border-border bg-card p-4 space-y-3">
          <div class="flex items-center gap-2">
            <Calendar class="w-4 h-4 text-muted-foreground" />
            <span class="text-sm font-medium">Daily Usage</span>
            <span class="text-xs text-muted-foreground">({daily.length} days · {formatNumber(chartTotal)} total · {chartAvgCache}% cache)</span>
          </div>
          <div class="flex items-end gap-1 h-32 px-2">
            {#each daily as day (day.date)}
              {@const maxTokens = Math.max(...daily.map(d => d.prompt_tokens + d.completion_tokens + d.cached_tokens)) || 1}
              {@const dayCache = day.prompt_tokens > 0 ? Math.round((day.cached_tokens / day.prompt_tokens) * 100) : 0}
              <div class="flex-1 flex flex-col items-center gap-1 min-w-0 relative group" role="presentation" aria-hidden="true" style="--tip-x: 50%;" onmousemove={(e) => { e.currentTarget.style.setProperty('--tip-x', `${e.offsetX}px`); }}>
                <!-- Hover tooltip -->
                <div class="absolute bottom-full mb-1 opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none z-10 w-max whitespace-nowrap" style="left: var(--tip-x); transform: translateX(-50%);">
                  <div class="rounded-lg border border-border bg-popover shadow-md px-2.5 py-1.5 text-xs text-foreground">
                    <div class="font-medium mb-1">{formatDate(day.date)}</div>
                    <div class="text-muted-foreground">Prompt: <span class="text-foreground">{formatNumber(day.prompt_tokens)}</span></div>
                    <div class="text-muted-foreground">Cached: <span class="text-green-500">{formatNumber(day.cached_tokens)}</span> ({dayCache}%)</div>
                    <div class="text-muted-foreground">Completion: <span class="text-foreground">{formatNumber(day.completion_tokens)}</span></div>
                    <div class="text-muted-foreground">Total: <span class="text-foreground">{formatNumber(day.prompt_tokens + day.cached_tokens + day.completion_tokens)}</span></div>
                    <div class="text-muted-foreground">Requests: <span class="text-foreground">{formatNumber(day.request_count)}</span></div>
                    {#if day.models.length > 0}
                      <div class="text-muted-foreground mt-1">{day.models.join(', ')}</div>
                    {/if}
                  </div>
                </div>
                <div class="w-full flex gap-0.5 h-24 items-end">
                  <!-- Cached bar -->
                  <div
                    class="flex-1 bg-green-500/60 rounded-t-sm"
                    style="height: {barHeight(day.cached_tokens, maxTokens)}"
                  ></div>
                  <!-- Prompt bar -->
                  <div
                    class="flex-1 bg-primary/60 rounded-t-sm"
                    style="height: {barHeight(day.prompt_tokens, maxTokens)}"
                  ></div>
                  <!-- Completion bar -->
                  <div
                    class="flex-1 bg-primary rounded-t-sm"
                    style="height: {barHeight(day.completion_tokens, maxTokens)}"
                  ></div>
                </div>
                <span class="text-[10px] text-muted-foreground truncate w-full text-center">{formatDate(day.date)}</span>
              </div>
            {/each}
          </div>
          <div class="flex items-center gap-4 text-xs text-muted-foreground">
            <div class="flex items-center gap-1">
              <div class="w-2 h-2 rounded-sm bg-green-500/60"></div>
              <span>Cached</span>
            </div>
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
      {:else}
        <div class="rounded-xl border border-border bg-card p-8 text-center text-sm text-muted-foreground">
          No usage data for the selected period
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
    </div>
  {/if}
</div>
