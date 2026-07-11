<script lang="ts">
  import { onDestroy } from "svelte";
  import { flip } from "svelte/animate";
  import { fade } from "svelte/transition";
  import {
    pauseToasts,
    removeToast,
    resumeToasts,
    toasts,
  } from "../../toast.svelte";
  import ToastCard from "./ToastCard.svelte";

  let expanded = $state(false);
  let container = $state<HTMLDivElement>();
  let hovered = $state(false);
  let focusWithin = $state(false);

  const visibleToasts = $derived([...toasts].reverse());

  function updatePauseState() {
    if (hovered || focusWithin) pauseToasts();
    else resumeToasts();
  }

  function handleMouseEnter() {
    hovered = true;
    updatePauseState();
  }

  function handleMouseLeave() {
    hovered = false;
    updatePauseState();
  }

  function handleFocusIn() {
    focusWithin = true;
    updatePauseState();
  }

  function collapseOnOutsideClick(event: PointerEvent) {
    if (expanded && !container?.contains(event.target as Node))
      expanded = false;
  }

  function handleFocusOut(event: FocusEvent) {
    focusWithin = Boolean(
      event.relatedTarget && container?.contains(event.relatedTarget as Node),
    );
    updatePauseState();
  }

  function clearInteractionState() {
    hovered = false;
    focusWithin = false;
    resumeToasts();
  }

  $effect(() => {
    if (visibleToasts.length === 0) clearInteractionState();
  });

  onDestroy(clearInteractionState);
</script>

<svelte:window onpointerdown={collapseOnOutsideClick} />

{#if visibleToasts.length > 0}
  <div
    bind:this={container}
    class="pointer-events-auto fixed right-4 top-12 z-[9999] w-[min(22rem,calc(100vw-2rem))]"
    class:space-y-2={expanded}
    onmouseenter={handleMouseEnter}
    onmouseleave={handleMouseLeave}
    onfocusin={handleFocusIn}
    onfocusout={handleFocusOut}
    role="region"
    aria-label="Notifications"
  >
    {#each visibleToasts as toast, index (toast.id)}
      <div
        class="origin-top-right transition-[transform,opacity] duration-200 ease-out"
        class:relative={expanded || index === 0}
        class:absolute={!expanded && index > 0}
        class:inset-x-0={!expanded && index > 0}
        class:top-0={!expanded && index > 0}
        class:pointer-events-none={!expanded && index > 0}
        style:z-index={visibleToasts.length - index}
        style:transform={expanded
          ? "translate3d(0, 0, 0) scale(1)"
          : index === 0
            ? "translate3d(0, 0, 0) scale(1)"
            : `translate3d(0, ${index * 7}px, 0) scale(${Math.max(0.9, 1 - index * 0.025)})`}
        style:opacity={!expanded && index > 3 ? 0 : 1}
        animate:flip={{ duration: 200 }}
        transition:fade={{ duration: 140 }}
      >
        <div>
          <ToastCard
            {toast}
            compact={!expanded && index > 0}
            onDismiss={() => removeToast(toast.id)}
          />
        </div>
      </div>
    {/each}

    {#if !expanded && visibleToasts.length > 1}
      <button
        type="button"
        class="absolute -bottom-5 right-1 z-50 rounded-full bg-popover px-2 py-0.5 font-mono text-[10px] text-muted-foreground shadow-sm ring-1 ring-border/70 transition-colors hover:text-foreground"
        onclick={() => (expanded = true)}
        aria-label={`Show ${visibleToasts.length} notifications`}
      >
        {visibleToasts.length} notifications
      </button>
    {/if}
  </div>
{/if}
