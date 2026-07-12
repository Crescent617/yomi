<script lang="ts">
  import { Check, CircleAlert } from "lucide-svelte";
  import LoadingIndicator from "./LoadingIndicator.svelte";

  let {
    state = "loading",
    label,
    class: className = "",
  }: {
    state?: "loading" | "success" | "error";
    label: string;
    class?: string;
  } = $props();
</script>

<span
  class="inline-flex min-h-5 items-center gap-1.5 text-xs text-muted-foreground {className}"
  class:text-success={state === "success"}
  class:text-error={state === "error"}
  role="status"
  aria-live="polite"
>
  {#if state === "loading"}
    <LoadingIndicator size="sm" {label} class="text-primary" />
  {:else if state === "success"}
    <Check class="size-3.5" />
  {:else}
    <CircleAlert class="size-3.5" />
  {/if}
  <span>{label}</span>
</span>
