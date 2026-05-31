<script lang="ts">
  import { PanelRightOpen, PanelRightClose, ListChecks, FileDiff } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import { appState } from "../../state.svelte";

  let { session }: { session: SessionState | null } = $props();

  let activeTab = $state<"todo" | "diff">("todo");

  function togglePanel() {
    appState.rightPanelCollapsed = !appState.rightPanelCollapsed;
  }

  // Extract todo items from session messages (placeholder logic)
  const todoItems = $derived.by(() => {
    if (!session) return [];
    const items: { id: string; text: string; done: boolean }[] = [];
    // TODO: parse actual todo tool results from session messages
    return items;
  });
</script>

{#if appState.rightPanelCollapsed}
  <button
    type="button"
    onclick={togglePanel}
    class="flex items-center justify-center w-8 h-full border-l border-border bg-muted/30 hover:bg-muted/50 transition-colors shrink-0"
    title="Open side panel"
  >
    <PanelRightOpen size={16} class="text-muted-foreground" />
  </button>
{:else}
  <div class="w-80 flex flex-col h-full border-l border-border bg-background shrink-0">
    <!-- Tab header -->
    <div class="flex items-center justify-between border-b border-border px-2 py-1.5">
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
          <FileDiff size={14} />
          Diff
        </button>
      </div>
      <button onclick={togglePanel} class="p-1 hover:bg-secondary rounded transition-colors" title="Close panel">
        <PanelRightClose size={14} class="text-muted-foreground" />
      </button>
    </div>

    <!-- Content -->
    <div class="flex-1 overflow-y-auto p-3">
      {#if activeTab === "todo"}
        {#if todoItems.length === 0}
          <div class="text-sm text-muted-foreground text-center mt-8">
            No todo items yet.
          </div>
        {:else}
          <div class="space-y-2">
            {#each todoItems as item (item.id)}
              <div class="flex items-center gap-2 text-sm">
                <input type="checkbox" checked={item.done} class="rounded" />
                <span class={item.done ? "line-through text-muted-foreground" : ""}>{item.text}</span>
              </div>
            {/each}
          </div>
        {/if}
      {:else}
        <div class="text-sm text-muted-foreground text-center mt-8">
          Diff view will appear here.
        </div>
      {/if}
    </div>
  </div>
{/if}
