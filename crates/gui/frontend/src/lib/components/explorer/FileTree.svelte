<script lang="ts">
  import { ChevronRight, ChevronDown, Folder, File, FileCode } from "lucide-svelte";
  import { fsProvider } from "../../fs/factory";
  import type { FileEntry } from "../../fs/provider";
  import { onMount } from "svelte";
  import FileTree from "./FileTree.svelte";

  let {
    path,
    onFileClick,
    depth = 0,
  }: {
    path: string;
    onFileClick: (entry: FileEntry) => void;
    depth?: number;
  } = $props();

  let entries = $state<FileEntry[]>([]);
  let expanded = $state<Set<string>>(new Set());
  let loaded = $state(false);

  async function load() {
    if (loaded || !path) return;
    try {
      const list = await fsProvider.listDir(path);
      entries = list.sort((a, b) => {
        if (a.isDirectory && !b.isDirectory) return -1;
        if (!a.isDirectory && b.isDirectory) return 1;
        return a.name.localeCompare(b.name);
      });
      loaded = true;
    } catch (e) {
      console.error("Failed to list dir:", path, e);
    }
  }

  async function init() {
    await load();
    if (depth === 0 && expanded.size === 0) {
      const firstDir = entries.find(e => e.isDirectory);
      if (firstDir) {
        expanded = new Set([firstDir.path]);
      }
    }
  }

  onMount(() => {
    if (depth === 0) init();
  });

  function toggleDir(entryPath: string) {
    load();
    const next = new Set(expanded);
    if (next.has(entryPath)) next.delete(entryPath);
    else next.add(entryPath);
    expanded = next;
  }

  function getFileIcon(entry: FileEntry) {
    if (entry.isDirectory) return Folder;
    const ext = entry.name.split(".").pop()?.toLowerCase();
    if (["rs", "js", "ts", "py", "go", "java", "c", "cpp", "h", "hpp"].includes(ext ?? "")) {
      return FileCode;
    }
    return File;
  }

  function handleClick(entry: FileEntry) {
    if (entry.isDirectory) {
      toggleDir(entry.path);
    } else {
      onFileClick(entry);
    }
  }
</script>

{#if depth === 0 || loaded}
  <div class="flex flex-col gap-0.5 {depth > 0 ? 'ml-3 border-l border-border pl-1' : ''}">
    {#each entries as entry (entry.path)}
      {@const isExpanded = expanded.has(entry.path)}
      {@const Icon = getFileIcon(entry)}
      <div class="flex flex-col">
        <button
          class="flex items-center gap-1.5 px-2 py-1 rounded text-sm hover:bg-secondary transition-colors text-left"
          style="padding-left: {depth * 12 + 8}px"
          onclick={() => handleClick(entry)}
        >
          {#if entry.isDirectory}
            {#if isExpanded}
              <ChevronDown size={14} class="shrink-0 text-muted-foreground" />
            {:else}
              <ChevronRight size={14} class="shrink-0 text-muted-foreground" />
            {/if}
          {:else}
            <span class="w-3.5 shrink-0"></span>
          {/if}

          <Icon
            size={14}
            class="shrink-0 {entry.isDirectory ? 'text-primary' : 'text-muted-foreground'}"
          />

          <span class="truncate {entry.isDirectory ? 'font-medium' : ''}">{entry.name}</span>
        </button>

        {#if entry.isDirectory && isExpanded}
          <FileTree path={entry.path} {onFileClick} depth={depth + 1} />
        {/if}
      </div>
    {/each}
  </div>
{/if}
