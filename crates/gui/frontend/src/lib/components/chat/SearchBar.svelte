<script lang="ts">
  import { onMount } from "svelte";
  import { ChevronDown, ChevronUp, X } from "lucide-svelte";

  let {
    query = $bindable(""),
    activeIndex,
    total,
    focusTick = 0,
    onNext,
    onPrev,
    onClose,
  }: {
    query: string;
    /** 0-based index of the current match; display adds 1. */
    activeIndex: number;
    total: number;
    /** Increment to re-focus + select (⌘F while already open). */
    focusTick?: number;
    onNext: () => void;
    onPrev: () => void;
    onClose: () => void;
  } = $props();

  let inputEl = $state<HTMLInputElement | null>(null);

  onMount(() => {
    inputEl?.focus();
    inputEl?.select();
  });

  $effect(() => {
    if (focusTick > 0) {
      inputEl?.focus();
      inputEl?.select();
    }
  });

  function onKeydown(event: KeyboardEvent) {
    // 组字中按 Enter 是确认候选，不是导航（Esc 同理交给输入法）。
    if (event.isComposing) return;
    if (event.key === "Enter") {
      event.preventDefault();
      if (event.shiftKey) onPrev();
      else onNext();
    } else if (event.key === "Escape") {
      // Consume so a bubble-phase listener never pairs this Esc with a
      // second close action.
      event.preventDefault();
      event.stopPropagation();
      onClose();
    }
  }
</script>

<div
  role="search"
  aria-label="Search messages"
  class="pointer-events-auto flex items-center gap-1 rounded-md border border-border bg-card py-1 pl-2.5 pr-1 shadow-md"
>
  <input
    bind:this={inputEl}
    bind:value={query}
    type="text"
    placeholder="Search messages"
    aria-label="Search messages"
    spellcheck="false"
    class="w-44 bg-transparent text-sm text-foreground placeholder:text-muted-foreground/70 focus:outline-none"
    onkeydown={onKeydown}
  />
  <span
    class="micro-label min-w-10 text-center text-muted-foreground"
    aria-live="polite"
  >
    {#if query.trim() && total === 0}
      0/0
    {:else if total > 0}
      {activeIndex + 1}/{total}
    {:else}
      &nbsp;
    {/if}
  </span>
  <button
    type="button"
    onclick={onPrev}
    disabled={total === 0}
    aria-label="Previous match"
    title="Previous match (Shift+Enter)"
    class="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40 disabled:hover:bg-transparent motion-reduce:transition-none"
  >
    <ChevronUp size={14} strokeWidth={2.25} />
  </button>
  <button
    type="button"
    onclick={onNext}
    disabled={total === 0}
    aria-label="Next match"
    title="Next match (Enter)"
    class="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-40 disabled:hover:bg-transparent motion-reduce:transition-none"
  >
    <ChevronDown size={14} strokeWidth={2.25} />
  </button>
  <button
    type="button"
    onclick={onClose}
    aria-label="Close search"
    title="Close search (Esc)"
    class="inline-flex h-6 w-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring motion-reduce:transition-none"
  >
    <X size={14} strokeWidth={2.25} />
  </button>
</div>
