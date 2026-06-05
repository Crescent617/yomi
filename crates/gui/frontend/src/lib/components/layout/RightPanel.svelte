<script lang="ts">
  import { FileDiff as FileDiffIcon } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import DiffPreview from "../diff/DiffPreview.svelte";
  import { computeFileDiff } from "../../diff/engine";
  import type { FileDiff } from "../../diff/types";

  let { session }: { session: SessionState | null } = $props();

  let diffs = $state<{ toolId: string; path: string; diff: FileDiff }[]>([]);
  let showDiffModal = $state(false);
  let selectedDiffs = $state<FileDiff[]>([]);

  // Extract diffs from session messages (file_edit tool results)
  function extractDiffs() {
    if (!session) {
      diffs = [];
      return;
    }
    const items: { toolId: string; path: string; diff: FileDiff }[] = [];
    for (const msg of session.messages) {
      if (msg.role !== "assistant" || !msg.tools) continue;
      for (const tool of msg.tools) {
        if (tool.toolName !== "file_edit" || tool.status !== "completed" || !tool.arguments) continue;
        try {
          const args = JSON.parse(tool.arguments);
          const path = args.path || args.file || "";
          const oldContent = args.old_content || "";
          const newContent = args.new_content || "";
          if (!path || (oldContent === "" && newContent === "")) continue;
          const diff = computeFileDiff(path, oldContent, newContent);
          items.push({ toolId: tool.id, path, diff });
        } catch {
          // ignore parse errors
        }
      }
    }
    diffs = items;
  }

  // Refresh whenever session messages change
  $effect(() => {
    const _ = session?.messages;
    if (!session) return;
    extractDiffs();
  });

  function openDiff(diff: FileDiff) {
    selectedDiffs = [diff];
    showDiffModal = true;
  }

  function handleApprove() {
    showDiffModal = false;
  }
  function handleReject() {
    showDiffModal = false;
  }
  function handlePartial(_filtered: FileDiff[]) {
    showDiffModal = false;
  }
</script>

<div class="flex flex-col h-full bg-background shrink-0">
  <!-- Content -->
  <div class="flex-1 overflow-y-auto p-3">
    {#if diffs.length === 0}
      <div class="text-sm text-muted-foreground text-center mt-8">
        No diffs yet.
      </div>
    {:else}
      <div class="space-y-1.5">
        {#each diffs as item (item.toolId)}
          <button
            type="button"
            onclick={() => openDiff(item.diff)}
            class="w-full text-left flex items-center gap-2 text-sm rounded-lg px-2 py-1.5 hover:bg-secondary/40 transition-colors"
          >
            <FileDiffIcon size={14} class="text-muted-foreground shrink-0" />
            <span class="truncate">{item.path.split("/").pop()}</span>
            <span class="text-xs text-muted-foreground shrink-0 ml-auto">
              +{item.diff.hunks.reduce((a, h) => a + h.lines.filter(l => l.type === 'add').length, 0)}
              -{item.diff.hunks.reduce((a, h) => a + h.lines.filter(l => l.type === 'remove').length, 0)}
            </span>
          </button>
        {/each}
      </div>
    {/if}
  </div>
</div>

<!-- Diff modal -->
{#if showDiffModal}
  <DiffPreview
    diffs={selectedDiffs}
    onApprove={handleApprove}
    onReject={handleReject}
    onPartial={handlePartial}
    bind:open={showDiffModal}
  />
{/if}
