<script lang="ts">
  import { Menu, MonitorCog, Palette, SlidersHorizontal } from "lucide-svelte";
  import { appState } from "../../state.svelte";
  import ApplicationConfigPanel from "./ApplicationConfigPanel.svelte";
  import ConfigEditor from "./ConfigEditor.svelte";
  import ThemePanel from "./ThemePanel.svelte";

  interface Props {
    onToggleLeftPanel?: () => void;
  }

  let { onToggleLeftPanel }: Props = $props();

  type Section = "application" | "themes" | "kernel";

  let section = $state<Section>("application");
  let applicationDirty = $state(false);

  function setApplicationDirty(value: boolean) {
    applicationDirty = value;
    appState.app_config_dirty = value;
  }

  function selectSection(next: Section) {
    if (section === next) return;
    if (
      section === "application" &&
      applicationDirty &&
      !window.confirm("Discard unsaved application preference changes?")
    ) {
      return;
    }
    section = next;
  }
</script>

<div class="flex h-full min-w-0 flex-1 flex-col overflow-hidden bg-background">
  <nav
    class="flex shrink-0 items-center gap-1 border-b border-border bg-card/25 px-3 py-2 sm:px-4"
    aria-label="Configuration sections"
  >
    <button
      type="button"
      onclick={onToggleLeftPanel}
      class="mr-1 rounded-md p-1.5 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground md:hidden"
      aria-label="Show navigation"
    >
      <Menu size={18} />
    </button>
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
      {#if applicationDirty}
        <span class="h-1.5 w-1.5 rounded-full bg-warning" aria-label="Unsaved"
        ></span>
      {/if}
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
    <ApplicationConfigPanel onDirtyChange={setApplicationDirty} />
  {:else if section === "themes"}
    <ThemePanel />
  {:else}
    <ConfigEditor />
  {/if}
</div>
