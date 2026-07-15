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
  class="relative flex min-w-0 items-start gap-2 rounded-md bg-info/5 px-2.5 py-1.5"
  aria-label="Steer message"
  title="Steer message"
>
  <div class="flex w-4 shrink-0 flex-col items-center">
    {#if parsed.source?.type === "agent"}
      <Bot class="mt-0.5 size-3.5 text-info" aria-hidden="true" />
    {:else if parsed.source?.type === "shell"}
      <Terminal class="mt-0.5 size-3.5 text-info" aria-hidden="true" />
    {:else}
      <Mail class="mt-0.5 size-3.5 text-info" aria-hidden="true" />
    {/if}
  </div>
  <div
    class="min-w-0 flex-1 text-xs leading-4 text-foreground"
    class:pb-3={isLong && expanded}
  >
    {#if parsed.source?.type === "agent"}
      <button
        type="button"
        class="mb-0.5 inline-flex max-w-full items-center gap-1 rounded-sm text-[11px] font-medium text-info transition-colors hover:text-info/80 hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        onclick={() => void activateSession(parsed.source!.id)}
        title={`Open agent session ${parsed.source.id}`}
        aria-label={`Open agent session ${parsed.source.id}`}
      >
        <span class="truncate font-mono">{parsed.source.id}</span>
      </button>
    {:else if parsed.source?.type === "shell"}
      <span
        class="mb-0.5 inline-flex max-w-full items-center gap-1 text-[11px] font-medium text-info"
        title={`Background shell ${parsed.source.id}`}
      >
        <span class="truncate font-mono">{parsed.source.id}</span>
      </span>
    {/if}
    <div class="relative min-w-0" class:message-collapsed={isLong && !expanded}>
      <div class="min-w-0" class:truncate={!expanded} bind:this={textElement}>
        <UserText text={parsed.content} compact />
      </div>
      {#if isLong && !expanded}
        <div
          class="pointer-events-none absolute inset-x-0 bottom-0 h-10 bg-linear-to-t from-info/5 to-transparent"
          aria-hidden="true"
        ></div>
      {/if}
    </div>
  </div>
  {#if isLong}
    <button
      type="button"
      class="absolute bottom-0 left-1/2 z-10 grid h-5 w-10 -translate-x-1/2 place-items-center bg-linear-to-r from-transparent via-info/5 to-transparent text-info/70 transition-colors hover:text-info focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      onclick={() => (expanded = !expanded)}
      aria-expanded={expanded}
      aria-label={expanded ? "Collapse steer message" : "Expand steer message"}
      title={expanded ? "Collapse" : "Expand"}
    >
      <span class="inline-flex" class:expand-hint={!expanded}>
        <ChevronDown
          class="size-4 transition-transform duration-150 {expanded
            ? 'rotate-180'
            : ''}"
          aria-hidden="true"
        />
      </span>
    </button>
  {/if}
  {#if created_at}
    <time
      datetime={created_at}
      class="ml-auto shrink-0 text-[10px] leading-4 tabular-nums text-muted-foreground/70"
    >
      {formatMessageTime(created_at)}
    </time>
  {/if}
</div>

<style>
  .truncate {
    max-height: 2rem;
    overflow: hidden;
  }

  .message-collapsed {
    margin-bottom: -0.125rem;
  }

  .expand-hint {
    animation: expand-hint 1.8s ease-in-out infinite;
  }

  @keyframes expand-hint {
    0%,
    100% {
      transform: translateY(-1px);
    }
    50% {
      transform: translateY(1px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .expand-hint {
      animation: none;
    }
  }
</style>
