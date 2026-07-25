<script lang="ts">
  /**
   * Session activity indicator.
   * - running (streaming / executing_tool / compacting): typing dots —
   *   the "agent is producing" metaphor from chat apps
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
    <span class="typing text-primary shrink-0" title="Running">
      {#each Array(3) as _, i (i)}
        <span class="typing-dot" style="--i: {i}"></span>
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
  .typing {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    height: 10px;
    padding: 0 1px;
  }
  .typing-dot {
    width: 3px;
    height: 3px;
    border-radius: 9999px;
    background: currentColor;
    animation: typing-wave 1.2s ease-in-out infinite;
    animation-delay: calc(var(--i) * 0.15s);
  }
  @keyframes typing-wave {
    0%,
    60%,
    100% {
      transform: translateY(0);
      opacity: 0.6;
    }
    30% {
      transform: translateY(-1.5px);
      opacity: 1;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .typing-dot {
      animation: none;
      opacity: 1;
    }
  }
</style>
