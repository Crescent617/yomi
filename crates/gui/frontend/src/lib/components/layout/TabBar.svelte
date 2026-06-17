<script lang="ts">
  import { X, FileText, FileEdit } from "lucide-svelte";
  import type { Tab } from "../../state.svelte";

  let {
    tabs,
    active_tab_id,
    onSwitch,
    onClose,
  }: {
    tabs: Tab[];
    active_tab_id: string;
    onSwitch: (id: string) => void;
    onClose: (id: string) => void;
  } = $props();

  function getIcon(tab: Tab) {
    switch (tab.type) {
      case "preview":
        return FileText;
      case "edit":
        return FileEdit;
      default:
        return FileText;
    }
  }
</script>

<div
  class="flex items-center gap-0.5 px-2 border-b border-border bg-muted/30 overflow-x-auto"
>
  {#each tabs.filter((t) => t.type !== "chat") as tab (tab.id)}
    {@const Icon = getIcon(tab)}
    <button
      class="group flex items-center gap-1.5 px-3 py-2 text-xs border-b-2 transition-colors min-w-0 {tab.id ===
      active_tab_id
        ? 'border-primary bg-background text-foreground'
        : 'border-transparent text-muted-foreground hover:text-foreground hover:bg-secondary'}"
      onclick={() => onSwitch(tab.id)}
    >
      <Icon size={14} class="shrink-0" />
      <span class="truncate max-w-[120px]">{tab.label}</span>
      {#if !tab.pinned}
        <span
          role="button"
          tabindex="0"
          class="opacity-0 group-hover:opacity-100 transition-opacity ml-1 p-0.5 rounded hover:bg-secondary cursor-pointer"
          onclick={(e) => {
            e.stopPropagation();
            onClose(tab.id);
          }}
          onkeydown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.stopPropagation();
              e.preventDefault();
              onClose(tab.id);
            }
          }}
          aria-label="Close tab"
        >
          <X size={12} />
        </span>
      {/if}
    </button>
  {/each}
</div>
