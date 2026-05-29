<script lang="ts">
  import type { FileDiff } from "../../diff/types";
  import { filterAppliedHunks } from "../../diff/engine";
  import { X } from "lucide-svelte";

  let {
    diffs,
    onApprove,
    onReject,
    onPartial,
    open = $bindable(true),
  }: {
    diffs: FileDiff[];
    onApprove: () => void;
    onReject: () => void;
    onPartial: (filtered: FileDiff[]) => void;
    open?: boolean;
  } = $props();

  let activeFileIndex = $state(0);
  let viewMode = $state<"unified" | "split">("unified");

  const activeFile = $derived(diffs[activeFileIndex]);

  function toggleHunk(hunkId: string) {
    const file = diffs[activeFileIndex];
    if (!file) return;
    const hunk = file.hunks.find((h) => h.id === hunkId);
    if (hunk) hunk.applied = !hunk.applied;
  }

  function handleApproveAll() {
    onApprove();
    open = false;
  }

  function handleApproveSelected() {
    const filtered = diffs.map((d) => filterAppliedHunks(d));
    onPartial(filtered);
    open = false;
  }

  function handleReject() {
    onReject();
    open = false;
  }

  function stats(diff: FileDiff) {
    const added = diff.hunks.reduce(
      (sum, h) => sum + h.lines.filter((l) => l.type === "add").length,
      0
    );
    const removed = diff.hunks.reduce(
      (sum, h) => sum + h.lines.filter((l) => l.type === "remove").length,
      0
    );
    return { added, removed };
  }

  function close() {
    open = false;
  }
</script>

