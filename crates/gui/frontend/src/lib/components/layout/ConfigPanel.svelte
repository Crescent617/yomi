<script lang="ts">
  import { MonitorCog, Palette, SlidersHorizontal } from "lucide-svelte";
  import { appState } from "../../state.svelte";
  import ApplicationConfigPanel from "./ApplicationConfigPanel.svelte";
  import ConfigEditor from "./ConfigEditor.svelte";
  import ThemePanel from "./ThemePanel.svelte";
  import SidebarToggle from "./SidebarToggle.svelte";

  interface Props {
    onToggleLeftPanel?: () => void;
  }

  let { onToggleLeftPanel }: Props = $props();

  type Section = "application" | "themes" | "kernel";

  let section = $state<Section>("application");

  function selectSection(next: Section) {
    section = next;
  }
</script>

<div class="flex h-full min-w-0 flex-1 flex-col overflow-hidden bg-background">
  <nav
    class="flex shrink-0 items-center gap-1 border-b border-border bg-card/25 px-3 py-2 sm:px-4"
    aria-label="Configuration sections"
  >
    {#if onToggleLeftPanel}
      <SidebarToggle class="lg:hidden mr-1" onclick={onToggleLeftPanel} />
    {/if}
    <button
      type="button"
      onclick={() => selectSection("application")}
      class="group flex min-w-0 items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors {section ===
      'application'
        ? 'bg-secondary text-foreground'
        : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'}"
      aria-current={section === "application" ? "page" : undefined}
    >
      <SlidersHorizontal size={15} />
      <span class="text-xs font-medium">Application</span>
    </button>

    <button
      type="button"
      onclick={() => selectSection("themes")}
      class="group flex min-w-0 items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors {section ===
      'themes'
        ? 'bg-secondary text-foreground'
        : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'}"
      aria-current={section === "themes" ? "page" : undefined}
    >
      <Palette size={15} />
      <span class="text-xs font-medium">Themes</span>
    </button>

    <button
      type="button"
      onclick={() => selectSection("kernel")}
      class="group flex min-w-0 items-center gap-2 rounded-lg px-3 py-2 text-left transition-colors {section ===
      'kernel'
        ? 'bg-secondary text-foreground'
        : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'}"
      aria-current={section === "kernel" ? "page" : undefined}
    >
      <MonitorCog size={15} />
      <span class="text-xs font-medium">Kernel</span>
      {#if appState.config_dirty}
        <span class="h-1.5 w-1.5 rounded-full bg-warning" aria-label="Unsaved"
        ></span>
      {/if}
    </button>
  </nav>

  {#if section === "application"}
    <ApplicationConfigPanel />
  {:else if section === "themes"}
    <ThemePanel />
  {:else}
    <ConfigEditor />
  {/if}
</div>
