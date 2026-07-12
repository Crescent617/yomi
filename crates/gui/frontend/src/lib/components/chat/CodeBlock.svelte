<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { Check, Copy } from "lucide-svelte";
  import { highlightCode, normalizeCodeLanguage } from "./code-highlight";

  let { language = "Code", code }: { language?: string; code: string } =
    $props();

  let copied = $state(false);
  let highlighted = $state<string | null>(null);
  let block: HTMLDivElement | null = null;
  let resetTimer: ReturnType<typeof setTimeout> | undefined;
  let idleHandle: number | undefined;
  let observer: IntersectionObserver | undefined;
  let nearViewport = false;
  let highlightStarted = false;
  let highlightVersion = 0;

  type IdleWindow = Window & {
    requestIdleCallback?: (
      callback: () => void,
      options?: { timeout: number },
    ) => number;
    cancelIdleCallback?: (handle: number) => void;
  };

  async function copyCode() {
    try {
      await navigator.clipboard.writeText(code);
      copied = true;
      if (resetTimer) clearTimeout(resetTimer);
      resetTimer = setTimeout(() => (copied = false), 1600);
    } catch (error) {
      console.error("Failed to copy code:", error);
    }
  }

  function scrollRoot(element: HTMLElement): Element | null {
    let parent = element.parentElement;
    while (parent) {
      const { overflowY } = getComputedStyle(parent);
      if (overflowY === "auto" || overflowY === "scroll") return parent;
      parent = parent.parentElement;
    }
    return null;
  }

  function cancelScheduledHighlight() {
    if (idleHandle === undefined) return;
    const idleWindow = window as IdleWindow;
    if (idleWindow.cancelIdleCallback)
      idleWindow.cancelIdleCallback(idleHandle);
    else clearTimeout(idleHandle);
    idleHandle = undefined;
  }

  function scheduleHighlight() {
    if (!nearViewport || highlightStarted || idleHandle !== undefined) return;
    const run = () => {
      idleHandle = undefined;
      if (!nearViewport || highlightStarted) return;
      highlightStarted = true;
      const source = code;
      const sourceLanguage = normalizeCodeLanguage(language);
      const version = ++highlightVersion;
      void highlightCode(source, sourceLanguage).then((html) => {
        if (version === highlightVersion) highlighted = html;
      });
    };
    const idleWindow = window as IdleWindow;
    idleHandle = idleWindow.requestIdleCallback
      ? idleWindow.requestIdleCallback(run, { timeout: 500 })
      : window.setTimeout(run, 0);
  }

  onMount(() => {
    if (!block || typeof IntersectionObserver === "undefined") {
      nearViewport = true;
      scheduleHighlight();
      return;
    }

    observer = new IntersectionObserver(
      ([entry]) => {
        nearViewport = entry.isIntersecting;
        if (nearViewport) scheduleHighlight();
        else cancelScheduledHighlight();
      },
      {
        root: scrollRoot(block),
        rootMargin: "300px 0px",
      },
    );
    observer.observe(block);
  });

  $effect(() => {
    const sourceKey = `${code}\0${language}`;
    if (!sourceKey) return;
    highlighted = null;
    highlightStarted = false;
    highlightVersion += 1;
    cancelScheduledHighlight();
    scheduleHighlight();
  });

  onDestroy(() => {
    observer?.disconnect();
    cancelScheduledHighlight();
    highlightVersion += 1;
    if (resetTimer) clearTimeout(resetTimer);
  });
</script>

<div
  bind:this={block}
  class="code-block group/code relative my-2 overflow-hidden rounded-md border border-border/70 bg-code-bg"
>
  <button
    type="button"
    onclick={copyCode}
    class="absolute right-1.5 top-1.5 z-10 inline-flex h-6 w-6 items-center justify-center rounded-sm border border-border/60 bg-background/80 text-muted-foreground opacity-0 shadow-sm backdrop-blur-sm transition-all group-hover/code:opacity-100 hover:bg-background hover:text-foreground focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    aria-label={copied ? "Code copied" : `Copy ${language || "code"}`}
    title={copied ? "Copied" : `Copy ${language || "code"}`}
  >
    {#if copied}
      <Check size={12} class="text-success" />
    {:else}
      <Copy size={12} />
    {/if}
  </button>
  <div class="code-block-content">
    {#if highlighted}
      {@html highlighted}
    {:else}
      <pre class="code-block-pre"><code>{code}</code></pre>
    {/if}
  </div>
</div>

<style>
  .code-block-content :global(pre) {
    margin: 0;
    overflow-x: auto;
    border: 0 !important;
    border-radius: 0;
    background: transparent !important;
    padding: 0.5rem 2.25rem 0.5rem 0.625rem;
    box-shadow: none;
    color: hsl(var(--foreground));
    font-family: ui-monospace, monospace;
    font-size: 0.75rem;
    line-height: 1.625;
    tab-size: 2;
  }
  .code-block-content :global(code) {
    border: 0;
    background: transparent;
    padding: 0;
    font: inherit;
    box-shadow: none;
  }
  .code-block-content :global(.shiki),
  .code-block-content :global(.shiki span) {
    background-color: transparent !important;
  }
  :global(.dark) .code-block-content :global(.shiki),
  :global(.dark) .code-block-content :global(.shiki span) {
    color: var(--shiki-dark) !important;
    background-color: transparent !important;
  }
</style>
