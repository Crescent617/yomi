<script lang="ts">
  import { ChevronDown, ChevronRight } from "lucide-svelte";

  let { content, elapsedMs }: { content: string; elapsedMs: number } = $props();

  let expanded = $state(false);

  function formatElapsed(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(1)}s`;
  }
</script>

<div class="text-xs text-muted-foreground">
  <button
    type="button"
    class="flex items-center gap-1.5 hover:text-muted-foreground/80 transition-colors cursor-pointer"
    onclick={() => expanded = !expanded}
  >
    <span class="font-mono"></span>
    <span>Thinking</span>
    {#if elapsedMs > 0}
      <span class="text-muted-foreground/50">· {formatElapsed(elapsedMs)}</span>
    {/if}
    {#if expanded}
      <ChevronDown class="w-3 h-3" />
    {:else}
      <ChevronRight class="w-3 h-3" />
    {/if}
  </button>
  {#if expanded}
    <pre class="mt-2 whitespace-pre-wrap text-muted-foreground/70 bg-muted/50 rounded px-3 py-2 text-xs">{content}</pre>
  {/if}
</div>
