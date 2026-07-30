<script lang="ts">
  import { PanelLeftOpen, PanelLeftClose } from "lucide-svelte";

  let {
    open = false,
    attention = false,
    class: className = "",
    onclick,
  }: {
    /** Whether the sidebar/drawer is currently open — drives icon and aria. */
    open?: boolean;
    /** Tint the toggle to signal that content is hidden behind it. */
    attention?: boolean;
    class?: string;
    onclick?: () => void;
  } = $props();

  const label = $derived(open ? "Hide sidebar" : "Show sidebar");
</script>

<button
  type="button"
  {onclick}
  class="inline-flex size-7 shrink-0 items-center justify-center rounded-md transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring {attention
    ? 'bg-primary/5 text-primary hover:bg-primary/10'
    : 'text-muted-foreground hover:bg-secondary/80 hover:text-foreground'} {className}"
  title={label}
  aria-label={label}
  aria-expanded={open}
>
  {#if open}
    <PanelLeftClose size={16} />
  {:else}
    <PanelLeftOpen size={16} />
  {/if}
</button>
