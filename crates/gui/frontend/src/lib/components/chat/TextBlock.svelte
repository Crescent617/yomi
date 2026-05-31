<script lang="ts">
  import { Marked } from "marked";

  let { content }: { content: string } = $props();

  const md = new Marked();
  md.setOptions({ gfm: true, breaks: true });

  const rendered = $derived(md.parse(content || "", { async: false }) as string);
</script>

<style>
  .text-block :global(h1) { font-size: 1.25rem; font-weight: 700; margin: 0.5rem 0; }
  .text-block :global(h2) { font-size: 1.125rem; font-weight: 600; margin: 0.4rem 0; }
  .text-block :global(h3) { font-size: 1rem; font-weight: 600; margin: 0.3rem 0; }
  .text-block :global(p) { margin: 0.25rem 0; }
  .text-block :global(ul) { list-style-type: disc; padding-left: 1.25rem; margin: 0.25rem 0; }
  .text-block :global(ol) { list-style-type: decimal; padding-left: 1.25rem; margin: 0.25rem 0; }
  .text-block :global(li) { margin: 0.125rem 0; }
  .text-block :global(pre) { background: hsl(var(--muted)); padding: 0.5rem; border-radius: 0.375rem; overflow-x: auto; margin: 0.25rem 0; }
  .text-block :global(code) { font-family: ui-monospace, monospace; font-size: 0.875rem; }
  .text-block :global(pre code) { background: transparent; padding: 0; }
  .text-block :global(:not(pre) > code) { background: hsl(var(--muted)); padding: 0.125rem 0.25rem; border-radius: 0.25rem; }
  .text-block :global(blockquote) { border-left: 3px solid hsl(var(--border)); padding-left: 0.75rem; margin: 0.25rem 0; color: hsl(var(--muted-foreground)); }
  .text-block :global(hr) { border: 0; border-top: 1px solid hsl(var(--border)); margin: 0.5rem 0; }
  .text-block :global(table) { width: 100%; border-collapse: collapse; margin: 0.25rem 0; }
  .text-block :global(th), .text-block :global(td) { border: 1px solid hsl(var(--border)); padding: 0.25rem 0.5rem; text-align: left; }
  .text-block :global(th) { background: hsl(var(--muted)); font-weight: 600; }
  .text-block :global(a) { color: hsl(var(--primary)); text-decoration: underline; }
  .text-block :global(strong) { font-weight: 700; }
  .text-block :global(em) { font-style: italic; }
</style>

<div class="text-sm text-block">
  {@html rendered}
</div>
