<script lang="ts">
  import {
    MessageSquare,
    BarChart3,
    Settings,
    Sun,
    Moon,
    Monitor,
    CalendarClock,
  } from "lucide-svelte";
  import { appState, requestActivePanel } from "../../state.svelte";
  import {
    guiPreferences,
    snapshotGuiPreferences,
    saveGuiPreferences,
    applyTheme,
  } from "../../settings.svelte";

  const tabs = [
    { id: "chat", icon: MessageSquare, label: "Chat" },
    { id: "automation", icon: CalendarClock, label: "Automation" },
    { id: "usage", icon: BarChart3, label: "Usage" },
    { id: "config", icon: Settings, label: "Config" },
  ] as const;

  function toggleTheme() {
    const order = ["light", "dark", "system"] as const;
    const idx = order.indexOf(guiPreferences.appearance.theme);
    const nextTheme = order[(idx + 1) % 3];
    const next = snapshotGuiPreferences();
    next.appearance.theme = nextTheme;
    applyTheme(nextTheme);
    void saveGuiPreferences(next);
  }
</script>

<div
  class="shrink-0 w-12 border-r border-border bg-card flex flex-col items-center py-2 gap-1"
>
  {#each tabs as tab (tab.id)}
    <button
      type="button"
      onclick={() => requestActivePanel(tab.id)}
      class="w-9 h-9 rounded-lg flex items-center justify-center transition-colors
             {appState.activePanel === tab.id
        ? 'bg-primary/10 text-primary'
        : 'text-muted-foreground hover:text-foreground hover:bg-accent'}"
      title={tab.label}
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
