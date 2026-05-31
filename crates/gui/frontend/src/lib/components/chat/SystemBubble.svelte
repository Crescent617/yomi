<script lang="ts">
  import { Terminal } from "lucide-svelte";
  import type { ChatMessage } from "../../state.svelte";

  let { message }: { message: ChatMessage } = $props();

  let expanded = $state(false);
</script>

<div class="flex justify-center">
  <div class="max-w-[90%] w-full">
    <button
      type="button"
      class="flex items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors cursor-pointer select-none"
      onclick={() => expanded = !expanded}
    >
      <Terminal size={12} />
      <span>{expanded ? "Hide system prompt" : "System prompt"}</span>
    </button>

    {#if expanded}
      <div
        class="mt-1 rounded-md border border-border bg-muted/50 px-3 py-2 text-xs text-muted-foreground font-mono leading-relaxed whitespace-pre-wrap break-words"
      >
        {message.content}
      </div>
    {/if}
  </div>
</div>
