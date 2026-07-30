<script lang="ts">
  import {
    MessageSquare,
    BarChart3,
    Bug,
    Settings,
    Sun,
    Moon,
    Monitor,
    CalendarClock,
    Star,
    PanelLeftClose,
  } from "lucide-svelte";
  import { appState, requestActivePanel } from "../../state.svelte";
  import {
    guiPreferences,
    scheduleGuiPreferencesSave,
    applyTheme,
  } from "../../settings.svelte";

  let {
    onClose,
    onNavigate,
  }: {
    /** Render a close button atop the rail (overlay drawer mode). */
    onClose?: () => void;
    /** Called after a tab was successfully activated. */
    onNavigate?: () => void;
  } = $props();

  const tabs = [
    { id: "chat", icon: MessageSquare, label: "Chat" },
    { id: "favorites", icon: Star, label: "Favorites" },
    { id: "automation", icon: CalendarClock, label: "Automation" },
    { id: "usage", icon: BarChart3, label: "Usage" },
    { id: "debug", icon: Bug, label: "Debug" },
    { id: "config", icon: Settings, label: "Config" },
  ] as const;

  function tabTitle(tab: (typeof tabs)[number]): string {
    return tab.label;
  }

  function toggleTheme() {
    const order = ["light", "dark", "system"] as const;
    const idx = order.indexOf(guiPreferences.appearance.theme);
    const nextTheme = order[(idx + 1) % 3];
    guiPreferences.appearance.theme = nextTheme;
    applyTheme(nextTheme);
    scheduleGuiPreferencesSave();
  }
</script>

<div
  class="shrink-0 w-12 border-r border-border bg-card flex flex-col items-center py-2 gap-1"
>
  {#if onClose}
    <button
      type="button"
      onclick={onClose}
      class="w-9 h-9 rounded-lg flex items-center justify-center transition-colors
             text-muted-foreground hover:text-foreground hover:bg-accent"
      title="Hide navigation"
      aria-label="Hide navigation"
    >
      <PanelLeftClose class="w-5 h-5" />
    </button>
    <div class="h-px w-6 bg-border" aria-hidden="true"></div>
  {/if}
  {#each tabs as tab (tab.id)}
    <button
      type="button"
      onclick={() => {
        if (requestActivePanel(tab.id)) onNavigate?.();
      }}
      class="w-9 h-9 rounded-lg flex items-center justify-center transition-colors
             {appState.activePanel === tab.id
        ? 'bg-primary/10 text-primary'
        : 'text-muted-foreground hover:text-foreground hover:bg-accent'}"
      title={tabTitle(tab)}
    >
      <tab.icon class="w-5 h-5" />
    </button>
  {/each}

  <div class="flex-1"></div>

  <!-- Theme toggle -->
  <button
    type="button"
    onclick={toggleTheme}
    class="w-9 h-9 rounded-lg flex items-center justify-center transition-colors
           text-muted-foreground hover:text-foreground hover:bg-accent"
    title="Toggle theme"
  >
    {#if guiPreferences.appearance.theme === "system"}
      <Monitor class="w-5 h-5" />
    {:else if guiPreferences.appearance.theme === "dark"}
      <Moon class="w-5 h-5" />
    {:else}
      <Sun class="w-5 h-5" />
    {/if}
  </button>
</div>
