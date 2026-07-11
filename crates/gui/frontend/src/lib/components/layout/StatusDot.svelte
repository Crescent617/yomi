<script lang="ts">
  /**
   * Session activity indicator.
   * - running (streaming / executing_tool / compacting): 6-dot chase spinner
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
    <span class="spinner text-primary shrink-0" title="Running">
      {#each Array(6) as _, i (i)}
        <span class="spin-dot" style="--i: {i}"></span>
      {/each}
    </span>
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

<style>
  .spinner {
    position: relative;
    display: inline-flex;
    width: 10px;
    height: 10px;
  }
  .spin-dot {
    position: absolute;
    top: 50%;
    left: 50%;
    width: 2px;
    height: 2px;
    margin: -1px 0 0 -1px;
    border-radius: 9999px;
    background: currentColor;
    transform: rotate(calc(var(--i) * 60deg)) translateY(-3.5px);
    animation: dot-chase 0.9s linear infinite;
    animation-delay: calc(var(--i) * -0.15s);
  }
  @keyframes dot-chase {
    0%,
    100% {
      opacity: 1;
    }
    60% {
      opacity: 0.15;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .spin-dot {
      animation: none;
      opacity: 1;
    }
  }
</style>
