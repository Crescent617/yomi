<script lang="ts">
  import { ChevronDown, ChevronRight } from "lucide-svelte";
  import { formatElapsed, tokenEstimate } from "../../utils";

  let { content, elapsed_ms }: { content: string; elapsed_ms: number } = $props();

  let expanded = $state(false);
</script>

<div class="text-xs text-muted-foreground">
  <button
    type="button"
    class="flex items-center gap-1.5 hover:text-muted-foreground/80 transition-colors cursor-pointer"
    onclick={() => expanded = !expanded}
  >
    <span class="font-mono">Thinking</span>
    {#if elapsed_ms > 0}
      <span class="text-muted-foreground/50">· {formatElapsed(elapsed_ms)}</span>
    {/if}
    <span class="text-muted-foreground/50">· {tokenEstimate(content)} tokens</span>
    {#if expanded}
      <ChevronDown class="w-3 h-3" />
    {:else}
      <ChevronRight class="w-3 h-3" />
    {/if}
  </button>
  {#if expanded}
    <div class="mt-2 max-h-60 overflow-y-auto rounded bg-muted/50 px-3 py-2 text-xs text-muted-foreground/70">
      <pre class="whitespace-pre-wrap">{content}</pre>
    </div>
  {/if}
</div>
