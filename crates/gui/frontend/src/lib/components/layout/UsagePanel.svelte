<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as echarts from "echarts";
  import * as api from "../../api";
  import {
    ArrowUpRight,
    ArrowDownLeft,
    Zap,
    Calendar,
    TrendingUp,
    PanelLeftOpen,
    Award,
    BarChart3,
    Flame,
    Activity,
    Cpu,
    Hash,
  } from "lucide-svelte";

  let {
    onToggleLeftPanel,
  }: {
    onToggleLeftPanel?: () => void;
  } = $props();

  interface DayData {
    date: string;
    prompt_tokens: number;
    completion_tokens: number;
    cached_tokens: number;
    total_tokens: number;
    request_count: number;
    models: string[];
  }

  let config = $state<{ model: string; context_window: number; provider: string } | null>(null);
  let summary = $state<{ prompt_tokens: number; completion_tokens: number; cached_tokens: number; total_tokens: number; request_count: number } | null>(null);
  let daily = $state<DayData[]>([]);
  let loading = $state(true);

  let chartDiv: HTMLDivElement | null = $state(null);
  let chartInstance: echarts.ECharts | null = $state(null);

  const DAYS_RANGE = 365;

  // ── derived stats ──

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

  const activeDays = $derived.by(() => daily.filter((d) => d.total_tokens > 0).length);

  const streaks = $derived.by(() => {
    if (daily.length === 0) return { current: 0, longest: 0 };
    const activeSet = new Set(daily.filter((d) => d.total_tokens > 0).map((d) => d.date));

    // Current streak from today backwards
    let current = 0;
    const today = new Date();
    for (let i = 0; i < 365; i++) {
      const d = new Date(today);
      d.setDate(d.getDate() - i);
      const iso = d.toISOString().slice(0, 10);
      if (activeSet.has(iso)) {
        current++;
      } else if (i > 0) {
        break;
      }
    }

    // Longest streak
    const sorted = [...daily]
      .filter((d) => d.total_tokens > 0)
      .sort((a, b) => a.date.localeCompare(b.date));
    let longest = 0;
    let cur = 0;
    let prev: string | null = null;
    for (const d of sorted) {
      if (prev) {
        const a = new Date(prev + "T00:00:00");
        const b = new Date(d.date + "T00:00:00");
        const diffDays = (b.getTime() - a.getTime()) / 86400000;
        if (diffDays === 1) {
          cur++;
        } else {
          longest = Math.max(longest, cur);
          cur = 1;
        }
      } else {
        cur = 1;
      }
      prev = d.date;
    }
    longest = Math.max(longest, cur);

    return { current, longest };
  });

  const topDays = $derived.by(() =>
    [...daily].sort((a, b) => b.total_tokens - a.total_tokens).slice(0, 10)
  );

  const busiestDay = $derived.by(() => {
    if (daily.length === 0) return null;
    return daily.reduce((max, d) => (d.total_tokens > max.total_tokens ? d : max), daily[0]);
  });

  const mostRequestsDay = $derived.by(() => {
    if (daily.length === 0) return null;
    return daily.reduce((max, d) => (d.request_count > max.request_count ? d : max), daily[0]);
  });

  // ── helpers ──

  function formatNumber(n: number): string {
    if (n >= 1000000) return `${(n / 1000000).toFixed(2)}M`;
    if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
    return `${n}`;
  }

  function cacheRate(d: { prompt_tokens: number; cached_tokens: number }): string {
    if (d.prompt_tokens === 0) return "0%";
    return `${Math.round((d.cached_tokens / d.prompt_tokens) * 100)}%`;
  }

  function formatDateLabel(d: string): string {
    const date = new Date(d + "T00:00:00");
    const now = new Date();
    if (date.toDateString() === now.toDateString()) return "Today";
    return date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" });
  }

  function formatFullDate(d: string): string {
    const date = new Date(d + "T00:00:00");
    return date.toLocaleDateString(undefined, { year: "numeric", month: "long", day: "numeric", weekday: "long" });
  }

  /** Fill 365 days back from today so the heatmap has a fixed width */
  const filledDaily = $derived.by(() => {
    const map = new Map<string, DayData>();
    for (const d of daily) map.set(d.date, d);
    const today = new Date();
    today.setHours(0, 0, 0, 0);
    const result: DayData[] = [];
    for (let i = DAYS_RANGE - 1; i >= 0; i--) {
      const cur = new Date(today);
      cur.setDate(cur.getDate() - i);
      const iso = cur.toISOString().slice(0, 10);
      result.push(
        map.get(iso) ?? {
          date: iso,
          prompt_tokens: 0,
          completion_tokens: 0,
          cached_tokens: 0,
          total_tokens: 0,
          request_count: 0,
          models: [],
        }
      );
    }
    return result;
  });

  // ── echarts ──

  function isDarkMode() {
    return document.documentElement.classList.contains("dark");
  }

  function getHeatColors(dark: boolean): string[] {
    // GitHub contribution graph colors
    if (dark) {
      return ["#161b22", "#0e4429", "#006d32", "#26a641", "#39d353"];
    }
    return ["#ebedf0", "#9be9a8", "#40c463", "#30a14e", "#216e39"];
  }

  function getBorderColor(dark: boolean): string {
    return dark ? "#0f172a" : "#ffffff";
  }

  function getTooltipCacheColor(dark: boolean): string {
    return dark ? "#39d353" : "#216e39";
  }

  function buildChartOption(data: DayData[]): echarts.EChartsOption {
    const dark = isDarkMode();
    const colors = getHeatColors(dark);
    const borderColor = getBorderColor(dark);
    const textColor = dark ? "#94a3b8" : "#64748b";

    const maxTokens = Math.max(...data.map((d) => d.total_tokens), 1);
    const chartData = data.map((d) => [d.date, d.total_tokens]);
    const start = data[0]?.date ?? "";
    const end = data[data.length - 1]?.date ?? "";

    return {
      backgroundColor: "transparent",
      tooltip: {
        appendToBody: true,
        backgroundColor: dark ? "#1e293b" : "#ffffff",
        borderColor: dark ? "#334155" : "#e2e8f0",
        textStyle: { color: dark ? "#f1f5f9" : "#0f172a", fontSize: 12 },
        extraCssText: "border-radius: 0.5rem; box-shadow: 0 4px 6px -1px rgba(0,0,0,0.1); z-index: 9999;",
        formatter: (params: { value: [string, number] }) => {
          const day = data.find((d) => d.date === params.value[0]);
          if (!day) return params.value[0];
          const total = day.prompt_tokens + day.cached_tokens + day.completion_tokens;
          let html = `<div style="font-weight:600;margin-bottom:4px;">${formatFullDate(day.date)}</div>`;
          if (total > 0) {
            html += `<div>Prompt: <b>${formatNumber(day.prompt_tokens)}</b></div>`;
            html += `<div>Cached: <b style="color:${getTooltipCacheColor(dark)}">${formatNumber(day.cached_tokens)}</b> (${cacheRate(day)})</div>`;
            html += `<div>Completion: <b>${formatNumber(day.completion_tokens)}</b></div>`;
            html += `<div>Total: <b>${formatNumber(total)}</b></div>`;
            html += `<div>Requests: <b>${formatNumber(day.request_count)}</b></div>`;
            if (day.models.length) html += `<div style="margin-top:4px;opacity:0.7">${day.models.join(", ")}</div>`;
          } else {
            html += `<div style="opacity:0.7">No activity</div>`;
          }
          return html;
        },
      },
      visualMap: {
        show: false,
        min: 0,
        max: maxTokens,
        type: "piecewise",
        pieces: [
          { min: 0, max: 0, color: colors[0] },
          { min: 1, max: Math.round(maxTokens * 0.25), color: colors[1] },
          { min: Math.round(maxTokens * 0.25) + 1, max: Math.round(maxTokens * 0.5), color: colors[2] },
          { min: Math.round(maxTokens * 0.5) + 1, max: Math.round(maxTokens * 0.75), color: colors[3] },
          { min: Math.round(maxTokens * 0.75) + 1, color: colors[4] },
        ],
      },
      calendar: {
        top: 18,
        left: 32,
        right: 4,
        bottom: 4,
        cellSize: [10, 10],
        range: [start, end],
        itemStyle: {
          color: colors[0],
          borderWidth: 3,
          borderColor: borderColor,
          borderRadius: 2,
        },
        splitLine: { show: false },
        yearLabel: { show: false },
        monthLabel: {
          show: true,
          nameMap: ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"],
          color: textColor,
          fontSize: 9,
          align: "left",
          margin: 4,
        },
        dayLabel: {
          show: true,
          firstDay: 1,
          nameMap: ["", "M", "", "W", "", "F", ""],
          color: textColor,
          fontSize: 9,
        },
        weekLabel: { show: false },
      },
      series: [
        {
          type: "heatmap",
          coordinateSystem: "calendar",
          data: chartData,
        },
      ],
    };
  }

  function calcChartWidth(data: DayData[]): number {
    if (data.length === 0) return 200;
    const first = new Date(data[0].date + "T00:00:00");
    const last = new Date(data[data.length - 1].date + "T00:00:00");
    const firstDay = first.getDay(); // 0=Sun
    const mondayOffset = firstDay === 0 ? 6 : firstDay - 1;
    const lastDay = last.getDay();
    const sundayOffset = lastDay === 0 ? 0 : 7 - lastDay;
    const totalWeeks = Math.ceil((mondayOffset + data.length + sundayOffset) / 7);
    const cellW = 10;
    const gap = 3; // borderWidth
    const labelW = 32; // dayLabel space on left
    const rightPad = 4;
    return labelW + totalWeeks * (cellW + gap) + rightPad;
  }

  function renderChart() {
    if (!chartDiv || filledDaily.length === 0) return;
    if (!chartInstance) {
      chartInstance = echarts.init(chartDiv, undefined, { renderer: "canvas" });
    }
    chartInstance.setOption(buildChartOption(filledDaily), true);
    chartDiv.style.height = "128px";
    chartDiv.style.width = `${calcChartWidth(filledDaily)}px`;
    chartInstance.resize();
  }

  function disposeChart() {
    chartInstance?.dispose();
    chartInstance = null;
  }

  // ── lifecycle ──

  onMount(() => {
    loadData();

    const resize = () => chartInstance?.resize();
    window.addEventListener("resize", resize);

    const onTheme = () => {
      if (chartInstance && filledDaily.length > 0) {
        chartInstance.setOption(buildChartOption(filledDaily), true);
      }
    };
    window.addEventListener("theme-changed", onTheme);

    return () => {
      window.removeEventListener("resize", resize);
      window.removeEventListener("theme-changed", onTheme);
      disposeChart();
    };
  });

  $effect(() => {
    if (filledDaily.length > 0 && chartDiv) {
      tick().then(renderChart);
    }
  });

  async function loadData() {
    console.log("[UsagePanel] loadData start");
    loading = true;
    try {
      const cfg = await api.getConfig();
      console.log("[UsagePanel] config ok", cfg);
      config = cfg;

      const sum = await api.getUsageSummary();
      console.log("[UsagePanel] summary ok", sum);
      summary = sum;

      const d = await api.getDailyUsage(DAYS_RANGE);
      console.log("[UsagePanel] daily ok", d?.length ?? 0, "days");
      daily = d ?? [];
    } catch (e: unknown) {
      console.error("[UsagePanel] loadData failed:", e instanceof Error ? e.message : e);
    } finally {
      loading = false;
      console.log("[UsagePanel] loadData done, loading=false");
    }
  }
