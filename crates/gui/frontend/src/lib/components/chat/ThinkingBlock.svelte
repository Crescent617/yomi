<script lang="ts">
  import { ChevronDown, ChevronRight, Lightbulb } from "lucide-svelte";
  import { formatElapsed, tokenEstimate } from "../../utils";
  import { thinkingPreview } from "./thinking-block";

  let {
    content,
    elapsed_ms,
    isRunning = false,
    isFirst = false,
    isLast = false,
  }: {
    content: string;
    elapsed_ms: number;
    isRunning?: boolean;
    isFirst?: boolean;
    isLast?: boolean;
  } = $props();

  let expanded = $state(false);
  const preview = $derived(thinkingPreview(content));
</script>

<div class="relative flex gap-1">
  <div class="relative w-3 shrink-0 pt-0.5" aria-hidden="true">
    {#if !(isFirst && isLast)}
      <span
        class="absolute left-1/2 w-px -translate-x-1/2 bg-border/70 {isFirst
          ? 'bottom-0 top-[16px]'
          : isLast
            ? 'bottom-[calc(100%-16px)] top-0'
            : 'inset-y-0'}"
      ></span>
    {/if}
    <span class="relative z-10 flex h-7 items-center justify-center">
      <span
        class="flex size-3 items-center justify-center rounded-full border border-border bg-background text-muted-foreground"
      >
        {#if isRunning}
          <span class="relative flex size-1.5 items-center justify-center">
            <span
              class="absolute size-2 animate-ping rounded-full bg-primary/70"
            ></span>
            <span
              class="absolute size-2.5 rounded-full bg-primary/25 blur-[2px]"
            ></span>
            <span class="relative size-1.5 rounded-full bg-primary"></span>
          </span>
        {:else}
          <span class="size-1 rounded-full bg-muted-foreground/60"></span>
        {/if}
      </span>
    </span>
  </div>

  <div class="min-w-0 flex-1 py-0.5">
    <button
      type="button"
      class="flex min-h-7 w-full items-center gap-2 rounded-md px-0.5 text-left transition-colors hover:bg-secondary/40 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      onclick={() => (expanded = !expanded)}
      aria-expanded={expanded}
    >
      <Lightbulb class="size-3.5 shrink-0 text-muted-foreground" />
      <span class="shrink-0 text-xs font-medium text-foreground">Thought</span>
      <span class="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
        {preview}
      </span>
      <span
        class="shrink-0 font-mono text-[10px] tabular-nums text-muted-foreground"
      >
        {tokenEstimate(content)} tokens
      </span>
      {#if elapsed_ms > 0}
        <span class="shrink-0 text-[11px] tabular-nums text-muted-foreground">
          {formatElapsed(elapsed_ms)}
        </span>
      {/if}
      {#if expanded}
        <ChevronDown class="size-3.5 shrink-0 text-muted-foreground" />
      {:else}
        <ChevronRight class="size-3.5 shrink-0 text-muted-foreground" />
      {/if}
    </button>

    {#if expanded}
      <pre
        class="mt-1 max-h-60 overflow-y-auto whitespace-pre-wrap px-0.5 py-1 text-xs leading-relaxed text-muted-foreground">{content}</pre>
    {/if}
  </div>
</div>
