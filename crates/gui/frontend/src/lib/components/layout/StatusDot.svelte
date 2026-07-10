<script lang="ts">
  /**
   * Session activity indicator.
   * - running (streaming / executing_tool / compacting): breathing dot
   * - attention (any other non-idle, non-closed phase, e.g. waiting for
   *   permission): ping halo to draw the eye — this state needs the user.
   */
  let { phase }: { phase: string } = $props();

  const RUNNING = ["streaming", "executing_tool", "compacting"];
  const active = $derived(phase !== "idle" && phase !== "closed");
  const running = $derived(RUNNING.includes(phase));
</script>

{#if active}
  {#if running}
    <span
      class="inline-flex h-1.5 w-1.5 shrink-0 rounded-full bg-primary animate-pulse"
      title="Running"
    ></span>
  {:else}
    <span class="relative flex h-1.5 w-1.5 shrink-0" title="Waiting for you">
      <span
        class="animate-ping absolute inline-flex h-full w-full rounded-full bg-warning opacity-60"
      ></span>
      <span class="relative inline-flex rounded-full h-1.5 w-1.5 bg-warning"
      ></span>
    </span>
  {/if}
{/if}
