<script lang="ts">
  import { Bell } from "lucide-svelte";
  import { parseUserText } from "./user-text";

  let { text, compact = false }: { text: string; compact?: boolean } = $props();
  const segments = $derived(parseUserText(text));

  let tooltip = $state<HTMLDivElement | null>(null);
  let tooltipContent = $state("");
  let tooltipLeft = $state(0);
  let tooltipTop = $state(0);
  let tooltipBelow = $state(false);

  function showTooltip(event: MouseEvent | FocusEvent, content: string) {
    const anchor = event.currentTarget as HTMLElement;
    const rect = anchor.getBoundingClientRect();
    tooltipContent = content;
    tooltipLeft = Math.min(
      Math.max(rect.left + rect.width / 2, 152),
      window.innerWidth - 152,
    );
    tooltipBelow = rect.top < 120;
    tooltipTop = tooltipBelow ? rect.bottom + 6 : rect.top - 6;
    tooltip?.showPopover();
  }

  function hideTooltip() {
    if (tooltip?.matches(":popover-open")) tooltip.hidePopover();
  }
</script>

<svelte:window onscroll={hideTooltip} onresize={hideTooltip} />

<span class="whitespace-pre-wrap [overflow-wrap:anywhere]">
  {#each segments as segment, index (`${segment.type}-${index}`)}
    {#if segment.type === "text"}
      {segment.content}
    {:else}
      <button
        type="button"
        class="inline-flex cursor-help items-center gap-1 rounded-full border border-info/20 bg-info/10 px-1.5 py-0.5 text-[10px] font-medium leading-none text-info outline-none transition-colors hover:bg-info/15 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"
        class:mx-0.5={!compact}
        aria-label={`System reminder: ${segment.content}`}
        onmouseenter={(event) => showTooltip(event, segment.content)}
        onmouseleave={hideTooltip}
        onfocus={(event) => showTooltip(event, segment.content)}
        onblur={hideTooltip}
      >
        <Bell class="size-2.5" aria-hidden="true" />
        <span>Reminder</span>
      </button>
    {/if}
  {/each}
</span>

<div
  bind:this={tooltip}
  popover="manual"
  role="tooltip"
  class="fixed z-50 m-0 w-max max-w-72 -translate-x-1/2 whitespace-pre-wrap rounded-md border border-border bg-popover px-2.5 py-2 text-left text-xs font-normal leading-relaxed text-popover-foreground shadow-md"
  class:-translate-y-full={!tooltipBelow}
  style:left={`${tooltipLeft}px`}
  style:top={`${tooltipTop}px`}
>
  {tooltipContent}
</div>
