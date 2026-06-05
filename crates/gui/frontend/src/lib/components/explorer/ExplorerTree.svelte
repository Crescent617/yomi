<script lang="ts">
  import { FolderTree } from "lucide-svelte";
  import FileTree from "./FileTree.svelte";
  import type { FileEntry } from "../../fs/provider";
  import { getActiveSession, openFileTab } from "../../state.svelte";

  const activeSession = $derived(getActiveSession());

  function handleFileClick(entry: FileEntry) {
    if (!activeSession) return;
    openFileTab(activeSession, entry, "preview");
  }
</script>

<div class="flex flex-col flex-1 overflow-hidden">
  <div class="flex items-center gap-2 px-3 py-2 text-xs font-medium text-muted-foreground border-b border-border">
    <FolderTree size={14} />
    Explorer
  </div>
  <div class="flex-1 overflow-y-auto py-1">
    {#if activeSession?.projectPath}
      <FileTree path={activeSession.projectPath} onFileClick={handleFileClick} />
    {:else if activeSession}
      <div class="px-3 py-4 text-sm text-muted-foreground text-center">
        No project path
      </div>
    {:else}
      <div class="px-3 py-4 text-sm text-muted-foreground text-center">
        No active session
      </div>
    {/if}
  </div>
</div>
