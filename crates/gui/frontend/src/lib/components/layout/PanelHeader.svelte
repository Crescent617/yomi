<script lang="ts">
  import type { ComponentType, Snippet, SvelteComponent } from "svelte";
  import SidebarToggle from "./SidebarToggle.svelte";

  /**
   * Shared panel header: h-14 bar with border, mobile sidebar toggle,
   * icon + title (+ optional inline meta) on the left, actions on the right.
   */
  interface Props {
    title: string;
    /** Lucide icon component. */
    icon: ComponentType<SvelteComponent<{ class?: string }>>;
    /** Extra classes for the icon (e.g. "text-warning fill-current"). */
    iconClass?: string;
    onToggleLeftPanel?: () => void;
    /** Inline content after the title (counts, segmented toggles, …). */
    meta?: Snippet;
    /** Right-aligned header actions (soft buttons per DESIGN.md). */
    actions?: Snippet;
  }

  let {
    title,
    icon: Icon,
    iconClass = "text-primary",
    onToggleLeftPanel,
    meta,
    actions,
  }: Props = $props();
</script>

<header
  class="flex h-14 shrink-0 items-center justify-between gap-3 border-b border-border px-4 lg:px-6"
>
  <div class="flex min-w-0 items-center gap-2">
    {#if onToggleLeftPanel}
      <SidebarToggle class="lg:hidden" onclick={onToggleLeftPanel} />
    {/if}
    <Icon class="size-5 shrink-0 {iconClass}" />
    <h1 class="truncate text-lg font-semibold">{title}</h1>
    {#if meta}
      {@render meta()}
    {/if}
  </div>
  {#if actions}
    <div class="flex shrink-0 items-center gap-1.5">
      {@render actions()}
    </div>
  {/if}
</header>
