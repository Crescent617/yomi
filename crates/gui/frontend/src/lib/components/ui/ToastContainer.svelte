<script lang="ts">
  import { onMount } from "svelte";
  import { flip } from "svelte/animate";
  import { cubicOut } from "svelte/easing";
  import { fade } from "svelte/transition";
  import { toasts, removeToast, clearToasts } from "../../toast.svelte";
  import ToastCard from "./ToastCard.svelte";

  const MAX_VISIBLE_TOASTS = 10;

  let expanded = $state(false);
  let animateExpandedItems = $state(false);
  let isClearing = $state(false);
  let reduceMotion = $state(false);

  const latestToast = $derived(toasts[toasts.length - 1]);
  const hiddenCount = $derived(Math.max(toasts.length - 1, 0));
  const visibleToasts = $derived(
    toasts.slice(-MAX_VISIBLE_TOASTS).toReversed(),
  );

  onMount(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    const updatePreference = () => (reduceMotion = media.matches);
    updatePreference();
    media.addEventListener("change", updatePreference);
    return () => media.removeEventListener("change", updatePreference);
  });

  function trackExpansion(node: HTMLElement) {
    const expand = () => {
      animateExpandedItems = false;
      expanded = true;
      requestAnimationFrame(() => {
        animateExpandedItems = true;
      });
    };
    const collapse = () => {
      if (!node.contains(document.activeElement)) {
        expanded = false;
        animateExpandedItems = false;
      }
    };
    const handleFocusOut = (event: FocusEvent) => {
      if (!node.contains(event.relatedTarget as Node | null)) {
        expanded = false;
        animateExpandedItems = false;
      }
    };

    node.addEventListener("pointerenter", expand);
    node.addEventListener("pointerleave", collapse);
    node.addEventListener("focusin", expand);
    node.addEventListener("focusout", handleFocusOut);

    return {
      destroy() {
        node.removeEventListener("pointerenter", expand);
        node.removeEventListener("pointerleave", collapse);
        node.removeEventListener("focusin", expand);
        node.removeEventListener("focusout", handleFocusOut);
      },
    };
  }

  function collapsedToastMotion(
    _node: Element,
    _params: undefined,
    { direction }: { direction: "in" | "out" | "both" },
  ) {
    if (reduceMotion) return { duration: 0 };

    const entering = direction === "in";
    return {
      delay: entering ? 70 : 0,
      duration: entering ? 240 : 380,
      easing: entering ? cubicOut : undefined,
      css: (t: number) => `opacity: ${t};`,
    };
  }

  function expandedToastMotion(
    _node: Element,
    { index }: { index: number },
    { direction }: { direction: "in" | "out" | "both" },
  ) {
    if (reduceMotion) return { duration: 0 };

    const entering = direction === "in";
    if (entering && !animateExpandedItems) return { duration: 0 };

    return {
      delay: entering
        ? Math.min(index * 18, 90)
        : isClearing
          ? Math.min(index * 24, 120)
          : 0,
      duration: entering ? 260 : 300,
      easing: entering ? cubicOut : undefined,
      css: (t: number) => {
        const x = entering ? (1 - t) * 20 : 0;
        const scale = entering ? 0.985 + t * 0.015 : 1;
        return `opacity: ${t}; transform: translate3d(${x}px, 0, 0) scale(${scale});`;
      },
    };
  }

  function stackViewMotion(
    _node: Element,
    { view }: { view: "collapsed" | "expanded" },
    { direction }: { direction: "in" | "out" | "both" },
  ) {
    if (reduceMotion) return { duration: 0 };

    const entering = direction === "in";
    const expandedView = view === "expanded";

    return {
      delay: entering && !expandedView ? 70 : 0,
      duration: expandedView ? (entering ? 300 : 240) : entering ? 200 : 150,
      easing: cubicOut,
      css: (t: number) => {
        if (!expandedView) {
          return `opacity: ${t};`;
        }

        return `opacity: ${t}; clip-path: inset(0 0 ${(1 - t) * 100}% 0 round var(--radius-lg));`;
      },
    };
  }

  function countMotion() {
    if (reduceMotion) return { duration: 0 };

    return {
      duration: 180,
      easing: cubicOut,
      css: (t: number) =>
        `opacity: ${t}; transform: translateY(${(1 - t) * 4}px) scale(${0.88 + t * 0.12});`,
    };
  }

  function clearAllToasts() {
    isClearing = true;
    clearToasts();
    window.setTimeout(
      () => {
        isClearing = false;
      },
      reduceMotion ? 0 : 520,
    );
  }
