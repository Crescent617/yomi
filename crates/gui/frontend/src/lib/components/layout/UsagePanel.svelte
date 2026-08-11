<script lang="ts">
  import { onMount, tick } from "svelte";
  import * as echarts from "echarts";
  import * as api from "../../api";
  import type { ModelInfo, ModelUsage, UsageRecord } from "../../api";
  import { getActiveSession } from "../../state.svelte";
  import InlineLoadingStatus from "../ui/InlineLoadingStatus.svelte";
  import PageLoading from "../ui/PageLoading.svelte";
  import UsagePageSkeleton from "./UsagePageSkeleton.svelte";
  import PanelHeader from "./PanelHeader.svelte";
  import {
    ArrowUpRight,
    ArrowDownLeft,
    Zap,
    Calendar,
    CalendarRange,
    TrendingUp,
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

  let config = $state<{
    model: string;
    context_window: number;
    provider: string;
  } | null>(null);
  let summary = $state<{
    prompt_tokens: number;
    completion_tokens: number;
    cached_tokens: number;
    total_tokens: number;
    request_count: number;
  } | null>(null);
  let daily = $state<DayData[]>([]);
  let todayUsage = $state<ModelUsage[]>([]);
  let allModelUsage = $state<ModelUsage[]>([]);
  let configuredModels = $state<ModelInfo[]>([]);
  let loading = $state(true);

  // ── raw request records (infinite scroll) ──
  let records = $state<UsageRecord[]>([]);
  let recordsLoading = $state(false);
  let recordsDone = $state(false);
  let recordsSentinel = $state<HTMLDivElement | null>(null);
  let recordsScrollContainer = $state<HTMLDivElement | null>(null);
  const RECORDS_PAGE_SIZE = 50;

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
      {
        prompt_tokens: 0,
        completion_tokens: 0,
        cached_tokens: 0,
        total_tokens: 0,
        request_count: 0,
      },
    );
  });

  const activeDays = $derived.by(
    () => daily.filter((d) => d.total_tokens > 0).length,
  );

  // ── month-to-date (local 1st of current month → today) ──

  const monthStartDate = $derived.by(() => {
    const now = new Date();
    return new Date(now.getFullYear(), now.getMonth(), 1);
  });

  const monthSummary = $derived.by(() => {
    const start = toLocalISODate(monthStartDate);
    const acc = {
      prompt_tokens: 0,
      completion_tokens: 0,
      cached_tokens: 0,
      total_tokens: 0,
      request_count: 0,
    };
    for (const d of daily) {
      if (d.date >= start) {
        acc.prompt_tokens += d.prompt_tokens;
        acc.completion_tokens += d.completion_tokens;
        acc.cached_tokens += d.cached_tokens;
        acc.total_tokens += d.total_tokens;
        acc.request_count += d.request_count;
      }
    }
    return acc;
  });

  const monthStartLabel = $derived(
    monthStartDate.toLocaleDateString(undefined, {
      month: "short",
      day: "numeric",
    }),
  );

  const streaks = $derived.by(() => {
    if (daily.length === 0) return { current: 0, longest: 0 };
    const activeSet = new Set(
      daily.filter((d) => d.total_tokens > 0).map((d) => d.date),
    );

    // Current streak from today backwards
    let current = 0;
    const today = new Date();
    for (let i = 0; i < 365; i++) {
      const d = new Date(today);
      d.setDate(d.getDate() - i);
      const iso = toLocalISODate(d);
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
    [...daily].sort((a, b) => b.total_tokens - a.total_tokens).slice(0, 10),
  );

  const busiestDay = $derived.by(() => {
    if (daily.length === 0) return null;
    return daily.reduce(
      (max, d) => (d.total_tokens > max.total_tokens ? d : max),
      daily[0],
    );
  });

  const mostRequestsDay = $derived.by(() => {
    if (daily.length === 0) return null;
    return daily.reduce(
      (max, d) => (d.request_count > max.request_count ? d : max),
      daily[0],
    );
  });

  // ── today / per-model stats ──

  const activeSession = $derived(getActiveSession());

  /** model_id of the active session's model (falls back to default model) */
  const currentModelId = $derived.by(() => {
    const key = activeSession?.model_key;
    if (key) {
      const m = configuredModels.find((m) => m.name === key);
      if (m) return m.model_id;
    }
    return config?.model ?? null;
  });

  const modelTotal = (u: ModelUsage) => u.prompt_tokens + u.completion_tokens;

  const todayTotals = $derived.by(() =>
    todayUsage.reduce(
      (acc, u) => ({
        prompt_tokens: acc.prompt_tokens + u.prompt_tokens,
        completion_tokens: acc.completion_tokens + u.completion_tokens,
        cached_tokens: acc.cached_tokens + u.cached_tokens,
        request_count: acc.request_count + u.request_count,
      }),
      {
        prompt_tokens: 0,
        completion_tokens: 0,
        cached_tokens: 0,
        request_count: 0,
      },
    ),
  );

  const todayGrandTotal = $derived(
    todayTotals.prompt_tokens + todayTotals.completion_tokens,
  );

  /** per-model usage over the full range, keyed by `model_id:provider` */
  const usageRangeByModel = $derived.by(() => {
    const map = new Map<string, ModelUsage>();
    for (const u of allModelUsage)
      map.set(modelUsageKey(u.model, u.provider), u);
    return map;
  });

  const todayLabel = $derived(
    new Date().toLocaleDateString(undefined, {
      weekday: "short",
      month: "short",
      day: "numeric",
    }),
  );

  // ── helpers ──

  function formatNumber(n: number): string {
    if (n >= 1000000) return `${(n / 1000000).toFixed(2)}M`;
    if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
    return `${n}`;
  }

  function cacheRate(d: {
    prompt_tokens: number;
    cached_tokens: number;
  }): string {
    if (d.prompt_tokens === 0) return "0.0%";
    return `${((d.cached_tokens / d.prompt_tokens) * 100).toFixed(1)}%`;
  }

  function cacheRateClass(d: {
    prompt_tokens: number;
    cached_tokens: number;
  }): string {
    return d.prompt_tokens > 0 && d.cached_tokens / d.prompt_tokens > 0.5
      ? "bg-success/15 text-success"
      : "bg-muted text-muted-foreground";
  }

  /** Join key matching the backend `GROUP BY model, provider` */
  function modelUsageKey(model: string, provider: string): string {
    return `${model}:${provider}`;
  }

  function formatDateLabel(d: string): string {
    const date = new Date(d + "T00:00:00");
    const now = new Date();
    if (date.toDateString() === now.toDateString()) return "Today";
    return date.toLocaleDateString(undefined, {
      weekday: "short",
      month: "short",
      day: "numeric",
    });
  }

  function formatFullDate(d: string): string {
    const date = new Date(d + "T00:00:00");
    return date.toLocaleDateString(undefined, {
      year: "numeric",
      month: "long",
      day: "numeric",
      weekday: "long",
    });
  }

  function toLocalISODate(d: Date): string {
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
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
      const iso = toLocalISODate(cur);
      result.push(
        map.get(iso) ?? {
          date: iso,
          prompt_tokens: 0,
          completion_tokens: 0,
          cached_tokens: 0,
          total_tokens: 0,
          request_count: 0,
          models: [],
        },
      );
    }
    return result;
  });

  // ── echarts ──

  /** Resolve theme colors from the CSS variables on <body> so the chart
   *  always tracks the active theme (`hsl(var(--x))` tokens in app.css). */
  function chartTheme() {
    const style = getComputedStyle(document.body);
    const hsl = (name: string) => `hsl(${style.getPropertyValue(name).trim()})`;
    const alpha = (name: string, opacity: number) =>
      `hsl(${style.getPropertyValue(name).trim()} / ${opacity})`;
    return {
      heat: [
        hsl("--secondary"),
        alpha("--primary", 0.22),
        alpha("--primary", 0.42),
        alpha("--primary", 0.68),
        hsl("--primary"),
      ],
      cellBorder: hsl("--card"),
      text: hsl("--muted-foreground"),
      cache: hsl("--success"),
      tooltipBg: hsl("--popover"),
      tooltipBorder: hsl("--border"),
      tooltipText: hsl("--popover-foreground"),
      emphasisShadow: alpha("--foreground", 0.28),
    };
  }

  function buildChartOption(data: DayData[]): echarts.EChartsOption {
    const theme = chartTheme();

    const activeTokens = data
      .map((d) => d.total_tokens)
      .filter((tokens) => tokens > 0)
      .sort((a, b) => a - b);
    const quantile = (ratio: number) => {
      if (activeTokens.length === 0) return 1;
      return activeTokens[
        Math.min(
          activeTokens.length - 1,
          Math.floor((activeTokens.length - 1) * ratio),
        )
      ];
    };
    const thresholds = [quantile(0.25), quantile(0.5), quantile(0.75)];
    const intensity = (tokens: number) => {
      if (tokens === 0) return 0;
      if (tokens <= thresholds[0]) return 1;
      if (tokens <= thresholds[1]) return 2;
      if (tokens <= thresholds[2]) return 3;
      return 4;
    };
    const chartData = data.map((d) => [
      d.date,
      d.total_tokens,
      intensity(d.total_tokens),
    ]);
    const start = data[0]?.date ?? "";
    const end = data[data.length - 1]?.date ?? "";

    return {
      backgroundColor: "transparent",
      tooltip: {
        trigger: "item",
        appendToBody: true,
        enterable: true,
        hideDelay: 250,
        transitionDuration: 0.15,
        backgroundColor: theme.tooltipBg,
        borderColor: theme.tooltipBorder,
        textStyle: { color: theme.tooltipText, fontSize: 12 },
        extraCssText:
          "border-radius: 0.375rem; box-shadow: 0 8px 24px rgba(0,0,0,0.14); padding: 10px 12px; z-index: 9999;",
        formatter: (params: unknown) => {
          const [date] = (params as { value: [string, ...unknown[]] }).value;
          const day = data.find((d) => d.date === date);
          if (!day) return date;
          const total = day.prompt_tokens + day.completion_tokens;
          let html = `<div style="font-weight:600;margin-bottom:6px;">${formatFullDate(day.date)}</div>`;
          if (total > 0) {
            html += `<div style="font-size:16px;font-weight:700;margin-bottom:6px;">${formatNumber(total)} <span style="font-size:11px;font-weight:400;opacity:.7">tokens</span></div>`;
            html += `<div style="display:grid;grid-template-columns:auto auto;gap:2px 14px;font-size:11px;opacity:.85"><span>Prompt</span><b style="text-align:right">${formatNumber(day.prompt_tokens)}</b>`;
            html += `<span>Cached</span><b style="text-align:right;color:${theme.cache}">${formatNumber(day.cached_tokens)} · ${cacheRate(day)}</b>`;
            html += `<span>Completion</span><b style="text-align:right">${formatNumber(day.completion_tokens)}</b>`;
            html += `<span>Requests</span><b style="text-align:right">${formatNumber(day.request_count)}</b></div>`;
            if (day.models.length)
              html += `<div style="margin-top:6px;font-size:10px;opacity:.65">${day.models.join(", ")}</div>`;
          } else {
            html += `<div style="font-size:11px;opacity:.7">No activity</div>`;
          }
          return html;
        },
      },
      visualMap: {
        show: false,
        min: 0,
        max: 4,
        type: "piecewise",
        dimension: 2,
        pieces: [
          { value: 0, color: theme.heat[0] },
          { value: 1, color: theme.heat[1] },
          { value: 2, color: theme.heat[2] },
          { value: 3, color: theme.heat[3] },
          { value: 4, color: theme.heat[4] },
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
          color: theme.heat[0],
          borderWidth: 3,
          borderColor: theme.cellBorder,
          borderRadius: 2,
        },
        splitLine: { show: false },
        yearLabel: { show: false },
        monthLabel: {
          show: true,
          nameMap: [
            "Jan",
            "Feb",
            "Mar",
            "Apr",
            "May",
            "Jun",
            "Jul",
            "Aug",
            "Sep",
            "Oct",
            "Nov",
            "Dec",
          ],
          color: theme.text,
          fontSize: 9,
          align: "left",
          margin: 4,
        },
        dayLabel: {
          show: true,
          firstDay: 1,
          nameMap: ["", "M", "", "W", "", "F", ""],
          color: theme.text,
          fontSize: 9,
        },
      },
      series: [
        {
          type: "heatmap",
          coordinateSystem: "calendar",
          data: chartData,
          emphasis: {
            disabled: false,
            itemStyle: {
              borderColor: theme.tooltipText,
              borderWidth: 2,
              shadowBlur: 6,
              shadowColor: theme.emphasisShadow,
            },
          },
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
    const totalWeeks = Math.ceil(
      (mondayOffset + data.length + sundayOffset) / 7,
    );
    const cellW = 10;
    const gap = 3; // borderWidth
    const labelW = 32; // dayLabel space on left
    const rightPad = 4;
    return labelW + totalWeeks * (cellW + gap) + rightPad;
  }

  function renderChart() {
    if (!chartDiv || filledDaily.length === 0) return;
    // ECharts reads the container size during init, so establish dimensions
    // first. Initializing before these styles are applied emits a zero-size
    // warning and can produce a blank canvas.
    chartDiv.style.height = "128px";
    chartDiv.style.width = `${calcChartWidth(filledDaily)}px`;
    if (chartDiv.clientWidth === 0 || chartDiv.clientHeight === 0) return;
    if (!chartInstance) {
      chartInstance = echarts.init(chartDiv, undefined, { renderer: "canvas" });
    }
    chartInstance.setOption(buildChartOption(filledDaily), true);
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
      const [cfg, sum, d, today, allModels, modelsRes] = await Promise.all([
        api.getConfig(),
        api.getUsageSummary(),
        api.getDailyUsage(DAYS_RANGE),
        api.getTodayModelUsage(),
        api.getModelUsage(DAYS_RANGE),
        api.getModels(),
      ]);
      config = cfg;
      summary = {
        ...sum,
        total_tokens: sum.prompt_tokens + sum.completion_tokens,
      };
      daily = (d ?? []).map((day) => ({
        ...day,
        total_tokens: day.prompt_tokens + day.completion_tokens,
      }));
      todayUsage = today ?? [];
      allModelUsage = allModels ?? [];
      configuredModels = modelsRes?.models ?? [];
    } catch (e: unknown) {
      console.error(
        "[UsagePanel] loadData failed:",
        e instanceof Error ? e.message : e,
      );
    } finally {
      loading = false;
      console.log("[UsagePanel] loadData done, loading=false");
    }
    // Kick off the first page of raw records regardless of scroll position.
    loadMoreRecords();
  }

  // ── raw request records (infinite scroll) ──

  async function loadMoreRecords() {
    if (recordsLoading || recordsDone) return;
    recordsLoading = true;
    try {
      const beforeId =
        records.length > 0 ? records[records.length - 1].id : undefined;
      const batch = await api.getUsageRecords(beforeId, RECORDS_PAGE_SIZE);
      if (batch.length === 0) {
        recordsDone = true;
      } else {
        records = [...records, ...batch];
        if (batch.length < RECORDS_PAGE_SIZE) recordsDone = true;
      }
    } catch (e: unknown) {
      console.error(
        "[UsagePanel] loadMoreRecords failed:",
        e instanceof Error ? e.message : e,
      );
    } finally {
      recordsLoading = false;
    }
  }

  $effect(() => {
    if (!recordsSentinel || !recordsScrollContainer) return;
    const el = recordsSentinel;
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries[0]?.isIntersecting) loadMoreRecords();
      },
      { root: recordsScrollContainer, rootMargin: "200px" },
    );
    observer.observe(el);
    return () => observer.disconnect();
  });

  function formatRecordTime(iso: string): string {
    const d = new Date(iso);
    return d.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  }
