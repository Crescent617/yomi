<script lang="ts">
  let { checkpoints, onRewind }: { checkpoints: any[]; onRewind: (id: string) => void } = $props();
</script>

{#if checkpoints.length > 0}
  <div class="flex items-center gap-1 px-4 py-2 border-t border-border overflow-x-auto">
    <span class="text-xs text-muted-foreground mr-2">Checkpoints:</span>
    {#each checkpoints as cp, i (cp.id)}
      <button
        class="flex items-center gap-1 text-xs px-2 py-1 rounded-full border border-border hover:bg-secondary transition-colors"
        onclick={() => onRewind(cp.id)}
        title="Rewind to {cp.name ?? 'checkpoint ' + (i + 1)}"
      >
        <span class="w-1.5 h-1.5 rounded-full bg-primary"></span>
        {cp.name ?? i + 1}
      </button>
    {/each}
  </div>
{/if}
