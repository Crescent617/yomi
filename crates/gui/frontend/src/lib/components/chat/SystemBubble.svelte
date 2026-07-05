<script lang="ts">
  import { Terminal } from "lucide-svelte";
  import type { SystemMessage } from "../../state.svelte";

  let { message }: { message: SystemMessage } = $props();

  let expanded = $state(false);

  const isShort =
    message.content.length < 100 && !message.content.includes("\n");
</script>

<div class="flex justify-center my-1">
  <div class="max-w-[90%]">
    {#if isShort}
      <div
        class="inline-flex items-center gap-1.5 text-xs text-muted-foreground/60 bg-muted/30 rounded-md px-2 py-1"
      >
        <Terminal size={10} />
        <span class="font-mono">{message.content}</span>
      </div>
    {:else}
      <button
        type="button"
        class="inline-flex items-center gap-1.5 text-xs text-muted-foreground/60 hover:text-muted-foreground transition-colors cursor-pointer select-none bg-muted/30 rounded-md px-2 py-1"
        onclick={() => (expanded = !expanded)}
      >
        <Terminal size={10} />
        <span>{expanded ? "Hide" : "System"}</span>
      </button>

      {#if expanded}
        <div
          class="mt-1 rounded-md border border-border/50 bg-muted/30 px-3 py-2 text-xs text-muted-foreground font-mono leading-relaxed whitespace-pre-wrap break-words"
        >
          {message.content}
        </div>
      {/if}
    {/if}
  </div>
</div>
