<script lang="ts">
  import { ChevronDown, ChevronRight } from "lucide-svelte";

  let { content, elapsedMs, isStreaming = false }: { content: string; elapsedMs: number; isStreaming?: boolean } = $props();

  let expanded = $state(false);
  let userToggled = $state(false);

  // Auto-expand while streaming, auto-fold when streaming ends
  $effect(() => {
    if (isStreaming) {
      expanded = true;
      userToggled = false;
    } else if (!userToggled) {
      expanded = false;
    }
  });

  function formatElapsed(ms: number): string {
    if (ms < 1000) return `${ms}ms`;
    return `${(ms / 1000).toFixed(1)}s`;
  }

  function tokenEstimate(text: string): string {
    const n = Math.round(text.length / 4);
    if (n >= 1000) return `~${(n / 1000).toFixed(1)}k`;
    return `~${n}`;
  }

  function toggle() {
    expanded = !expanded;
    userToggled = true;
  }
</script>

<div class="text-xs text-muted-foreground">
  <button
    type="button"
    class="flex items-center gap-1.5 hover:text-muted-foreground/80 transition-colors cursor-pointer"
    onclick={toggle}
  >
    <span class="font-mono">Thinking</span>
    {#if elapsedMs > 0}
      <span class="text-muted-foreground/50">· {formatElapsed(elapsedMs)}</span>
    {/if}
    <span class="text-muted-foreground/50">· {tokenEstimate(content)} tokens</span>
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
