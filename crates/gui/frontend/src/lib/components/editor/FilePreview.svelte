<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { ChevronRight, Pencil, MessageSquare } from "lucide-svelte";
  import { fsProvider } from "../../fs/factory";
  import type { FileEntry } from "../../fs/provider";
  import { detectLang } from "../../utils";

  let {
    entry,
    onEdit,
    onAskAI,
  }: {
    entry: FileEntry;
    onEdit?: (entry: FileEntry) => void;
    onAskAI?: (path: string) => void;
  } = $props();

  let content = $state("");
  let highlighted = $state("");
  let loading = $state(true);
  let error = $state("");
  let highlighter: any = null;

  function breadcrumb(path: string): string[] {
    return path.split("/").filter(Boolean);
  }

  onMount(async () => {
    try {
      content = await fsProvider.readFile(entry.path);
      highlighted = content; // Fallback
      loading = false;

      // Lazy load Shiki
      const shiki = await import("shiki");
      const lang = detectLang(entry.name);
      highlighter = await shiki.createHighlighter({
        themes: ["github-light", "github-dark"],
        langs: [lang],
      });
      highlighted = highlighter.codeToHtml(content, {
        lang,
        theme: "github-dark",
      });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      loading = false;
    }
  });

  onDestroy(() => {
    highlighter?.dispose();
  });
</script>

<div class="h-full flex flex-col">
  <!-- Breadcrumb + Actions -->
  <div class="flex items-center gap-1 px-4 py-2 border-b border-border text-sm">
    {#each breadcrumb(entry.path) as part, i}
      <span class="text-muted-foreground">{part}</span>
      {#if i < breadcrumb(entry.path).length - 1}
        <ChevronRight size={14} class="text-muted-foreground" />
      {/if}
    {/each}
    <div class="ml-auto flex gap-1">
      {#if onEdit}
        <button
          class="inline-flex items-center gap-1 px-2 py-1 rounded text-xs hover:bg-secondary transition-colors"
          onclick={() => onEdit(entry)}
        >
          <Pencil size={12} />
          Edit
        </button>
      {/if}
      {#if onAskAI}
        <button
          class="inline-flex items-center gap-1 px-2 py-1 rounded text-xs hover:bg-secondary transition-colors"
          onclick={() => onAskAI(entry.path)}
        >
          <MessageSquare size={12} />
          Ask AI
        </button>
      {/if}
    </div>
  </div>

  <!-- Content -->
  <div class="flex-1 overflow-auto p-4">
    {#if loading}
      <div class="text-muted-foreground text-sm">Loading...</div>
    {:else if error}
      <div class="text-destructive text-sm">{error}</div>
    {:else}
      <div class="text-sm">
        {@html highlighted}
      </div>
    {/if}
  </div>
</div>