</script>

<div
  class="notification-region fixed top-8 right-4 z-[100] w-full max-w-xs"
  role="region"
  aria-label="Notifications"
  use:trackExpansion
>
  <div class="view-slot grid items-start">
    {#if expanded}
      <div
        class="expanded-view"
        transition:stackViewMotion={{ view: "expanded" }}
      >
        <div
          class="toast-list flex max-h-[min(26rem,calc(100vh-7rem))] flex-col gap-2 overflow-y-auto"
        >
          {#each visibleToasts as toast, index (toast.id)}
            <div
              class="toast-layout"
              animate:flip={{
                duration: reduceMotion ? 0 : 240,
                easing: cubicOut,
              }}
            >
              <div
                class="toast-motion"
                transition:expandedToastMotion={{ index }}
              >
                <ToastCard {toast} onDismiss={() => removeToast(toast.id)} />
              </div>
            </div>
          {/each}
        </div>
      </div>
    {:else}
      <div
        class="collapsed-view"
        transition:stackViewMotion={{ view: "collapsed" }}
      >
        <div class="collapsed-stack relative pb-3">
          {#if toasts.length > 2}
            <div
              class="stack-layer stack-layer-back"
              aria-hidden="true"
              transition:fade={{ duration: reduceMotion ? 0 : 160 }}
            ></div>
          {/if}
          {#if toasts.length > 1}
            <div
              class="stack-layer stack-layer-front"
              aria-hidden="true"
              transition:fade={{ duration: reduceMotion ? 0 : 160 }}
            ></div>
          {/if}

          <div class="collapsed-slot grid">
            {#if latestToast}
              {#key latestToast.id}
                <div class="collapsed-toast" transition:collapsedToastMotion>
                  <ToastCard
                    toast={latestToast}
                    onDismiss={() => removeToast(latestToast.id)}
                  />
                </div>
              {/key}
            {/if}
          </div>

          {#key hiddenCount}
            {#if hiddenCount > 0}
              <span
                class="toast-count pointer-events-none absolute right-2 -bottom-1.5 z-10 rounded-full border border-subtle bg-popover px-1.5 py-0.5 text-[10px] font-semibold leading-none text-muted-foreground shadow-sm"
                aria-hidden="true"
                transition:countMotion
              >
                +{hiddenCount}
              </span>
            {/if}
          {/key}
        </div>
      </div>
    {/if}
  </div>

  {#if toasts.length > 0}
    <button
      type="button"
      onclick={clearAllToasts}
      class="clear-all pointer-events-none absolute -top-5 right-0 z-[110] rounded-md border border-subtle bg-popover px-2 py-0.5 text-xs font-medium text-muted-foreground opacity-0 shadow-sm hover:bg-muted"
      class:clear-all-visible={expanded}
      transition:fade={{ duration: reduceMotion ? 0 : 140 }}
    >
      Clear all
    </button>
  {/if}
</div>

<style>
  .view-slot > :global(*) {
    grid-area: 1 / 1;
  }

  .collapsed-slot > :global(*) {
    grid-area: 1 / 1;
  }

  .collapsed-toast {
    position: relative;
    z-index: 3;
  }

  .stack-layer {
    position: absolute;
    inset: 0 0 0.75rem;
    z-index: 1;
    border: 1px solid var(--color-border);
    border-radius: var(--radius-lg);
    background: var(--color-popover);
    box-shadow: 0 4px 10px var(--color-subtle);
    transform-origin: top center;
  }

  .stack-layer-front {
    transform: translateY(0.375rem) scale(0.97);
  }

  .stack-layer-back {
    transform: translateY(0.75rem) scale(0.94);
  }

  .toast-list {
    scrollbar-width: none;
    overscroll-behavior: contain;
  }

  .toast-list::-webkit-scrollbar {
    display: none;
  }

  .toast-layout {
    will-change: transform;
  }

  .toast-motion,
  .collapsed-toast {
    transform-origin: top right;
    will-change: transform, opacity;
  }

  .toast-count {
    transform-origin: center;
  }

  .clear-all {
    transition:
      opacity 160ms ease,
      background-color 150ms ease;
  }

  .clear-all-visible {
    pointer-events: auto;
    opacity: 1;
  }

  @media (prefers-reduced-motion: reduce) {
    .clear-all {
      transition-duration: 1ms !important;
      transition-delay: 0ms !important;
    }
  }
</style>
