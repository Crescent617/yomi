<script lang="ts">
  import { X, FileText, FileEdit } from "lucide-svelte";
  import type { Tab } from "../../state.svelte";

  let {
    tabs,
    activeTabId,
    onSwitch,
    onClose,
  }: {
    tabs: Tab[];
    activeTabId: string;
    onSwitch: (id: string) => void;
    onClose: (id: string) => void;
  } = $props();

  function getIcon(tab: Tab) {
    switch (tab.type) {
      case "preview": return FileText;
      case "edit": return FileEdit;
      default: return FileText;
    }
  }
</script>

<div class="flex items-center gap-0.5 px-2 border-b border-border bg-muted/30 overflow-x-auto">
  {#each tabs.filter(t => t.type !== "chat") as tab (tab.id)}
    <button
      class="group flex items-center gap-1.5 px-3 py-2 text-xs border-b-2 transition-colors min-w-0 {tab.id === activeTabId
        ? 'border-primary bg-background text-foreground'
        : 'border-transparent text-muted-foreground hover:text-foreground hover:bg-secondary'}"
      onclick={() => onSwitch(tab.id)}
    >
      <svelte:component this={getIcon(tab)} size={14} class="shrink-0" />
      <span class="truncate max-w-[120px]">{tab.label}</span>
      {#if !tab.pinned}
        <span
          class="opacity-0 group-hover:opacity-100 transition-opacity ml-1"
          onclick={(e) => { e.stopPropagation(); onClose(tab.id); }}
        >
          <X size={12} />
        </span>
      {/if}
    </button>
  {/each}
</div>