{#if open}
<!-- Modal backdrop -->
<div
  class="fixed inset-0 z-50 flex items-center justify-center bg-black/50"
  onclick={(e) => { if (e.target === e.currentTarget) close(); }}
>
  <div class="bg-background rounded-xl shadow-xl max-w-4xl w-[90vw] h-[80vh] flex flex-col mx-4">
    <!-- Header -->
    <div class="px-6 py-4 border-b border-border flex items-center justify-between">
      <h2 class="text-lg font-semibold">Diff Preview — {diffs.length} file{diffs.length > 1 ? 's' : ''}</h2>
      <button
        onclick={close}
        class="p-1 rounded-lg hover:bg-secondary transition-colors"
      >
        <X size={18} />
      </button>
    </div>

    <!-- File tabs -->
    <div class="flex items-center gap-1 border-b border-border mt-4 overflow-x-auto px-2">
      {#each diffs as diff, i}
        <button
          class="px-3 py-1.5 text-xs rounded-t-lg transition-colors {i === activeFileIndex
            ? 'bg-primary/10 text-primary border-b-2 border-primary'
            : 'text-muted-foreground hover:bg-secondary'}"
          onclick={() => activeFileIndex = i}
        >
          {diff.path.split("/").pop()}
          {@const s = stats(diff)}
          <span class="text-green-600 ml-1">+{s.added}</span>
          <span class="text-red-600">-{s.removed}</span>
        </button>
      {/each}
    </div>

    <!-- View toggle -->
    <div class="flex items-center justify-between px-2 py-1 border-b border-border">
      <span class="text-xs text-muted-foreground">{activeFile?.path}</span>
      <div class="flex gap-1">
        <button
          class="text-xs px-2 py-1 rounded {viewMode === 'unified' ? 'bg-secondary' : ''}"
          onclick={() => viewMode = "unified"}
        >
          Unified
        </button>
        <button
          class="text-xs px-2 py-1 rounded {viewMode === 'split' ? 'bg-secondary' : ''}"
          onclick={() => viewMode = "split"}
        >
          Split
        </button>
      </div>
    </div>

    <!-- Diff content -->
    <div class="flex-1 overflow-auto font-mono text-xs">
      {#if activeFile}
        {#if viewMode === 'unified'}
          {#each activeFile.hunks as hunk (hunk.id)}
            <div class="border-l-2 {hunk.applied ? 'border-primary' : 'border-muted'} my-2">
              <!-- Hunk header -->
              <div class="flex items-center gap-2 px-2 py-1 bg-muted/50 sticky top-0">
                <input
                  type="checkbox"
                  checked={hunk.applied}
                  onclick={() => toggleHunk(hunk.id)}
                  class="rounded"
                />
                <span class="text-muted-foreground">
                  @@ -{hunk.oldStart},{hunk.oldLines} +{hunk.newStart},{hunk.newLines} @@
                </span>
              </div>

              <!-- Lines -->
              {#each hunk.lines as line}
                <div class="flex items-start gap-2 px-2 py-0.5 {line.type === 'add'
                  ? 'bg-emerald-500/10'
                  : line.type === 'remove'
                    ? 'bg-red-500/10'
                    : ''}"
                >
                  <span class="w-8 text-right text-muted-foreground select-none shrink-0">
                    {line.oldLineNum ?? ''}
                  </span>
                  <span class="w-8 text-right text-muted-foreground select-none shrink-0">
                    {line.newLineNum ?? ''}
                  </span>
                  <span class="w-4 shrink-0 select-none {line.type === 'add'
                    ? 'text-emerald-600'
                    : line.type === 'remove'
                      ? 'text-red-600'
                      : 'text-muted-foreground'}"
                  >
                    {line.type === 'add' ? '+' : line.type === 'remove' ? '-' : ' '}
                  </span>
                  <span class="flex-1 break-all">
                    {#if line.intraLineSegments}
                      {#each line.intraLineSegments as seg}
                        <span class={seg.type === 'add'
                          ? 'bg-emerald-500/30'
                          : seg.type === 'remove'
                            ? 'bg-red-500/30 line-through'
                            : ''}>
                          {seg.text}
                        </span>
                      {/each}
                    {:else}
                      {line.content}
                    {/if}
                  </span>
                </div>
              {/each}
            </div>
          {/each}
        {:else}
          <!-- Split view -->
          <div class="flex">
            <div class="flex-1 border-r border-border">
              <div class="sticky top-0 bg-muted/80 text-xs text-muted-foreground px-2 py-1">Old</div>
              {#each activeFile.hunks as hunk}
                {#each hunk.lines as line}
                  {#if line.type !== 'add'}
                    <div class="flex items-start gap-2 px-2 py-0.5 {line.type === 'remove' ? 'bg-red-500/10' : ''}">
                      <span class="w-8 text-right text-muted-foreground select-none shrink-0">{line.oldLineNum ?? ''}</span>
                      <span class="w-4 shrink-0 select-none {line.type === 'remove' ? 'text-red-600' : 'text-muted-foreground'}">-</span>
                      <span class="flex-1 break-all">{line.content}</span>
                    </div>
                  {/if}
                {/each}
              {/each}
            </div>
            <div class="flex-1">
              <div class="sticky top-0 bg-muted/80 text-xs text-muted-foreground px-2 py-1">New</div>
              {#each activeFile.hunks as hunk}
                {#each hunk.lines as line}
                  {#if line.type !== 'remove'}
                    <div class="flex items-start gap-2 px-2 py-0.5 {line.type === 'add' ? 'bg-emerald-500/10' : ''}">
                      <span class="w-8 text-right text-muted-foreground select-none shrink-0">{line.newLineNum ?? ''}</span>
                      <span class="w-4 shrink-0 select-none {line.type === 'add' ? 'text-emerald-600' : 'text-muted-foreground'}">+</span>
                      <span class="flex-1 break-all">
                        {#if line.intraLineSegments}
                          {#each line.intraLineSegments as seg}
                            <span class={seg.type === 'add' ? 'bg-emerald-500/30' : ''}>{seg.text}</span>
                          {/each}
                        {:else}
                          {line.content}
                        {/if}
                      </span>
                    </div>
                  {/if}
                {/each}
              {/each}
            </div>
          </div>
        {/if}
      {/if}
    </div>

    <!-- Action bar -->
    <div class="flex items-center justify-between border-t border-border pt-3 px-2 pb-3">
      <div class="text-xs text-muted-foreground">
        {activeFile?.hunks.filter(h => h.applied).length ?? 0} / {activeFile?.hunks.length ?? 0} hunks applied
      </div>
      <div class="flex gap-2">
        <button
          class="px-4 py-2 rounded-lg border border-border hover:bg-secondary text-sm transition-colors"
          onclick={handleReject}
        >
          Reject All
        </button>
        <button
          class="px-4 py-2 rounded-lg bg-primary text-primary-foreground hover:bg-primary/90 text-sm transition-colors"
          onclick={handleApproveSelected}
        >
          Approve Selected
        </button>
        <button
          class="px-4 py-2 rounded-lg bg-secondary text-secondary-foreground hover:bg-secondary/80 text-sm transition-colors"
          onclick={handleApproveAll}
        >
          Approve All
        </button>
      </div>
    </div>
  </div>
</div>
{/if}