</script>

<div class="flex-1 flex flex-col min-w-0 overflow-y-auto">
  <PanelHeader title="Usage" icon={TrendingUp} {onToggleLeftPanel}>
    {#snippet meta()}
      <span class="hidden text-xs text-muted-foreground sm:inline"
        >last 365 days</span
      >
      {#if loading && config}
        <InlineLoadingStatus label="Refreshing" />
      {/if}
    {/snippet}
  </PanelHeader>

  <div class="container mx-auto px-4 lg:px-6">
    {#if loading && !config}
      <PageLoading label="Loading usage data">
        <UsagePageSkeleton />
      </PageLoading>
    {:else if !config}
      <div
        class="flex-1 flex items-center justify-center text-muted-foreground"
      >
        Failed to load usage data
      </div>
    {:else}
      <div class="flex-1 py-6 space-y-6">
        <!-- Today (hero panel) -->
        <div
          class="rounded-md border border-primary/40 bg-primary/5 p-4 space-y-3"
        >
          <div class="flex items-center gap-2">
            <span class="relative flex h-2 w-2">
              <span
                class="animate-ping absolute inline-flex h-full w-full rounded-full bg-success opacity-60"
              ></span>
              <span class="relative inline-flex rounded-full h-2 w-2 bg-success"
              ></span>
            </span>
            <span class="text-sm font-semibold">Today</span>
            <span class="text-xs text-muted-foreground ml-auto"
              >{todayLabel}</span
            >
          </div>

          {#if todayGrandTotal === 0}
            <div class="py-4 text-center text-sm text-muted-foreground">
              No activity yet today
            </div>
          {:else}
            <div class="flex flex-col md:flex-row gap-4 md:gap-6">
              <!-- Left: day totals -->
              <div class="md:w-1/3 md:min-w-0 space-y-3">
                <div>
                  <div class="text-3xl font-bold font-mono leading-none">
                    {formatNumber(todayGrandTotal)}
                  </div>
                  <div class="text-xs text-muted-foreground mt-1">
                    tokens today
                  </div>
                </div>
                <div class="flex gap-x-4 gap-y-2 flex-wrap text-xs">
                  <div>
                    <span class="font-mono font-medium text-foreground"
                      >{formatNumber(todayTotals.request_count)}</span
                    >
                    <span class="text-muted-foreground"> req</span>
                  </div>
                  <div>
                    <span class="font-mono font-medium text-foreground"
                      >{formatNumber(todayTotals.prompt_tokens)}</span
                    >
                    <span class="text-muted-foreground">
                      in ({cacheRate(todayTotals)} cached)</span
                    >
                  </div>
                  <div>
                    <span class="font-mono font-medium text-foreground"
                      >{formatNumber(todayTotals.completion_tokens)}</span
                    >
                    <span class="text-muted-foreground"> out</span>
                  </div>
                </div>
              </div>

              <!-- Right: per-model rows -->
              <div
                class="flex-1 min-w-0 space-y-2.5 md:border-l md:border-primary/20 md:pl-6 flex flex-col justify-center"
              >
                {#each todayUsage as usage, i (modelUsageKey(usage.model, usage.provider))}
                  {@const total = modelTotal(usage)}
                  {@const pct =
                    todayGrandTotal > 0
                      ? Math.round((total / todayGrandTotal) * 100)
                      : 0}
                  {@const isCurrent = usage.model === currentModelId}
                  <div class="space-y-1">
                    <div class="flex items-center gap-2 text-xs">
                      {#if i === 0}
                        <span
                          class="w-1.5 h-1.5 rounded-full bg-primary shrink-0"
                        ></span>
                      {:else}
                        <span
                          class="w-1.5 h-1.5 rounded-full bg-muted-foreground/50 shrink-0"
                        ></span>
                      {/if}
                      <span
                        class="font-medium truncate text-foreground"
                        title={`${usage.model} (${usage.provider})`}
                        >{usage.model}</span
                      >
                      {#if isCurrent}
                        <span
                          class="px-1 py-px rounded text-[9px] font-medium bg-primary/15 text-primary shrink-0"
                          >current</span
                        >
                      {/if}
                      <span
                        class="ml-auto font-mono text-muted-foreground shrink-0"
                        >{formatNumber(total)} · {pct}%</span
                      >
                    </div>
                    <div class="h-1.5 bg-background/60 overflow-hidden">
                      <div
                        class="h-full transition-all {isCurrent
                          ? 'bg-primary'
                          : 'bg-muted-foreground/40'}"
                        style="width: {pct}%"
                      ></div>
                    </div>
                    <div
                      class="text-[10px] text-muted-foreground flex items-center gap-3"
                    >
                      <span>{formatNumber(usage.request_count)} req</span>
                      <span>{cacheRate(usage)} cache</span>
                      <span class="font-mono"
                        >{formatNumber(usage.prompt_tokens)} in / {formatNumber(
                          usage.completion_tokens,
                        )} out</span
                      >
                    </div>
                  </div>
                {/each}
              </div>
            </div>
          {/if}
        </div>

        <!-- Summary Cards -->
        {#if filteredSummary}
          <div class="grid grid-cols-2 md:grid-cols-3 lg:grid-cols-6 gap-3">
            <div class="rounded-md border border-border bg-card p-3 space-y-1">
              <div
                class="flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <CalendarRange class="w-3.5 h-3.5" />
                This Month
              </div>
              <div class="text-lg font-semibold font-mono">
                {formatNumber(monthSummary.total_tokens)}
              </div>
              <div class="text-[10px] text-muted-foreground">
                since {monthStartLabel} · {formatNumber(
                  monthSummary.request_count,
                )} req
              </div>
            </div>

            <div class="rounded-md border border-border bg-card p-3 space-y-1">
              <div
                class="flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <Zap class="w-3.5 h-3.5" />
                Total
              </div>
              <div class="text-lg font-semibold font-mono">
                {formatNumber(filteredSummary.total_tokens)}
              </div>
              <div class="text-[10px] text-muted-foreground">tokens</div>
            </div>

            <div class="rounded-md border border-border bg-card p-3 space-y-1">
              <div
                class="flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <Hash class="w-3.5 h-3.5" />
                Requests
              </div>
              <div class="text-lg font-semibold font-mono">
                {formatNumber(filteredSummary.request_count)}
              </div>
              <div class="text-[10px] text-muted-foreground">calls</div>
            </div>

            <div class="rounded-md border border-border bg-card p-3 space-y-1">
              <div
                class="flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <ArrowUpRight class="w-3.5 h-3.5" />
                Prompt
              </div>
              <div class="text-lg font-semibold font-mono">
                {formatNumber(filteredSummary.prompt_tokens)}
              </div>
              <div class="text-[10px] text-muted-foreground">
                {cacheRate(filteredSummary)} cache
              </div>
            </div>

            <div class="rounded-md border border-border bg-card p-3 space-y-1">
              <div
                class="flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <ArrowDownLeft class="w-3.5 h-3.5" />
                Completion
              </div>
              <div class="text-lg font-semibold font-mono">
                {formatNumber(filteredSummary.completion_tokens)}
              </div>
              <div class="text-[10px] text-muted-foreground">tokens</div>
            </div>

            <div class="rounded-md border border-border bg-card p-3 space-y-1">
              <div
                class="flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <Activity class="w-3.5 h-3.5" />
                Active
              </div>
              <div class="text-lg font-semibold font-mono">
                {activeDays}<span
                  class="text-sm text-muted-foreground font-normal"
                  >/{daily.length}</span
                >
              </div>
              <div class="text-[10px] text-muted-foreground">days</div>
            </div>
          </div>
        {/if}

        <!-- ECharts Heatmap (square cells) -->
        {#if filledDaily.length > 0}
          <div class="rounded-md border border-border bg-card p-4 space-y-2">
            <div class="flex items-center gap-2">
              <Calendar class="w-4 h-4 text-muted-foreground" />
              <span class="text-sm font-medium">Activity Heatmap</span>
              <span class="text-xs text-muted-foreground ml-auto">
                {activeDays} active · {daily.length} days
              </span>
            </div>
            <div class="flex overflow-x-auto [justify-content:safe_center]">
              <div bind:this={chartDiv} style="height: 128px;"></div>
            </div>
            <div class="flex items-center justify-between gap-3">
              <span class="text-[10px] text-muted-foreground">Token volume</span
              >
              <div
                class="flex items-center gap-1.5 text-[10px] text-muted-foreground"
                aria-label="Heatmap intensity from less to more"
              >
                <span>Less</span>
                <span class="h-2.5 w-2.5 rounded-xs bg-secondary"></span>
                <span class="h-2.5 w-2.5 rounded-xs bg-primary/20"></span>
                <span class="h-2.5 w-2.5 rounded-xs bg-primary/40"></span>
                <span class="h-2.5 w-2.5 rounded-xs bg-primary/70"></span>
                <span class="h-2.5 w-2.5 rounded-xs bg-primary"></span>
                <span>More</span>
              </div>
            </div>
          </div>
        {:else}
          <div
            class="rounded-md border border-border bg-card p-8 text-center text-sm text-muted-foreground"
          >
            No activity data for the last {DAYS_RANGE} days
          </div>
        {/if}

        <!-- Streak + Highlights row -->
        {#if daily.length > 0}
          <div class="grid grid-cols-2 md:grid-cols-4 gap-3">
            <div
              class="rounded-md border border-border bg-card p-3 space-y-1.5"
            >
              <div
                class="flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <Flame class="w-3.5 h-3.5 text-warning" />
                Current Streak
              </div>
              <div class="text-2xl font-bold font-mono">
                {streaks.current}<span
                  class="text-sm font-normal text-muted-foreground">d</span
                >
              </div>
              <div class="text-[10px] text-muted-foreground">
                consecutive days
              </div>
            </div>

            <div
              class="rounded-md border border-border bg-card p-3 space-y-1.5"
            >
              <div
                class="flex items-center gap-1.5 text-xs text-muted-foreground"
              >
                <Award class="w-3.5 h-3.5 text-warning" />
                Longest Streak
              </div>
              <div class="text-2xl font-bold font-mono">
                {streaks.longest}<span
                  class="text-sm font-normal text-muted-foreground">d</span
                >
              </div>
              <div class="text-[10px] text-muted-foreground">best ever</div>
            </div>

            {#if busiestDay && busiestDay.total_tokens > 0}
              <div
                class="rounded-md border border-border bg-card p-3 space-y-1.5"
              >
                <div
                  class="flex items-center gap-1.5 text-xs text-muted-foreground"
                >
                  <Zap class="w-3.5 h-3.5 text-primary" />
                  Busiest Day
                </div>
                <div class="text-sm font-medium">
                  {formatDateLabel(busiestDay.date)}
                </div>
                <div class="text-xs text-muted-foreground">
                  {formatNumber(busiestDay.total_tokens)} tokens · {formatNumber(
                    busiestDay.request_count,
                  )} req
                </div>
              </div>
            {/if}

            {#if mostRequestsDay && mostRequestsDay.request_count > 0}
              <div
                class="rounded-md border border-border bg-card p-3 space-y-1.5"
              >
                <div
                  class="flex items-center gap-1.5 text-xs text-muted-foreground"
                >
                  <Hash class="w-3.5 h-3.5 text-info" />
                  Most Requests
                </div>
                <div class="text-sm font-medium">
                  {formatDateLabel(mostRequestsDay.date)}
                </div>
                <div class="text-xs text-muted-foreground">
                  {formatNumber(mostRequestsDay.request_count)} req · {formatNumber(
                    mostRequestsDay.total_tokens,
                  )} tok
                </div>
              </div>
            {/if}
          </div>
        {/if}

        <!-- Top Days Table -->
        {#if topDays.length > 0}
          <div class="rounded-md border border-border bg-card overflow-hidden">
            <div
              class="flex items-center gap-2 px-4 py-3 border-b border-border"
            >
              <BarChart3 class="w-4 h-4 text-muted-foreground" />
              <span class="text-sm font-medium">Top Days by Volume</span>
              <span class="text-xs text-muted-foreground ml-auto"
                >{topDays.length} of {daily.length} days</span
              >
            </div>
            <div class="overflow-x-auto">
              <table class="w-full text-sm">
                <thead>
                  <tr class="border-b border-border bg-muted/30">
                    <th
                      class="text-left px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Date</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Requests</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Prompt</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Cached</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Completion</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Total</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Cache</th
                    >
                    <th
                      class="text-left px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Models</th
                    >
                  </tr>
                </thead>
                <tbody>
                  {#each topDays as day, i (day.date)}
                    <tr
                      class="border-b border-border last:border-0 {i % 2 === 0
                        ? 'bg-background'
                        : 'bg-muted/20'} hover:bg-muted/40 transition-colors"
                    >
                      <td class="px-4 py-2 whitespace-nowrap">
                        <div class="text-sm font-medium">
                          {formatDateLabel(day.date)}
                        </div>
                        <div class="text-[10px] text-muted-foreground">
                          {day.date}
                        </div>
                      </td>
                      <td class="px-4 py-2 text-right font-mono"
                        >{formatNumber(day.request_count)}</td
                      >
                      <td class="px-4 py-2 text-right font-mono"
                        >{formatNumber(day.prompt_tokens)}</td
                      >
                      <td class="px-4 py-2 text-right font-mono text-success"
                        >{formatNumber(day.cached_tokens)}</td
                      >
                      <td class="px-4 py-2 text-right font-mono"
                        >{formatNumber(day.completion_tokens)}</td
                      >
                      <td class="px-4 py-2 text-right font-mono font-medium"
                        >{formatNumber(day.total_tokens)}</td
                      >
                      <td class="px-4 py-2 text-right">
                        <span
                          class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium {cacheRateClass(
                            day,
                          )}"
                        >
                          {cacheRate(day)}
                        </span>
                      </td>
                      <td class="px-4 py-2">
                        <div class="flex flex-wrap gap-1">
                          {#each day.models as model (model)}
                            <span
                              class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] bg-secondary text-secondary-foreground"
                              >{model}</span
                            >
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

        <!-- Models (all configured + totals) -->
        {#if configuredModels.length > 0}
          <div class="rounded-md border border-border bg-card overflow-hidden">
            <div
              class="flex items-center gap-2 px-4 py-3 border-b border-border"
            >
              <Cpu class="w-4 h-4 text-muted-foreground" />
              <span class="text-sm font-medium">Models</span>
              <span class="text-xs text-muted-foreground ml-auto"
                >{configuredModels.length} configured · totals over {DAYS_RANGE}
                days</span
              >
            </div>
            <div class="overflow-x-auto">
              <table class="w-full text-sm">
                <thead>
                  <tr class="border-b border-border bg-muted/30">
                    <th
                      class="text-left px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Name</th
                    >
                    <th
                      class="text-left px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Model ID</th
                    >
                    <th
                      class="text-left px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Provider</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Context</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Requests</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Tokens</th
                    >
                    <th
                      class="text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Cache</th
                    >
                  </tr>
                </thead>
                <tbody>
                  {#each configuredModels as m, i (m.name)}
                    {@const usage = usageRangeByModel.get(
                      modelUsageKey(m.model_id, m.provider),
                    )}
                    {@const total = usage ? modelTotal(usage) : 0}
                    <tr
                      class="border-b border-border last:border-0 {i % 2 === 0
                        ? 'bg-background'
                        : 'bg-muted/20'} hover:bg-muted/40 transition-colors"
                    >
                      <td class="px-4 py-2 whitespace-nowrap">
                        <div class="flex items-center gap-1.5">
                          <span class="font-medium">{m.name}</span>
                          {#if m.model_id === config?.model}
                            <span
                              class="px-1 py-px rounded text-[9px] font-medium bg-secondary text-muted-foreground"
                              >default</span
                            >
                          {/if}
                          {#if m.model_id === currentModelId}
                            <span
                              class="px-1 py-px rounded text-[9px] font-medium bg-primary/15 text-primary"
                              >current</span
                            >
                          {/if}
                        </div>
                      </td>
                      <td class="px-4 py-2 font-mono text-xs">{m.model_id}</td>
                      <td class="px-4 py-2 text-xs text-muted-foreground"
                        >{m.provider}</td
                      >
                      <td class="px-4 py-2 text-right font-mono text-xs"
                        >{formatNumber(m.context_window)}</td
                      >
                      <td class="px-4 py-2 text-right font-mono text-xs">
                        {usage ? formatNumber(usage.request_count) : "—"}
                      </td>
                      <td
                        class="px-4 py-2 text-right font-mono text-xs font-medium"
                      >
                        {usage ? formatNumber(total) : "—"}
                      </td>
                      <td class="px-4 py-2 text-right text-xs">
                        {#if usage}
                          <span
                            class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium {cacheRateClass(
                              usage,
                            )}"
                          >
                            {cacheRate(usage)}
                          </span>
                        {:else}
                          <span class="text-muted-foreground">—</span>
                        {/if}
                      </td>
                    </tr>
                  {/each}
                </tbody>
              </table>
            </div>
          </div>
        {/if}

        <!-- Raw request records -->
        <div class="rounded-md border border-border bg-card overflow-hidden">
          <div class="flex items-center gap-2 px-4 py-3 border-b border-border">
            <Hash class="w-4 h-4 text-muted-foreground" />
            <span class="text-sm font-medium">Requests</span>
            <span class="text-xs text-muted-foreground ml-auto">
              {records.length} loaded · newest first
            </span>
          </div>

          {#if records.length === 0 && !recordsLoading}
            <div class="p-8 text-center text-sm text-muted-foreground">
              No request records
            </div>
          {:else if records.length > 0}
            <div
              bind:this={recordsScrollContainer}
              class="max-h-96 overflow-y-auto overflow-x-auto"
            >
              <table class="w-full text-sm">
                <thead>
                  <tr class="border-b border-border bg-muted/30">
                    <th
                      class="sticky top-0 z-10 bg-card text-left px-4 py-2 text-xs font-medium text-muted-foreground"
                      >ID</th
                    >
                    <th
                      class="sticky top-0 z-10 bg-card text-left px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Time</th
                    >
                    <th
                      class="sticky top-0 z-10 bg-card text-left px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Model</th
                    >
                    <th
                      class="sticky top-0 z-10 bg-card text-left px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Type</th
                    >
                    <th
                      class="sticky top-0 z-10 bg-card text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Prompt</th
                    >
                    <th
                      class="sticky top-0 z-10 bg-card text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Cached</th
                    >
                    <th
                      class="sticky top-0 z-10 bg-card text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Cache</th
                    >
                    <th
                      class="sticky top-0 z-10 bg-card text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Completion</th
                    >
                    <th
                      class="sticky top-0 z-10 bg-card text-right px-4 py-2 text-xs font-medium text-muted-foreground"
                      >Total</th
                    >
                  </tr>
                </thead>
                <tbody>
                  {#each records as r, i (r.id)}
                    {@const total = r.prompt_tokens + r.completion_tokens}
                    <tr
                      class="border-b border-border last:border-0 {i % 2 === 0
                        ? 'bg-background'
                        : 'bg-muted/20'} hover:bg-muted/40 transition-colors"
                    >
                      <td class="px-4 py-2 whitespace-nowrap">
                        <span
                          class="font-mono text-[10px] text-muted-foreground"
                          >{r.id}</span
                        >
                      </td>
                      <td class="px-4 py-2 whitespace-nowrap text-xs">
                        {formatRecordTime(r.created_at)}
                      </td>
                      <td class="px-4 py-2">
                        <div class="text-xs font-medium">{r.model}</div>
                        <div class="text-[10px] text-muted-foreground">
                          {r.provider}
                        </div>
                      </td>
                      <td class="px-4 py-2">
                        <span
                          class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium {r.usage_type ===
                          'compactor'
                            ? 'bg-warning/15 text-warning'
                            : r.usage_type === 'subagent'
                              ? 'bg-info/15 text-info'
                              : 'bg-secondary text-secondary-foreground'}"
                        >
                          {r.usage_type}
                        </span>
                      </td>
                      <td class="px-4 py-2 text-right font-mono text-xs"
                        >{formatNumber(r.prompt_tokens)}</td
                      >
                      <td
                        class="px-4 py-2 text-right font-mono text-xs text-success"
                        >{formatNumber(r.cached_tokens)}</td
                      >
                      <td class="px-4 py-2 text-right">
                        <span
                          class="inline-flex items-center px-1.5 py-0.5 rounded text-[10px] font-medium {cacheRateClass(
                            r,
                          )}"
                        >
                          {cacheRate(r)}
                        </span>
                      </td>
                      <td class="px-4 py-2 text-right font-mono text-xs"
                        >{formatNumber(r.completion_tokens)}</td
                      >
                      <td
                        class="px-4 py-2 text-right font-mono text-xs font-medium"
                        >{formatNumber(total)}</td
                      >
                    </tr>
                  {/each}
                </tbody>
              </table>

              <!-- infinite scroll sentinel inside the scrollable table -->
              <div bind:this={recordsSentinel} class="h-1"></div>

              {#if recordsLoading}
                <div class="flex items-center justify-center py-3">
                  <InlineLoadingStatus label="Loading" />
                </div>
              {:else if recordsDone && records.length > 0}
                <div
                  class="py-3 text-center text-xs text-muted-foreground border-t border-border"
                >
                  No more records
                </div>
              {/if}
            </div>
          {/if}

          <!-- sentinel for empty state (observer needs it to fire the first load) -->
          {#if records.length === 0 && recordsLoading}
            <div class="flex items-center justify-center py-3">
              <InlineLoadingStatus label="Loading" />
            </div>
          {/if}
        </div>
      </div>
    {/if}
  </div>
</div>
