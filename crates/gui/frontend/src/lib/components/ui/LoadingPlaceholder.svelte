<script lang="ts">
  import LoadingIndicator from "./LoadingIndicator.svelte";

  let {
    label,
    description,
    active = true,
    class: className = "",
  }: {
    label: string;
    description?: string;
    active?: boolean;
    class?: string;
  } = $props();
</script>

<div
  class="flex h-full min-h-28 flex-col items-center justify-center gap-2 p-4 text-center {className}"
  role="status"
  aria-live="polite"
>
  {#if active}
    <LoadingIndicator size="lg" {label} class="text-primary" />
  {:else}
    <span class="h-3 w-9" aria-hidden="true"></span>
  {/if}
  <div class="text-xs font-medium text-foreground/85">{label}</div>
  {#if description}
    <div class="max-w-xs text-[11px] text-muted-foreground">
      {description}
    </div>
  {/if}
</div>
