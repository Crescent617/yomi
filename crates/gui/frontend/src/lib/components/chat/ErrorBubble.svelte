<script lang="ts">
  import { AlertCircle, ChevronDown, ChevronRight } from "lucide-svelte";
  import type { ErrorMessage } from "../../state.svelte";
  import { formatMessageTime } from "../../utils";

  let { messages }: { messages: ErrorMessage[] } = $props();

  let expanded = $state(false);
  const isGroup = $derived(messages.length > 1);
  const latest = $derived(messages.at(-1));
</script>

<div class="w-full max-w-3xl">
  {#if isGroup}
    <div class="overflow-hidden rounded-md bg-secondary/20">
      <button
        type="button"
        class="flex w-full items-center gap-2 px-2.5 py-1.5 text-left transition-colors hover:bg-secondary/40 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
        onclick={() => (expanded = !expanded)}
        aria-expanded={expanded}
      >
        <AlertCircle size={14} class="shrink-0 text-error" />
        <span class="shrink-0 text-xs font-medium text-error">
          {messages.length} errors
        </span>
        <span class="min-w-0 flex-1 truncate text-xs text-muted-foreground">
          {latest?.content}
        </span>
        {#if latest?.created_at}
          <time
            datetime={latest.created_at}
            class="shrink-0 text-[10px] text-muted-foreground"
          >
            {formatMessageTime(latest.created_at)}
          </time>
        {/if}
        {#if expanded}
          <ChevronDown size={13} class="shrink-0 text-muted-foreground" />
        {:else}
          <ChevronRight size={13} class="shrink-0 text-muted-foreground" />
        {/if}
      </button>

      {#if expanded}
        <div class="divide-y divide-border/60 border-t border-border/60">
          {#each messages as message (message.id)}
            <div class="flex items-start gap-2 px-2.5 py-1.5">
              {#if message.created_at}
                <time
                  datetime={message.created_at}
                  class="w-10 shrink-0 pt-0.5 text-[10px] text-muted-foreground"
                >
                  {formatMessageTime(message.created_at)}
                </time>
              {/if}
              <span
                class="min-w-0 whitespace-pre-wrap break-words text-xs leading-relaxed text-foreground/85"
              >
                {message.content}
              </span>
            </div>
          {/each}
        </div>
      {/if}
    </div>
  {:else if latest}
    <div
      class="flex items-start gap-2 rounded-md bg-secondary/20 px-2.5 py-1.5"
    >
      <AlertCircle size={14} class="mt-0.5 shrink-0 text-error" />
      <span
        class="min-w-0 flex-1 whitespace-pre-wrap break-words text-xs leading-relaxed text-foreground/85"
      >
        {latest.content}
      </span>
      {#if latest.created_at}
        <time
          datetime={latest.created_at}
          class="shrink-0 pt-0.5 text-[10px] text-muted-foreground"
        >
          {formatMessageTime(latest.created_at)}
        </time>
      {/if}
    </div>
  {/if}
</div>
