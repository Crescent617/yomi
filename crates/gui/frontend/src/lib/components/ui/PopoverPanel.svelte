<script lang="ts">
  import type { Snippet } from "svelte";
  import type { HTMLAttributes } from "svelte/elements";

  /**
   * Shared chrome for the small popover panels anchored to StatusBar and
   * chat-header buttons. Normalizes the pieces every panel agrees on:
   * rounded-lg border container, `border-b` header row (font-medium title),
   * and either a scrolling list body (default, `popover-list-item` rows)
   * or a padded custom body (`padded`). Positioning, width, offset, and
   * z-index stay at the call site via `class`; extra attributes (id, role,
   * aria-*) pass through to the container.
   */
  let {
    title,
    ref = $bindable(),
    class: className = "",
    bodyClass = "",
    padded = false,
    headerActions,
    children,
    ...rest
  }: HTMLAttributes<HTMLDivElement> & {
    title: string;
    ref?: HTMLDivElement | null;
    bodyClass?: string;
    padded?: boolean;
    headerActions?: Snippet;
    children: Snippet;
  } = $props();
</script>

<div
  bind:this={ref}
  class="overflow-hidden rounded-lg border border-border bg-popover text-xs text-popover-foreground shadow-xl {className}"
  {...rest}
>
  <div
    class="flex items-center justify-between gap-2 border-b border-border px-3 py-2"
  >
    <span class="font-medium">{title}</span>
    {#if headerActions}
      <div class="flex items-center gap-2">
        {@render headerActions()}
      </div>
    {/if}
  </div>
  {#if padded}
    <div class="space-y-2.5 px-3 py-2.5 {bodyClass}">
      {@render children()}
    </div>
  {:else}
    <div class="max-h-80 overflow-y-auto py-1 {bodyClass}">
      {@render children()}
    </div>
  {/if}
</div>
