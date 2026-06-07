<script lang="ts">
  import { FileText, Folder, File, FileCode } from "lucide-svelte";
  import type { FileEntry } from "../../fs/provider";

  let {
    show,
    entries,
    selectedIdx,
    query,
    root,
    onEnter,
    onAccept,
    onClose,
  }: {
    show: boolean;
    entries: FileEntry[];
    selectedIdx: number;
    query: string;
    root: string;
    onEnter: (entry: FileEntry) => void;
    onAccept: (entry: FileEntry) => void;
    onClose: () => void;
  } = $props();

  let listRef: HTMLDivElement | null = $state(null);

  $effect(() => {
    if (show && listRef) {
      const buttons = listRef.querySelectorAll("button");
      const selected = buttons[selectedIdx];
      if (selected) {
        selected.scrollIntoView({ block: "nearest", inline: "nearest" });
      }
    }
  });

  function getFileIcon(entry: FileEntry) {
    if (entry.isDirectory) return Folder;
    const ext = entry.name.split(".").pop()?.toLowerCase();
    if (["rs", "js", "ts", "py", "go", "java", "c", "cpp", "h", "hpp"].includes(ext ?? "")) {
      return FileCode;
    }
    return File;
  }
</script>

{#if show}
  <div bind:this={listRef} class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-56 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50">
    <div class="px-3 py-1.5 text-xs text-muted-foreground border-b border-border flex items-center gap-1.5">
      <FileText size={12} />
      <span class="truncate">{query || root || "Files"}</span>
    </div>
    {#if entries.length === 0}
      <div class="px-3 py-4 text-sm text-muted-foreground text-center">No files found</div>
    {:else}
      {#each entries as entry, i (entry.path)}
        {@const Icon = getFileIcon(entry)}
        <button
          type="button"
          class="flex items-center gap-2 w-full px-3 py-1.5 text-left text-sm transition-colors {i === selectedIdx ? 'bg-secondary' : 'hover:bg-secondary/50'}"
          onclick={() => {
            if (entry.isDirectory) {
              onEnter(entry);
            } else {
              onAccept(entry);
            }
          }}
        >
          <span class="w-3.5 shrink-0"></span>
          <Icon
            size={14}
            class="shrink-0 {entry.isDirectory ? 'text-primary' : 'text-muted-foreground'}"
          />
          <span class="truncate">{entry.name}</span>
        </button>
      {/each}
    {/if}
  </div>
{/if}