</script>

<div class="flex-1 flex flex-col min-w-0 overflow-y-auto">
  <div class="container mx-auto px-4 lg:px-6">
    <!-- Header -->
    <div class="shrink-0 py-4 border-b border-border">
      <div class="flex items-center justify-between">
        <div class="flex items-center gap-2">
          {#if onToggleLeftPanel}
            <button
              type="button"
              onclick={() => onToggleLeftPanel?.()}
              class="lg:hidden p-1.5 rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground mr-1"
              title="Toggle sidebar"
            >
              <PanelLeftOpen size={18} />
            </button>
          {/if}
          <TrendingUp class="w-5 h-5 text-primary" />
          <h2 class="text-lg font-semibold">Usage</h2>
          <span class="text-xs text-muted-foreground ml-1">· last 365 days</span>
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
      <div class="flex-1 py-6 space-y-6">
        <!-- Summary Cards -->
        {#if filteredSummary}
          <div class="grid grid-cols-2 lg:grid-cols-6 gap-3">
            <!-- Model tag -->
            <div class="rounded-xl border border-border bg-card p-3 space-y-1 col-span-2 lg:col-span-1">
              <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                <Cpu class="w-3.5 h-3.5" />
                Model
              </div>
              <div class="text-sm font-semibold truncate" title={config.model}>{config.model}</div>
              <div class="text-[10px] text-muted-foreground">{config.provider}</div>
            </div>

            <div class="rounded-xl border border-border bg-card p-3 space-y-1">
              <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                <Zap class="w-3.5 h-3.5" />
                Total
              </div>
              <div class="text-lg font-semibold font-mono">{formatNumber(filteredSummary.total_tokens)}</div>
              <div class="text-[10px] text-muted-foreground">tokens</div>
            </div>

            <div class="rounded-xl border border-border bg-card p-3 space-y-1">
              <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                <Hash class="w-3.5 h-3.5" />
                Requests
              </div>
              <div class="text-lg font-semibold font-mono">{formatNumber(filteredSummary.request_count)}</div>
              <div class="text-[10px] text-muted-foreground">calls</div>
            </div>

            <div class="rounded-xl border border-border bg-card p-3 space-y-1">
              <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                <ArrowUpRight class="w-3.5 h-3.5" />
                Prompt
              </div>
              <div class="text-lg font-semibold font-mono">{formatNumber(filteredSummary.prompt_tokens)}</div>
              <div class="text-[10px] text-muted-foreground">
                {cacheRate(filteredSummary)} cache
              </div>
            </div>

            <div class="rounded-xl border border-border bg-card p-3 space-y-1">
              <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                <ArrowDownLeft class="w-3.5 h-3.5" />
                Completion
              </div>
              <div class="text-lg font-semibold font-mono">{formatNumber(filteredSummary.completion_tokens)}</div>
              <div class="text-[10px] text-muted-foreground">tokens</div>
            </div>

            <div class="rounded-xl border border-border bg-card p-3 space-y-1">
              <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                <Activity class="w-3.5 h-3.5" />
                Active
              </div>
              <div class="text-lg font-semibold font-mono">{activeDays}<span class="text-sm text-muted-foreground font-normal">/{daily.length}</span></div>
              <div class="text-[10px] text-muted-foreground">days</div>
            </div>
          </div>
        {/if}

        <!-- ECharts Heatmap (square cells) -->
        {#if filledDaily.length > 0}
          <div class="rounded-xl border border-border bg-card p-4 space-y-2">
            <div class="flex items-center gap-2">
              <Calendar class="w-4 h-4 text-muted-foreground" />
              <span class="text-sm font-medium">Activity Heatmap</span>
              <span class="text-xs text-muted-foreground ml-auto">
                {activeDays} active · {daily.length} days
              </span>
            </div>
            <div class="flex justify-end overflow-x-auto">
              <div bind:this={chartDiv} style="height: 128px;"></div>
            </div>
          </div>
        {:else}
          <div class="rounded-xl border border-border bg-card p-8 text-center text-sm text-muted-foreground">
            No activity data for the last {DAYS_RANGE} days
          </div>
        {/if}

        <!-- Streak + Highlights row -->
        {#if daily.length > 0}
          <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
            <div class="rounded-xl border border-border bg-card p-3 space-y-1.5">
              <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                <Flame class="w-3.5 h-3.5 text-orange-500" />
                Current Streak
              </div>
              <div class="text-2xl font-bold font-mono">{streaks.current}<span class="text-sm font-normal text-muted-foreground">d</span></div>
              <div class="text-[10px] text-muted-foreground">consecutive days</div>
            </div>

            <div class="rounded-xl border border-border bg-card p-3 space-y-1.5">
              <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                <Award class="w-3.5 h-3.5 text-amber-500" />
                Longest Streak
              </div>
              <div class="text-2xl font-bold font-mono">{streaks.longest}<span class="text-sm font-normal text-muted-foreground">d</span></div>
              <div class="text-[10px] text-muted-foreground">best ever</div>
            </div>

            {#if busiestDay && busiestDay.total_tokens > 0}
              <div class="rounded-xl border border-border bg-card p-3 space-y-1.5">
                <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                  <Zap class="w-3.5 h-3.5 text-primary" />
                  Busiest Day
                </div>
                <div class="text-sm font-medium">{formatDateLabel(busiestDay.date)}</div>
                <div class="text-xs text-muted-foreground">
                  {formatNumber(busiestDay.total_tokens)} tokens · {formatNumber(busiestDay.request_count)} req
                </div>
              </div>
            {/if}

            {#if mostRequestsDay && mostRequestsDay.request_count > 0}
              <div class="rounded-xl border border-border bg-card p-3 space-y-1.5">
                <div class="flex items-center gap-1.5 text-xs text-muted-foreground">
                  <Hash class="w-3.5 h-3.5 text-blue-500" />
                  Most Requests
                </div>
                <div class="text-sm font-medium">{formatDateLabel(mostRequestsDay.date)}</div>
                <div class="text-xs text-muted-foreground">
                  {formatNumber(mostRequestsDay.request_count)} req · {formatNumber(mostRequestsDay.total_tokens)} tok
                </div>
              </div>
            {/if}
          </div>
        {/if}

        <!-- Top Days Table -->
        {#if topDays.length > 0}
          <div class="rounded-xl border border-border bg-card overflow-hidden">
            <div class="flex items-center gap-2 px-4 py-3 border-b border-border">
              <BarChart3 class="w-4 h-4 text-muted-foreground" />
              <span class="text-sm font-medium">Top Days by Volume</span>
              <span class="text-xs text-muted-foreground ml-auto">{topDays.length} of {daily.length} days</span>
            </div>
            <div class="overflow-x-auto">
              <table class="w-full text-sm">
                <thead>
                  <tr class="border-b border-border bg-muted/30">
                    <th class="text-left px-4 py-2 text-xs font-medium text-muted-foreground">Date</th>
                    <th class="text-right px-4 py-2 text-xs font-medium text-muted-foreground">Requests</th>
                    <th class="text-right px-4 py-2 text-xs font-medium text-muted-foreground">Prompt</th>
                    <th class="text-right px-4 py-2 text-xs font-medium text-muted-foreground">Cached</th>
                    <th class="text-right px-4 py-2 text-xs font-medium text-muted-foreground">Completion</th>
                    <th class="text-right px-4 py-2 text-xs font-medium text-muted-foreground">Total</th>
                    <th class="text-right px-4 py-2 text-xs font-medium text-muted-foreground">Cache</th>
                    <th class="text-left px-4 py-2 text-xs font-medium text-muted-foreground">Models</th>
                  </tr>
                </thead>
                <tbody>
                  {#each topDays as day, i (day.date)}
                    <tr class="border-b border-border last:border-0 {i % 2 === 0 ? 'bg-background' : 'bg-muted/20'} hover:bg-muted/40 transition-colors">
                      <td class="px-4 py-2 whitespace-nowrap">
                        <div class="text-sm font-medium">{formatDateLabel(day.date)}</div>
                        <div class="text-[10px] text-muted-foreground">{day.date}</div>
                      </td>
                      <td class="px-4 py-2 text-right font-mono">{formatNumber(day.request_count)}</td>
                      <td class="px-4 py-2 text-right font-mono">{formatNumber(day.prompt_tokens)}</td>
                      <td class="px-4 py-2 text-right font-mono text-green-600">{formatNumber(day.cached_tokens)}</td>
                      <td class="px-4 py-2 text-right font-mono">{formatNumber(day.completion_tokens)}</td>
                      <td class="px-4 py-2 text-right font-mono font-medium">{formatNumber(day.total_tokens)}</td>
                      <td class="px-4 py-2 text-right">
                        <span class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium {day.prompt_tokens > 0 && day.cached_tokens / day.prompt_tokens > 0.5 ? 'bg-green-100 text-green-700 dark:bg-green-900/30 dark:text-green-400' : 'bg-muted text-muted-foreground'}">
                          {cacheRate(day)}
                        </span>
                      </td>
                      <td class="px-4 py-2">
                        <div class="flex flex-wrap gap-1">
                          {#each day.models as model (model)}
                            <span class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] bg-secondary text-secondary-foreground">{model}</span>
                          {/each}
                        </div>
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
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
      </div>
    {/if}
  </div>
</div>