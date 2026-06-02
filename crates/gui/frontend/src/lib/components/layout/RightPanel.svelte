<script lang="ts">
  import { onMount } from "svelte";
  import { ListChecks, FileDiff as FileDiffIcon } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import * as api from "../../api";
  import DiffPreview from "../diff/DiffPreview.svelte";
  import { computeFileDiff } from "../../diff/engine";
  import type { FileDiff } from "../../diff/types";

  let { session }: { session: SessionState | null } = $props();

  let activeTab = $state<"todo" | "diff">("todo");
  let todoItems = $state<{ id: string; content: string; status: string }[]>([]);
  let todoLoading = $state(false);
  let diffs = $state<{ toolId: string; path: string; diff: FileDiff }[]>([]);
  let showDiffModal = $state(false);
  let selectedDiffs = $state<FileDiff[]>([]);

  async function loadTodos() {
    if (!session) return;
    todoLoading = true;
    try {
      const result = await api.getTodos(session.id);
      todoItems = result.todos ?? [];
    } catch (e) {
      console.error("Failed to load todos:", e);
      todoItems = [];
    } finally {
      todoLoading = false;
    }
  }

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

  // Refresh both whenever session messages change (any tool call, message update, etc.)
  $effect(() => {
    const _ = session?.messages; // establish dependency on messages array
    if (!session) return;
    extractDiffs();
    loadTodos().catch(() => {});
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

<div class="w-80 flex flex-col h-full border-l border-border bg-background shrink-0">
  <!-- Tab header -->
  <div class="flex items-center border-b border-border px-2 py-1.5">
    <div class="flex items-center gap-0.5">
      <button
        onclick={() => activeTab = "todo"}
        class="flex items-center gap-1 px-2 py-1 text-xs rounded transition-colors {activeTab === 'todo' ? 'bg-secondary text-foreground' : 'text-muted-foreground hover:text-foreground hover:bg-secondary/50'}"
      >
        <ListChecks size={14} />
        Todo
      </button>
      <button
        onclick={() => activeTab = "diff"}
        class="flex items-center gap-1 px-2 py-1 text-xs rounded transition-colors {activeTab === 'diff' ? 'bg-secondary text-foreground' : 'text-muted-foreground hover:text-foreground hover:bg-secondary/50'}"
      >
        <FileDiffIcon size={14} />
        Diff
      </button>
    </div>
  </div>

  <!-- Content -->
  <div class="flex-1 overflow-y-auto p-3">
    {#if activeTab === "todo"}
      {#if todoLoading}
        <div class="flex items-center justify-center py-8">
          <div class="w-4 h-4 border-2 border-primary border-t-transparent rounded-full animate-spin"></div>
        </div>
      {:else if todoItems.length === 0}
        <div class="text-sm text-muted-foreground text-center mt-8">
          No todo items yet.
        </div>
      {:else}
        <div class="space-y-1.5">
          {#each todoItems as item (item.id)}
            <div class="flex items-start gap-2 text-sm rounded-lg px-2 py-1.5 hover:bg-secondary/40 transition-colors">
              <div class="mt-0.5 shrink-0 w-4 h-4 rounded border {item.status === 'completed' ? 'bg-green-500 border-green-500' : item.status === 'in_progress' ? 'border-amber-500' : 'border-muted-foreground'} flex items-center justify-center">
                {#if item.status === 'completed'}
                  <svg class="w-3 h-3 text-white" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="3"><path stroke-linecap="round" stroke-linejoin="round" d="M5 13l4 4L19 7" /></svg>
                {/if}
              </div>
              <span class="{item.status === 'completed' ? 'line-through text-muted-foreground' : item.status === 'in_progress' ? 'text-amber-500' : ''}">{item.content}</span>
            </div>
          {/each}
        </div>
      {/if}
    {:else}
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
