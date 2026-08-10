<script lang="ts">
  import type { ComponentType, SvelteComponent } from "svelte";

  /**
   * Standard 32px ghost icon button used in panel headers and row actions.
   * `tone` picks the hover color; `pressed` turns it into an aria toggle.
   */
  interface Props {
    label: string;
    icon: ComponentType<SvelteComponent<{ class?: string }>>;
    onclick: () => void;
    disabled?: boolean;
    spinning?: boolean;
    tone?: "default" | "primary" | "destructive";
    /** Toggle state; leave undefined for plain buttons (no aria-pressed). */
    pressed?: boolean;
    iconClass?: string;
    class?: string;
  }

  let {
    label,
    icon: Icon,
    onclick,
    disabled = false,
    spinning = false,
    tone = "default",
    pressed = undefined,
    iconClass = "",
    class: cls = "",
  }: Props = $props();

  const tones = {
    default: "text-muted-foreground hover:bg-secondary hover:text-foreground",
    primary: "text-muted-foreground hover:bg-primary/10 hover:text-primary",
    destructive:
      "text-muted-foreground hover:bg-destructive/10 hover:text-destructive",
  };
</script>

<button
  type="button"
  {onclick}
  {disabled}
  class="inline-flex size-8 items-center justify-center rounded-md transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50 {pressed
    ? 'bg-secondary text-foreground'
    : tones[tone]} {cls}"
  title={label}
  aria-label={label}
  aria-pressed={pressed}
>
  <Icon class="size-4 {spinning ? 'animate-spin' : ''} {iconClass}" />
</button>
