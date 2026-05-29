<script lang="ts">
  let { tool }: { tool: { name: string; input: unknown; output?: unknown } } = $props();
  let open = $state(false);
</script>

<div class="rounded-lg border border-border bg-muted/50 my-2">
  <button
    class="flex items-center gap-2 w-full px-3 py-2 text-sm hover:bg-muted/80 transition-colors"
    onclick={() => open = !open}
  >
    <span class="text-muted-foreground">🔧</span>
    <span class="font-medium">{tool.name}</span>
    <span class="ml-auto text-xs text-muted-foreground">{open ? '▲' : '▼'}</span>
  </button>
  {#if open}
    <div class="px-3 pb-3 space-y-2">
      <div>
        <div class="text-xs text-muted-foreground mb-1">Input</div>
        <pre class="text-xs bg-background rounded p-2 overflow-x-auto">{JSON.stringify(tool.input, null, 2)}</pre>
      </div>
      {#if tool.output}
        <div>
          <div class="text-xs text-muted-foreground mb-1">Output</div>
          <pre class="text-xs bg-background rounded p-2 overflow-x-auto">{JSON.stringify(tool.output, null, 2)}</pre>
        </div>
      {/if}
    </div>
  {/if}
</div>
