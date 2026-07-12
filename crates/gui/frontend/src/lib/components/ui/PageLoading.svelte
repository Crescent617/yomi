<script lang="ts">
  import type { Snippet } from "svelte";

  let {
    label = "Loading page",
    children,
  }: {
    label?: string;
    children: Snippet;
  } = $props();
</script>

<div role="status" aria-label={label} aria-busy="true">
  {@render children()}
</div>
