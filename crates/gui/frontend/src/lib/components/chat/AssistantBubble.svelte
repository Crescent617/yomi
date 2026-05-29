<script lang="ts">
  import type { ChatMessage } from "../../state.svelte";

  let { message }: { message: ChatMessage } = $props();
</script>

<div class="flex gap-3 max-w-[85%]">
  <div
    class="shrink-0 w-8 h-8 rounded-full bg-primary flex items-center justify-center text-primary-foreground text-sm font-bold"
  >
    Y
  </div>
  <div class="flex-1 space-y-2">
    <div
      class="rounded-2xl rounded-tl-sm bg-card border border-border px-4 py-3 text-sm"
    >
      {message.content}
    </div>
    {#if message.thinking}
      <div class="text-xs text-muted-foreground bg-muted rounded-lg px-3 py-2">
        <details>
          <summary>Thinking ({message.thinking.elapsedMs}ms)</summary>
          <pre class="mt-1 whitespace-pre-wrap">{message.thinking.content}</pre>
        </details>
      </div>
    {/if}
  </div>
</div>
