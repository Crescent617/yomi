<script lang="ts">
  import type { ChatMessage } from "../../state.svelte";
  import { Marked } from "marked";
  import OperationBar from "./OperationBar.svelte";

  let { message, session_id }: { message: ChatMessage; session_id: string } =
    $props();

  const md = new Marked();
  md.setOptions({ gfm: true, breaks: true });

  const rawRendered = $derived(
    md.parse(message.content || "", { async: false }) as string,
  );

  // Escape unknown HTML tags so they display as text (e.g. <system_reminder>)
  // while preserving markdown-generated tags like <p>, <strong>, <code>, etc.
  const allowedTags = new Set([
    "p",
    "strong",
    "b",
    "em",
    "a",
    "code",
    "pre",
    "ul",
    "ol",
    "li",
    "blockquote",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "br",
    "hr",
    "div",
    "span",
    "img",
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "sup",
    "sub",
    "del",
    "s",
  ]);
  const rendered = $derived(
    rawRendered.replace(
      /<(\/?)([a-zA-Z][a-zA-Z0-9]*)[^>]*>/g,
      (match, slash, tag) => {
        if (allowedTags.has(tag.toLowerCase())) return match;
        return match.replace(/</g, "&lt;").replace(/>/g, "&gt;");
      },
    ),
  );

  let expanded = $state(false);

  const isLong = $derived(
    (message.content || "").split("\n").length > 5 ||
      (message.content || "").length > 400 ||
      (message.content || "").includes("```"),
  );

  const hasImages = $derived(
    message.content_blocks?.some(
      (b) => b.type === "image_url" && b.image_url?.url,
    ) ?? false,
  );
</script>

<div class="flex justify-end group">
  <div
    class="max-w-[80%] lg:max-w-[70%] rounded-2xl rounded-br-sm bg-secondary px-4 py-3 text-sm user-text space-y-2 relative"
  >
    <!-- Images -->
    {#if hasImages}
      <div class="flex flex-wrap gap-2">
        {#each message.content_blocks ?? [] as block (block.type + (block.image_url?.url ?? block.text ?? ""))}
          {#if block.type === "image_url" && block.image_url?.url}
            <img
              src={block.image_url.url}
              alt="Uploaded image"
              class="max-w-[200px] max-h-[200px] rounded-lg object-cover border border-border cursor-pointer hover:opacity-90 transition-opacity"
              onclick={() => window.open(block.image_url!.url, "_blank")}
            />
          {/if}
        {/each}
      </div>
    {/if}

    <!-- Text content -->
    {#if message.content?.trim()}
      <div class:truncate={isLong && !expanded}>
        {@html rendered}
      </div>
      {#if isLong}
        <button
          type="button"
          class="mt-1 text-xs text-primary hover:underline cursor-pointer"
          onclick={() => (expanded = !expanded)}
        >
          {expanded ? "less" : "more"}
        </button>
      {/if}
    {/if}
    <div
      class="absolute left-0 -bottom-6 pl-2 opacity-0 group-hover:opacity-100 transition-opacity z-10"
    >
      <OperationBar {message} {session_id} />
    </div>
  </div>
</div>

<style>
  .user-text :global(h1) {
    font-size: 1.25rem;
    font-weight: 700;
    margin: 0.5rem 0;
  }
  .user-text :global(h2) {
    font-size: 1.125rem;
    font-weight: 600;
    margin: 0.4rem 0;
  }
  .user-text :global(h3) {
    font-size: 1rem;
    font-weight: 600;
    margin: 0.3rem 0;
  }
  .user-text :global(p) {
    margin: 0.25rem 0;
  }
  .user-text :global(ul) {
    list-style-type: disc;
    padding-left: 1.25rem;
    margin: 0.25rem 0;
  }
  .user-text :global(ol) {
    list-style-type: decimal;
    padding-left: 1.25rem;
    margin: 0.25rem 0;
  }
  .user-text :global(li) {
    margin: 0.125rem 0;
  }
  .user-text :global(pre) {
    background: hsl(var(--muted));
    padding: 0.5rem;
    border-radius: 0.375rem;
    overflow-x: auto;
    margin: 0.25rem 0;
  }
  .user-text :global(code) {
    font-family: ui-monospace, monospace;
    font-size: 0.875rem;
  }
  .user-text :global(pre code) {
    background: transparent;
    padding: 0;
  }
  .user-text :global(:not(pre) > code) {
    background: hsl(var(--muted));
    padding: 0.125rem 0.25rem;
    border-radius: 0.25rem;
  }
  .user-text :global(blockquote) {
    border-left: 3px solid hsl(var(--border));
    padding-left: 0.75rem;
    margin: 0.25rem 0;
    color: hsl(var(--muted-foreground));
  }
  .user-text :global(hr) {
    border: 0;
    border-top: 1px solid hsl(var(--border));
    margin: 0.5rem 0;
  }
  .user-text :global(table) {
    width: 100%;
    border-collapse: collapse;
    margin: 0.25rem 0;
  }
  .user-text :global(th),
  .user-text :global(td) {
    border: 1px solid hsl(var(--border));
    padding: 0.25rem 0.5rem;
    text-align: left;
  }
  .user-text :global(th) {
    background: hsl(var(--muted));
    font-weight: 600;
  }
  .user-text :global(a) {
    color: hsl(var(--primary));
    text-decoration: underline;
  }
  .user-text :global(strong) {
    font-weight: 700;
  }
  .user-text :global(em) {
    font-style: italic;
  }

  .truncate {
    max-height: 120px;
    overflow: hidden;
  }
</style>
