<!-- eslint-disable svelte/no-dom-manipulating -->
<script lang="ts">
  /* eslint-disable svelte/no-dom-manipulating */
  import { mount, onDestroy, unmount } from "svelte";
  import * as smd from "streaming-markdown";
  import CodeBlock from "./CodeBlock.svelte";
  import MermaidBlock from "./MermaidBlock.svelte";
  import { countClosedMermaidFences } from "./markdown-fences";

  let { content, isStreaming }: { content: string; isStreaming?: boolean } =
    $props();

  let el: HTMLDivElement | null = null;
  let parser: ReturnType<typeof smd.parser> | null | undefined;
  let lastContent = "";
  let enhanceFrame: number | null = null;
  let enhancedMermaidCount = 0;
  let mountedCodeBlocks: ReturnType<typeof mount>[] = [];

  function clearMountedCodeBlocks() {
    for (const component of mountedCodeBlocks) void unmount(component);
    mountedCodeBlocks = [];
    enhancedMermaidCount = 0;
  }

  function createRenderer() {
    if (!el)
      throw new Error("Cannot create a Markdown renderer without a root");
    return smd.default_renderer(el);
  }

  function enhanceCodeBlocks() {
    if (!el) return;
    const closedMermaidCount = isStreaming
      ? countClosedMermaidFences(content)
      : Number.POSITIVE_INFINITY;
    let mermaidsToEnhance = Math.max(
      0,
      closedMermaidCount - enhancedMermaidCount,
    );
    const blocks = [...el.querySelectorAll<HTMLElement>("pre > code")];

    for (const codeElement of blocks) {
      const pre = codeElement.parentElement;
      if (!pre) continue;
      const languageClass = [...codeElement.classList].find((name) =>
        name.startsWith("language-"),
      );
      // streaming-markdown emits the fence info as a direct class ("mermaid"),
      // while other renderers commonly emit "language-mermaid".
      const rawLanguage = (
        languageClass?.slice("language-".length) ||
        codeElement.classList[0] ||
        "text"
      ).toLowerCase();
      const code = codeElement.textContent ?? "";

      if (isStreaming && rawLanguage !== "mermaid") continue;
      if (isStreaming && mermaidsToEnhance === 0) continue;

      const target = document.createElement("div");
      pre.replaceWith(target);

      if (rawLanguage === "mermaid") {
        mermaidsToEnhance -= 1;
        enhancedMermaidCount += 1;
        mountedCodeBlocks.push(
          mount(MermaidBlock, {
            target,
            props: { source: code },
          }),
        );
        continue;
      }

      mountedCodeBlocks.push(
        mount(CodeBlock, {
          target,
          props: {
            code,
            language: rawLanguage === "text" ? "Code" : rawLanguage,
          },
        }),
      );
    }
  }

  function scheduleCodeBlockEnhancement() {
    if (enhanceFrame !== null) cancelAnimationFrame(enhanceFrame);
    enhanceFrame = requestAnimationFrame(() => {
      enhanceFrame = null;
      enhanceCodeBlocks();
    });
  }

  function cancelCodeBlockEnhancement() {
    if (enhanceFrame === null) return;
    cancelAnimationFrame(enhanceFrame);
    enhanceFrame = null;
  }

  function finalizeParser() {
    if (!parser) return;
    smd.parser_end(parser);
    parser = null;
    scheduleCodeBlockEnhancement();
  }

  function resetParser(content: string) {
    cancelCodeBlockEnhancement();
    clearMountedCodeBlocks();
    if (!el) return;
    el.innerHTML = "";
    parser = smd.parser(createRenderer());
    smd.parser_write(parser, content);
    lastContent = content;
  }

  onDestroy(() => {
    cancelCodeBlockEnhancement();
    if (parser) smd.parser_end(parser);
    parser = null;
    clearMountedCodeBlocks();
    el = null;
  });

  $effect(() => {
    if (!el) return;
    const curr = content;
    const streaming = isStreaming;

    if (parser === undefined) {
      resetParser(curr);
      if (!streaming) finalizeParser();
      else scheduleCodeBlockEnhancement();
      return;
    }

    if (curr === lastContent) {
      if (!streaming) finalizeParser();
      return;
    }

    if (streaming && parser && curr.startsWith(lastContent)) {
      smd.parser_write(parser, curr.slice(lastContent.length));
      lastContent = curr;
      scheduleCodeBlockEnhancement();
      return;
    }

    // Rebuild after replacement, truncation, or a finalized message changing.
    resetParser(curr);
    if (!streaming) finalizeParser();
    else scheduleCodeBlockEnhancement();
  });
</script>

<div class="text-sm text-block" bind:this={el}></div>

