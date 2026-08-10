<script lang="ts">
  import {
    MonitorCog,
    Palette,
    Settings,
    SlidersHorizontal,
  } from "lucide-svelte";
  import { appState } from "../../state.svelte";
  import ApplicationConfigPanel from "./ApplicationConfigPanel.svelte";
  import ConfigEditor from "./ConfigEditor.svelte";
  import ThemePanel from "./ThemePanel.svelte";
  import PanelHeader from "./PanelHeader.svelte";

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
  <PanelHeader title="Config" icon={Settings} {onToggleLeftPanel}>
    {#snippet actions()}
      <nav class="flex items-center gap-1" aria-label="Configuration sections">
        <button
          type="button"
          onclick={() => selectSection("application")}
          class="group flex min-w-0 items-center gap-1.5 rounded-md px-2.5 py-1.5 text-left text-xs font-medium transition-colors {section ===
          'application'
            ? 'bg-secondary text-foreground'
            : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'}"
          aria-current={section === "application" ? "page" : undefined}
          aria-label="Application settings"
          title="Application"
        >
          <SlidersHorizontal size={15} />
          <span class="hidden sm:inline">Application</span>
        </button>

        <button
          type="button"
          onclick={() => selectSection("themes")}
          class="group flex min-w-0 items-center gap-1.5 rounded-md px-2.5 py-1.5 text-left text-xs font-medium transition-colors {section ===
          'themes'
            ? 'bg-secondary text-foreground'
            : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'}"
          aria-current={section === "themes" ? "page" : undefined}
          aria-label="Theme settings"
          title="Themes"
        >
          <Palette size={15} />
          <span class="hidden sm:inline">Themes</span>
        </button>

        <button
          type="button"
          onclick={() => selectSection("kernel")}
          class="group flex min-w-0 items-center gap-1.5 rounded-md px-2.5 py-1.5 text-left text-xs font-medium transition-colors {section ===
          'kernel'
            ? 'bg-secondary text-foreground'
            : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'}"
          aria-current={section === "kernel" ? "page" : undefined}
          aria-label="Kernel settings"
          title="Kernel"
        >
          <MonitorCog size={15} />
          <span class="hidden sm:inline">Kernel</span>
          {#if appState.config_dirty}
            <span
              class="h-1.5 w-1.5 rounded-full bg-warning"
              aria-label="Unsaved"
            ></span>
          {/if}
        </button>
      </nav>
    {/snippet}
  </PanelHeader>

  {#if section === "application"}
    <ApplicationConfigPanel />
  {:else if section === "themes"}
    <ThemePanel />
  {:else}
    <ConfigEditor />
  {/if}
</div>
