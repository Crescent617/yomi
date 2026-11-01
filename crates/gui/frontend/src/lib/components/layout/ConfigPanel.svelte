<script lang="ts">
  import {
    Blocks,
    MonitorCog,
    Palette,
    Settings,
    SlidersHorizontal,
  } from "lucide-svelte";
  import { appState, type ConfigSection } from "../../state.svelte";
  import { extensionOps } from "../../extensions.svelte";
  import ApplicationConfigPanel from "./ApplicationConfigPanel.svelte";
  import ConfigEditor from "./ConfigEditor.svelte";
  import ThemePanel from "./ThemePanel.svelte";
  import ExtensionsPanel from "../extensions/ExtensionsPanel.svelte";
  import PanelHeader from "./PanelHeader.svelte";

  interface Props {
    onToggleLeftPanel?: () => void;
  }

  let { onToggleLeftPanel }: Props = $props();

  // Section lives in app state so other surfaces (command palette) can
  // deep-link straight to a section.
  const sections: {
    id: ConfigSection;
    label: string;
    icon: typeof Settings;
    hint: string;
  }[] = [
    {
      id: "application",
      label: "Application",
      icon: SlidersHorizontal,
      hint: "Application settings",
    },
    { id: "themes", label: "Themes", icon: Palette, hint: "Theme settings" },
    {
      id: "extensions",
      label: "Extensions",
      icon: Blocks,
      hint: "Installed extension packages",
    },
    {
      id: "kernel",
      label: "Kernel",
      icon: MonitorCog,
      hint: "Kernel settings",
    },
  ];
</script>

<div class="flex h-full min-w-0 flex-1 flex-col overflow-hidden bg-background">
  <PanelHeader title="Config" icon={Settings} {onToggleLeftPanel}>
    {#snippet actions()}
      <nav class="flex items-center gap-1" aria-label="Configuration sections">
        {#each sections as s (s.id)}
          {@const Icon = s.icon}
          <button
            type="button"
            onclick={() => {
              // Don't abandon an in-flight extension install/remove by
              // unmounting its panel mid-flight.
              if (extensionOps.busy) return;
              appState.configSection = s.id;
            }}
            disabled={extensionOps.busy}
            title={extensionOps.busy
              ? "Extension operation in progress…"
              : s.label}
            class="group flex min-w-0 items-center gap-1.5 rounded-md px-2.5 py-1.5 text-left text-xs font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-50 {appState.configSection ===
            s.id
              ? 'bg-secondary text-foreground'
              : 'text-muted-foreground hover:bg-secondary/60 hover:text-foreground'}"
            aria-current={appState.configSection === s.id ? "page" : undefined}
            aria-label={s.hint}
          >
            <Icon size={15} />
            <span class="hidden sm:inline">{s.label}</span>
            {#if s.id === "kernel" && appState.config_dirty}
              <span
                class="h-1.5 w-1.5 rounded-full bg-warning"
                aria-label="Unsaved"
              ></span>
            {/if}
          </button>
        {/each}
      </nav>
    {/snippet}
  </PanelHeader>

  {#if appState.configSection === "application"}
    <ApplicationConfigPanel />
  {:else if appState.configSection === "themes"}
    <ThemePanel />
  {:else if appState.configSection === "extensions"}
    <ExtensionsPanel />
  {:else}
    <ConfigEditor />
  {/if}
</div>
