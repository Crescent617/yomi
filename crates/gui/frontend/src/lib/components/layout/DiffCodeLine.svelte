<script lang="ts">
  import type { ThemedToken } from "shiki";
  import { tokenStyle } from "./diff-highlight";

  let {
    tokens,
    text,
  }: {
    tokens: ThemedToken[] | undefined;
    text: string;
  } = $props();
</script>

<span class="diff-code whitespace-pre">
  {#if tokens}
    {#each tokens as token, i (i)}<span style={tokenStyle(token)}
        >{token.content}</span
      >{/each}
  {:else}{text}{/if}
</span>

<style>
  :global(.dark) .diff-code :global(span) {
    color: var(--shiki-dark) !important;
  }
</style>
