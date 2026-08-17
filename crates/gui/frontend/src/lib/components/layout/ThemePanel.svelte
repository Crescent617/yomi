<script lang="ts">
  import { onDestroy } from "svelte";
  import { Check, Pencil, Plus } from "lucide-svelte";
  import LongPressDelete from "../ui/LongPressDelete.svelte";
  import { showNotification } from "../../state.svelte";
  import {
    guiPreferences,
    saveGuiPreferences,
    snapshotGuiPreferences,
  } from "../../settings.svelte";
  import {
    applyColorThemeById,
    customThemes,
    deleteCustomTheme,
    getThemeById,
    newCustomThemeDraft,
    upsertCustomTheme,
  } from "../../themes/themes.svelte";
  import {
    BUILTIN_THEMES,
    DEFAULT_THEME_ID,
    type ColorTheme,
  } from "../../themes/palettes";
  import ThemeEditor from "./ThemeEditor.svelte";

  let editing = $state<ColorTheme | null>(null);
  let editingIsNew = $state(false);
  let busy = $state(false);

  const activeId = $derived(guiPreferences.appearance.theme_id);

  // If the panel unmounts while the editor is live-previewing, restore the
  // active theme (explicit editor closes already handle this).
  onDestroy(() => {
    if (editing) applyColorThemeById(activeId);
  });

  async function persistThemeSelection(id: string) {
    const next = snapshotGuiPreferences();
    next.appearance.theme_id = id;
    await saveGuiPreferences(next);
  }

  async function selectTheme(id: string) {
    if (busy || id === activeId) return;
    busy = true;
    try {
      await persistThemeSelection(id);
    } catch (e) {
      console.error("Failed to save theme selection:", e);
      showNotification("Failed to save theme selection", "error");
    } finally {
      busy = false;
    }
  }

  function openNewTheme() {
    editing = newCustomThemeDraft(getThemeById(activeId));
    editingIsNew = true;
  }

  function openEditTheme(theme: ColorTheme) {
    // $state.snapshot unwraps the proxy into a plain, editable deep copy.
    editing = $state.snapshot(theme);
    editingIsNew = false;
  }

  async function handleEditorSave(theme: ColorTheme) {
    editing = null;
    try {
      await upsertCustomTheme(theme);
      await selectTheme(theme.id);
      showNotification(`Theme "${theme.name}" saved`, "success");
    } catch (e) {
      console.error("Failed to save custom theme:", e);
      showNotification("Failed to save custom theme", "error");
    }
  }

  function handleEditorClose() {
    editing = null;
    // Undo the editor's live preview.
    applyColorThemeById(activeId);
  }

  async function handleDelete(theme: ColorTheme) {
    if (busy || !window.confirm(`Delete theme "${theme.name}"?`)) return;
    busy = true;
    try {
      await deleteCustomTheme(theme.id);
      if (activeId === theme.id) {
        // Falls back to the default theme; bypasses selectTheme's busy guard.
        await persistThemeSelection(DEFAULT_THEME_ID);
      }
      showNotification(`Theme "${theme.name}" deleted`, "info");
    } catch (e) {
      console.error("Failed to delete custom theme:", e);
      showNotification("Failed to delete custom theme", "error");
    } finally {
      busy = false;
    }
  }
</script>

{#snippet themeCard(theme: ColorTheme)}
  {@const active = theme.id === activeId}
  <div
    class="group relative overflow-hidden rounded-xl border text-left transition-colors {active
      ? 'border-primary ring-1 ring-primary'
      : 'border-border hover:border-ring'}"
  >
    <button
      type="button"
      onclick={() => selectTheme(theme.id)}
      class="block w-full"
    >
      <!-- Split light/dark preview -->
      <span class="flex h-16 w-full">
        {#each [theme.light, theme.dark] as palette, i (i)}
          <span
            class="flex flex-1 items-center justify-center gap-1.5"
            style="background: {palette.background}"
          >
            {#each [palette.primary, palette.success, palette.error] as dot, j (j)}
              <span class="h-2.5 w-2.5 rounded-full" style="background: {dot}"
              ></span>
            {/each}
          </span>
        {/each}
      </span>
      <span
        class="flex items-center gap-1.5 border-t border-border/50 px-2.5 py-1.5"
      >
        <span class="flex-1 truncate text-xs text-foreground">{theme.name}</span
        >
        {#if active}
          <Check size={13} class="shrink-0 text-primary" />
        {/if}
      </span>
    </button>
    {#if !theme.builtin}
      <span class="absolute right-1 top-1 flex gap-0.5">
        <button
          type="button"
          onclick={() => openEditTheme(theme)}
          title="Edit theme"
          class="rounded-md bg-background/80 p-1 text-muted-foreground opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100"
        >
          <Pencil size={12} />
        </button>
        <LongPressDelete
          label="Delete theme"
          size={12}
          class="rounded-md bg-background/80 p-1 opacity-0 transition-opacity group-hover:opacity-100"
          ondelete={() => handleDelete(theme)}
        />
      </span>
    {/if}
  </div>
{/snippet}

<div class="min-h-0 min-w-0 flex-1 overflow-y-auto bg-background">
  <div class="w-full px-4 py-5 sm:px-6 lg:py-7">
    <div class="mb-5">
      <h2 class="text-base font-semibold text-foreground">Themes</h2>
      <p class="mt-0.5 text-xs text-muted-foreground">
        Color themes apply instantly and include both light and dark palettes.
        The light/dark mode itself is under Application.
      </p>
    </div>

    <section class="mb-6">
      <h3
        class="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground"
      >
        Built-in
      </h3>
      <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
        {#each BUILTIN_THEMES as theme (theme.id)}
          {@render themeCard(theme)}
        {/each}
      </div>
    </section>

    <section>
      <h3
        class="mb-2 text-xs font-semibold uppercase tracking-wide text-muted-foreground"
      >
        Custom
      </h3>
      <div class="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-4">
        {#each customThemes as theme (theme.id)}
          {@render themeCard(theme)}
        {/each}
        <button
          type="button"
          onclick={openNewTheme}
          class="flex h-[104px] flex-col items-center justify-center gap-1.5 rounded-xl border border-dashed border-border text-muted-foreground transition-colors hover:border-ring hover:text-foreground"
        >
          <Plus size={18} />
          <span class="text-xs">New theme</span>
        </button>
      </div>
    </section>
  </div>
</div>

{#if editing}
  <ThemeEditor
    theme={editing}
    isNew={editingIsNew}
    onSave={handleEditorSave}
    onClose={handleEditorClose}
  />
{/if}
