<script lang="ts">
  import { Check, Copy } from "lucide-svelte";

  let { language = "Code", code }: { language?: string; code: string } =
    $props();

  let copied = $state(false);
  let resetTimer: ReturnType<typeof setTimeout> | undefined;

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
</script>

<div
  class="code-block group relative my-2 overflow-hidden rounded-md"
  style="background: var(--code-bg)"
>
  <button
    type="button"
    onclick={copyCode}
    class="absolute right-1.5 top-1.5 z-10 inline-flex h-6 w-6 items-center justify-center rounded-sm border border-border/60 bg-background/80 text-muted-foreground opacity-70 shadow-sm backdrop-blur-sm transition-all hover:bg-background hover:text-foreground hover:opacity-100 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    aria-label={copied ? "Code copied" : `Copy ${language || "code"}`}
    title={copied ? "Copied" : `Copy ${language || "code"}`}
  >
    {#if copied}
      <Check size={12} class="text-success" />
    {:else}
      <Copy size={12} />
    {/if}
  </button>
  <pre
    class="code-block-content m-0 overflow-x-auto border-0 bg-transparent p-3 pr-10 text-xs leading-relaxed"><code
      >{code}</code
    ></pre>
</div>

<style>
  .code-block :global(.code-block-content) {
    margin: 0;
    border: 0;
    background: transparent;
    border-radius: 0;
    box-shadow: none;
  }
  .code-block :global(.code-block-content code) {
    border: 0;
    background: transparent;
    padding: 0;
    box-shadow: none;
  }
</style>
