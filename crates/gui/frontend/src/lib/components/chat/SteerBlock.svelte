<script lang="ts">
  import { Bot, ChevronDown, Mail, Terminal } from "lucide-svelte";
  import { activateSession } from "../../session";
  import { formatMessageTime } from "../../utils";
  import { parseSteerMessage } from "./steer-message";
  import UserText from "./UserText.svelte";

  let {
    content,
    created_at,
  }: {
    content: string;
    created_at?: string;
  } = $props();

  let expanded = $state(false);
  let textElement = $state<HTMLDivElement>();
  let isLong = $state(false);
  const parsed = $derived(parseSteerMessage(content));

  $effect(() => {
    void parsed.content;
    const element = textElement;
    if (!element) return;

    const measureOverflow = () => {
      if (!expanded) {
        isLong = element.scrollHeight > element.clientHeight + 1;
      }
    };
    measureOverflow();

    const observer = new ResizeObserver(measureOverflow);
    observer.observe(element);
    return () => observer.disconnect();
  });
</script>

<div
  class="flex min-w-0 items-start gap-2 rounded-md border-l-2 border-info/40 bg-info/5 px-2.5 py-1.5"
  aria-label="Steer message"
  title="Steer message"
>
  <Mail class="mt-0.5 size-3 shrink-0 text-info" aria-hidden="true" />
  <div class="min-w-0 flex-1 text-xs leading-4 text-foreground">
    {#if parsed.source?.type === "agent"}
      <button
        type="button"
        class="mb-0.5 inline-flex max-w-full items-center gap-1 rounded-sm text-[11px] font-medium text-info transition-colors hover:text-info/80 hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        onclick={() => void activateSession(parsed.source!.id)}
        title={`Open agent session ${parsed.source.id}`}
        aria-label={`Open agent session ${parsed.source.id}`}
      >
        <Bot class="size-3 shrink-0" aria-hidden="true" />
        <span class="truncate font-mono">{parsed.source.id}</span>
      </button>
    {:else if parsed.source?.type === "shell"}
      <span
        class="mb-0.5 inline-flex max-w-full items-center gap-1 text-[11px] font-medium text-info"
        title={`Background shell ${parsed.source.id}`}
      >
        <Terminal class="size-3 shrink-0" aria-hidden="true" />
        <span class="truncate font-mono">{parsed.source.id}</span>
      </span>
    {/if}
    <div class="relative min-w-0" class:message-collapsed={isLong && !expanded}>
      <div class="min-w-0" class:truncate={!expanded} bind:this={textElement}>
        <UserText text={parsed.content} compact />
      </div>
      {#if isLong && !expanded}
        <div
          class="pointer-events-none absolute inset-x-0 bottom-0 h-8 bg-linear-to-t from-info/5 to-transparent"
          aria-hidden="true"
        ></div>
      {/if}
    </div>
  </div>
  <div class="ml-auto flex shrink-0 items-center gap-0.5">
    {#if created_at}
      <time
        datetime={created_at}
        class="text-[10px] leading-4 tabular-nums text-muted-foreground/70"
      >
        {formatMessageTime(created_at)}
      </time>
    {/if}
    {#if isLong}
      <button
        type="button"
        class="grid size-4 place-items-center rounded-sm text-muted-foreground transition-colors hover:bg-info/10 hover:text-info focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        onclick={() => (expanded = !expanded)}
        aria-expanded={expanded}
        aria-label={expanded
          ? "Collapse steer message"
          : "Expand steer message"}
        title={expanded ? "Collapse" : "Expand"}
      >
        <ChevronDown
          class="size-3 transition-transform duration-150 {expanded
            ? 'rotate-180'
            : ''}"
          aria-hidden="true"
        />
      </button>
    {/if}
  </div>
</div>

<style>
  .truncate {
    max-height: 2rem;
    overflow: hidden;
  }

  .message-collapsed {
    margin-bottom: -0.125rem;
  }
</style>