<style>
  .text-block {
    color: hsl(var(--foreground));
    line-height: 1.65;
  }
  .text-block :global(h1) {
    font-size: 1.25rem;
    font-weight: 700;
    margin: 0.75rem 0 0.5rem;
    padding-bottom: 0.3rem;
    border-bottom: 1px solid hsl(var(--border) / 0.65);
  }
  .text-block :global(h2) {
    font-size: 1.125rem;
    font-weight: 650;
    margin: 0.65rem 0 0.4rem;
  }
  .text-block :global(h3) {
    font-size: 1rem;
    font-weight: 600;
    margin: 0.5rem 0 0.3rem;
  }
  .text-block :global(h1),
  .text-block :global(h2),
  .text-block :global(h3) {
    color: hsl(var(--foreground));
    letter-spacing: -0.01em;
  }
  .text-block :global(p) {
    margin: 0.35rem 0;
  }
  .text-block > :global(:first-child) {
    margin-top: 0.125rem;
  }
  .text-block :global(ul) {
    list-style-type: disc;
    padding-left: 1.25rem;
    margin: 0.25rem 0;
  }
  .text-block :global(ol) {
    list-style-type: decimal;
    padding-left: 1.25rem;
    margin: 0.25rem 0;
  }
  .text-block :global(li) {
    margin: 0.125rem 0;
  }
  .text-block :global(li::marker) {
    color: hsl(var(--primary) / 0.8);
    font-weight: 600;
  }
  .text-block :global(pre) {
    position: relative;
    background: var(--code-bg);
    border: 1px solid hsl(var(--border) / 0.7);
    padding: 0.75rem;
    border-radius: 0.375rem;
    overflow-x: auto;
    margin: 0.5rem 0;
  }
  .text-block :global(code) {
    font-family: ui-monospace, monospace;
    font-size: 0.875rem;
  }
  .text-block :global(pre code) {
    background: transparent;
    padding: 0;
  }
  .text-block :global(:not(pre) > code) {
    color: hsl(var(--foreground));
    background: var(--code-bg);
    border: 1px solid hsl(var(--border) / 0.55);
    padding: 0.12rem 0.35rem;
    border-radius: 0.2rem;
  }
  .text-block :global(blockquote) {
    border-left: 2px solid hsl(var(--primary) / 0.65);
    padding: 0.15rem 0 0.15rem 0.75rem;
    margin: 0.5rem 0;
    color: hsl(var(--muted-foreground));
    background: hsl(var(--primary) / 0.035);
  }
  .text-block :global(hr) {
    height: 1px;
    border: 0;
    background: linear-gradient(
      to right,
      hsl(var(--primary) / 0.5),
      hsl(var(--border) / 0.45),
      transparent
    );
    margin: 0.75rem 0;
  }
  .text-block :global(table) {
    width: 100%;
    border-collapse: collapse;
    margin: 0.5rem 0;
    color: hsl(var(--foreground));
  }
  .text-block :global(th),
  .text-block :global(td) {
    border: 1px solid hsl(var(--border) / 0.75);
    padding: 0.35rem 0.6rem;
    text-align: left;
  }
  .text-block :global(th) {
    background: hsl(var(--secondary) / 0.65);
    font-weight: 600;
  }
  .text-block :global(tbody tr:nth-child(even)) {
    background: hsl(var(--secondary) / 0.2);
  }
  .text-block :global(tbody tr) {
    transition: background-color 120ms ease;
  }
  .text-block :global(tbody tr:hover) {
    background: hsl(var(--primary) / 0.045);
  }
  .text-block :global(a) {
    color: hsl(var(--primary));
    text-decoration-line: underline;
    text-decoration-color: hsl(var(--primary) / 0.45);
    text-underline-offset: 0.15em;
  }
  .text-block :global(a:hover) {
    text-decoration-color: hsl(var(--primary));
  }
  .text-block :global(mark) {
    color: hsl(var(--foreground));
    background: hsl(var(--warning) / 0.2);
    box-shadow: inset 0 -0.1em 0 hsl(var(--warning) / 0.28);
    border-radius: 0.15rem;
    padding: 0 0.12em;
  }
  .text-block :global(strong) {
    font-weight: 700;
  }
  .text-block :global(em) {
    font-style: italic;
  }
  .text-block :global(equation-block) {
    display: block;
    font-family: ui-monospace, monospace;
    background: var(--code-bg);
    border: 1px solid hsl(var(--border) / 0.7);
    padding: 0.75rem;
    border-radius: 0.375rem;
    overflow-x: auto;
    margin: 0.5rem 0;
    white-space: pre-wrap;
  }
  .text-block :global(equation-inline) {
    font-family: ui-monospace, monospace;
    background: color-mix(in srgb, var(--code-bg) 82%, transparent);
    border: 1px solid hsl(var(--border) / 0.55);
    padding: 0.1rem 0.3rem;
    border-radius: 0.2rem;
  }
</style>
