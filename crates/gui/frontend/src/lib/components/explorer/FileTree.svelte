<script lang="ts">
  import { ChevronRight, ChevronDown, Folder, FolderOpen, File, FileCode } from "lucide-svelte";
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
  // svelte-ignore state_referenced_locally
  let expanded = $state(depth === 0);
  let loaded = $state(false);

  async function load() {
    if (loaded || !path) return;
    try {
      const list = await fsProvider.listDir(path);
      // Sort: directories first, then alphabetically
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

  onMount(() => {
    if (depth === 0) load();
  });

  function toggle() {
    load();
    expanded = !expanded;
  }

  function getFileIcon(entry: FileEntry) {
    if (entry.isDirectory) return expanded ? FolderOpen : Folder;
    const ext = entry.name.split(".").pop()?.toLowerCase();
    if (["rs", "js", "ts", "py", "go", "java", "c", "cpp", "h", "hpp"].includes(ext ?? "")) {
      return FileCode;
    }
    return File;
  }

  function handleClick(entry: FileEntry) {
    if (entry.isDirectory) {
      toggle();
    } else {
      onFileClick(entry);
    }
  }
</script>

{#if depth === 0 || (expanded && loaded)}
  <div class="flex flex-col gap-0.5 {depth > 0 ? 'ml-3 border-l border-border pl-1' : ''}">
      {#each entries as entry (entry.path)}
        {@const Icon = getFileIcon(entry)}
        <div class="flex flex-col">
          <button
            class="flex items-center gap-1.5 px-2 py-1 rounded text-sm hover:bg-secondary transition-colors text-left"
            style="padding-left: {depth * 12 + 8}px"
            onclick={() => handleClick(entry)}
          >
            {#if entry.isDirectory}
              {#if expanded}
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

          {#if entry.isDirectory}
            {#if expanded}
              <FileTree path={entry.path} {onFileClick} depth={depth + 1} />
            {/if}
          {/if}
        </div>
      {/each}
  </div>
{/if}
