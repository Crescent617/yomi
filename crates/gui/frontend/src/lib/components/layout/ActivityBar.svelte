<script lang="ts">
  import { MessageSquare, BarChart3, Settings, Sun, Moon } from "lucide-svelte";
  import { appState } from "../../state.svelte";
  import { settings, applyTheme, persistSettings } from "../../settings.svelte";

  const tabs = [
    { id: "chat", icon: MessageSquare, label: "Chat" },
    { id: "usage", icon: BarChart3, label: "Usage" },
    { id: "config", icon: Settings, label: "Config" },
  ] as const;

  function toggleTheme() {
    const next = settings.theme === "dark" ? "light" : "dark";
    settings.theme = next;
    applyTheme(next);
    persistSettings(settings);
  }

  const ThemeIcon = $derived(settings.theme === "dark" ? Sun : Moon);
</script>

<div class="shrink-0 w-12 border-r border-border bg-muted/30 flex flex-col items-center py-2 gap-1">
  {#each tabs as tab (tab.id)}
    <button
      type="button"
      onclick={() => appState.activePanel = tab.id}
      class="w-9 h-9 rounded-lg flex items-center justify-center transition-colors
             {appState.activePanel === tab.id
               ? 'bg-primary/10 text-primary'
               : 'text-muted-foreground hover:text-foreground hover:bg-secondary/50'}"
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
           text-muted-foreground hover:text-foreground hover:bg-secondary/50"
    title="Toggle theme"
  >
    <ThemeIcon class="w-5 h-5" />
  </button>
</div>
