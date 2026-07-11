<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import type { ModelUsage } from "../../api";
  import { requestActivePanel } from "../../state.svelte";
  import { Flame, ArrowUpRight, ArrowDownLeft, Hash } from "lucide-svelte";

  let todayUsage = $state<ModelUsage[]>([]);
  let loaded = $state(false);

  onMount(() => {
    api
      .getTodayModelUsage()
      .then((u) => {
        todayUsage = u ?? [];
        loaded = true;
      })
      .catch(() => {
        loaded = true;
      });
  });

  const totals = $derived.by(() =>
    todayUsage.reduce(
      (acc, u) => ({
        prompt: acc.prompt + u.prompt_tokens,
        completion: acc.completion + u.completion_tokens,
        requests: acc.requests + u.request_count,
      }),
      { prompt: 0, completion: 0, requests: 0 },
    ),
  );
  const grandTotal = $derived(totals.prompt + totals.completion);

  const topModel = $derived.by(() => {
    if (todayUsage.length === 0) return null;
    return [...todayUsage].sort(
      (a, b) =>
        b.prompt_tokens +
        b.completion_tokens -
        (a.prompt_tokens + a.completion_tokens),
    )[0];
  });

  function fmt(n: number): string {
    if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
    if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
    return `${n}`;
  }

  function openUsagePanel() {
    requestActivePanel("usage");
  }
</script>

{#if loaded && grandTotal > 0}
  <button
    type="button"
    onclick={openUsagePanel}
    class="group w-full flex items-center gap-4 rounded-md border border-border bg-card/60 px-4 py-2.5
           hover:border-primary/40 hover:bg-card transition-all text-left"
    title="Open usage panel"
  >
    <span
      class="flex items-center justify-center w-8 h-8 rounded-md bg-primary/10 text-primary shrink-0"
    >
      <Flame class="w-4 h-4" />
    </span>
    <span class="flex items-baseline gap-1.5 shrink-0">
      <span class="text-lg font-bold font-mono leading-none"
        >{fmt(grandTotal)}</span
      >
      <span class="text-[11px] text-muted-foreground">tokens today</span>
    </span>
    <span
      class="hidden sm:flex items-center gap-3 text-[11px] text-muted-foreground min-w-0"
    >
      <span class="inline-flex items-center gap-1 shrink-0">
        <Hash class="w-3 h-3" />{fmt(totals.requests)} req
      </span>
      <span class="inline-flex items-center gap-1 shrink-0">
        <ArrowUpRight class="w-3 h-3" />{fmt(totals.prompt)} in
      </span>
      <span class="inline-flex items-center gap-1 shrink-0">
        <ArrowDownLeft class="w-3 h-3" />{fmt(totals.completion)} out
      </span>
      {#if topModel}
        <span class="truncate font-mono opacity-70" title={topModel.model}>
          {topModel.model}
        </span>
      {/if}
    </span>
    <span
      class="ml-auto text-[11px] text-muted-foreground opacity-0 group-hover:opacity-100 transition-opacity shrink-0"
    >
      View details →
    </span>
  </button>
{/if}
