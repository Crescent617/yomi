<!-- eslint-disable svelte/no-dom-manipulating -->
<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import {
    Check,
    Code2,
    Copy,
    Expand,
    RefreshCw,
    TriangleAlert,
  } from "lucide-svelte";
  import { renderMermaid } from "../../mermaid";
  import CodeBlock from "./CodeBlock.svelte";
  import MermaidPreview from "./MermaidPreview.svelte";

  let { source }: { source: string } = $props();

  let container = $state<HTMLDivElement | null>(null);
  let svg = $state("");
  let error = $state("");
  let loading = $state(true);
  let showSource = $state(false);
  let previewOpen = $state(false);
  let copied = $state(false);
  let renderVersion = 0;
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  async function render() {
    const version = ++renderVersion;
    loading = true;
    error = "";
    try {
      const result = await renderMermaid(source);
      if (version !== renderVersion) return;
      svg = result.svg;
      requestAnimationFrame(() => {
        if (version === renderVersion && container && result.bindFunctions) {
          result.bindFunctions(container);
        }
      });
    } catch (cause) {
      if (version !== renderVersion) return;
      svg = "";
      error = cause instanceof Error ? cause.message : "Unable to render diagram.";
    } finally {
      if (version === renderVersion) loading = false;
    }
  }

  async function copySource() {
    try {
      await navigator.clipboard.writeText(source);
      copied = true;
      if (copyTimer) clearTimeout(copyTimer);
      copyTimer = setTimeout(() => (copied = false), 1600);
    } catch (cause) {
      console.error("Failed to copy Mermaid source:", cause);
    }
  }

  function onThemeChanged() {
    void render();
  }

  onMount(() => {
    void render();
    window.addEventListener("theme-changed", onThemeChanged);
  });

  onDestroy(() => {
    renderVersion += 1;
    if (copyTimer) clearTimeout(copyTimer);
    window.removeEventListener("theme-changed", onThemeChanged);
  });
</script>

<div class="mermaid-block group relative my-2 overflow-hidden rounded-md bg-code-bg">
  <div
    class="absolute right-1.5 top-1.5 z-10 flex items-center gap-1 opacity-70 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100"
  >
    {#if svg && !showSource}
      <button
        type="button"
        onclick={() => (previewOpen = true)}
        class="inline-flex size-6 items-center justify-center rounded-sm border border-border/60 bg-background/80 text-muted-foreground shadow-sm backdrop-blur-sm transition-colors hover:bg-background hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        aria-label="Open fullscreen diagram preview"
        title="Fullscreen preview"
      >
        <Expand size={12} />
      </button>
    {/if}
    <button
      type="button"
      onclick={() => (showSource = !showSource)}
      class="inline-flex size-6 items-center justify-center rounded-sm border border-border/60 bg-background/80 text-muted-foreground shadow-sm backdrop-blur-sm transition-colors hover:bg-background hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      aria-label={showSource ? "Show diagram" : "Show Mermaid source"}
      title={showSource ? "Show diagram" : "Show source"}
    >
      <Code2 size={12} />
    </button>
    <button
      type="button"
      onclick={copySource}
      class="inline-flex size-6 items-center justify-center rounded-sm border border-border/60 bg-background/80 text-muted-foreground shadow-sm backdrop-blur-sm transition-colors hover:bg-background hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      aria-label={copied ? "Mermaid source copied" : "Copy Mermaid source"}
      title={copied ? "Copied" : "Copy source"}
    >
      {#if copied}
        <Check size={12} class="text-success" />
      {:else}
        <Copy size={12} />
      {/if}
    </button>
  </div>

  {#if showSource}
    <div class="mermaid-source">
      <CodeBlock code={source} language="mermaid" />
    </div>
  {:else if loading}
    <div
      class="flex min-h-28 items-center justify-center gap-2 p-4 text-xs text-muted-foreground"
      role="status"
    >
      <RefreshCw class="size-3.5 animate-spin text-primary" />
      Rendering diagram…
    </div>
  {:else if error}
    <div class="flex min-h-28 flex-col items-center justify-center gap-2 p-4 text-center">
      <TriangleAlert class="size-4 text-error" />
      <div>
        <p class="text-xs font-medium text-foreground">Diagram couldn’t be rendered</p>
        <p class="mt-0.5 text-[11px] text-muted-foreground">{error}</p>
      </div>
      <button
        type="button"
        onclick={() => (showSource = true)}
        class="text-xs font-medium text-primary hover:underline"
      >
        View source
      </button>
    </div>
  {:else}
    <div
      bind:this={container}
      class="mermaid-canvas overflow-auto p-3 pt-8"
      role="img"
      aria-label="Mermaid diagram"
    >
      <div class="mermaid-diagram">
        {@html svg}
      </div>
    </div>
  {/if}
</div>

{#if previewOpen && svg}
  <MermaidPreview {svg} onClose={() => (previewOpen = false)} />
{/if}

<style>
  .mermaid-block :global(.code-block) {
    margin: 0;
  }
  .mermaid-source :global(.code-block > button) {
    display: none;
  }
  .mermaid-canvas {
    max-height: 420px;
  }
  .mermaid-diagram {
    width: 100%;
    margin-inline: auto;
  }
  .mermaid-diagram :global(svg) {
    display: block;
    width: auto;
    max-width: 100%;
    height: auto;
    max-height: 372px;
    margin-inline: auto;
  }
  .mermaid-canvas :global(text),
  .mermaid-canvas :global(.label),
  .mermaid-canvas :global(.nodeLabel),
  .mermaid-canvas :global(.edgeLabel),
  .mermaid-canvas :global(.cluster-label),
  .mermaid-canvas :global(.messageText),
  .mermaid-canvas :global(.loopText),
  .mermaid-canvas :global(.labelText),
  .mermaid-canvas :global(.stateLabel) {
    font-family: "Inter", system-ui, sans-serif !important;
    color: hsl(var(--foreground));
  }
  .mermaid-canvas :global(.edgeLabel),
  .mermaid-canvas :global(.labelBkg) {
    background-color: hsl(var(--background));
  }
</style>
