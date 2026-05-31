<script lang="ts">
  import { Settings, Moon, Sun, Monitor, Type, Bell, RotateCcw } from "lucide-svelte";
  import { settings, persistSettings, applyTheme } from "../../lib/settings.svelte";
  import { pushToast } from "../../lib/toast.svelte";

  let themes = [
    { id: "light" as const, label: "Light", icon: Sun },
    { id: "dark" as const, label: "Dark", icon: Moon },
    { id: "system" as const, label: "System", icon: Monitor },
  ];

  function setTheme(theme: "light" | "dark" | "system") {
    settings.theme = theme;
    applyTheme(theme);
    persistSettings(settings);
    pushToast("Theme updated", "success", 2000);
  }

  function toggleNotifications() {
    settings.notificationsEnabled = !settings.notificationsEnabled;
    persistSettings(settings);
  }

  function resetSettings() {
    settings.theme = "system";
    settings.sidebarCollapsed = false;
    settings.fontSize = "base";
    settings.notificationsEnabled = true;
    applyTheme("system");
    persistSettings(settings);
    pushToast("Settings reset to defaults", "info", 2000);
  }
</script>

<div class="p-6 max-w-2xl mx-auto">
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-2xl font-bold flex items-center gap-2">
      <Settings size={24} />
      Settings
    </h1>
    <button
      onclick={resetSettings}
      class="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg border border-border hover:bg-secondary transition-colors text-sm"
    >
      <RotateCcw size={14} />
      Reset
    </button>
  </div>

  <div class="space-y-6">
    <!-- Theme -->
    <div class="rounded-lg border border-border p-4">
      <h2 class="text-sm font-medium mb-3 flex items-center gap-2">
        <Type size={16} />
        Appearance
      </h2>
      <div class="flex gap-2">
        {#each themes as t (t.id)}
          <button
            onclick={() => setTheme(t.id)}
            class="flex items-center gap-2 px-3 py-2 rounded-lg border transition-colors text-sm {settings.theme === t.id
              ? 'border-primary bg-primary/10'
              : 'border-border hover:bg-secondary'}"
          >
            <svelte:component this={t.icon} size={16} />
            {t.label}
          </button>
        {/each}
      </div>
    </div>

    <!-- Font size -->
    <div class="rounded-lg border border-border p-4">
      <h2 class="text-sm font-medium mb-3 flex items-center gap-2">
        <Type size={16} />
        Font Size
      </h2>
      <div class="flex gap-2">
        {#each ["sm", "base", "lg"] as size (size)}
          <button
            onclick={() => { settings.fontSize = size as any; persistSettings(settings); }}
            class="px-3 py-2 rounded-lg border text-sm transition-colors {settings.fontSize === size
              ? 'border-primary bg-primary/10'
              : 'border-border hover:bg-secondary'}"
          >
            {size === "sm" ? "Small" : size === "base" ? "Medium" : "Large"}
          </button>
        {/each}
      </div>
    </div>

    <!-- Notifications -->
    <div class="rounded-lg border border-border p-4">
      <h2 class="text-sm font-medium mb-3 flex items-center gap-2">
        <Bell size={16} />
        Notifications
      </h2>
      <label class="flex items-center gap-2 cursor-pointer">
        <input
          type="checkbox"
          checked={settings.notificationsEnabled}
          onchange={toggleNotifications}
          class="rounded border-border"
        />
        <span class="text-sm">Enable desktop notifications</span>
      </label>
    </div>
  </div>
</div>
