<script lang="ts">
  import { onMount } from "svelte";
  import { Save, FileCode, RotateCcw, Check } from "lucide-svelte";
  import * as api from "../../api";
  import { showNotification } from "../../state.svelte";

  let content = $state("");
  let filePath = $state("");
  let loading = $state(true);
  let saving = $state(false);
  let dirty = $state(false);
  let saved = $state(false);

  async function load() {
    loading = true;
    try {
      const result = await api.getConfigToml();
      content = result.content;
      filePath = result.path;
      dirty = false;
    } catch (e: any) {
      console.error("Failed to load config:", e);
      showNotification("Failed to load config", "error", 3000);
    } finally {
      loading = false;
    }
  }

  async function save() {
    if (!dirty) return;
    saving = true;
    try {
      await api.saveConfigToml(content);
      dirty = false;
      saved = true;
      setTimeout(() => saved = false, 2000);
      showNotification("Config saved", "success", 2000);
    } catch (e: any) {
      console.error("Failed to save config:", e);
      showNotification(`Failed to save: ${e?.message ?? ""}`, "error", 4000);
    } finally {
      saving = false;
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === "s" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      save();
    }
  }

  onMount(() => {
    load();
  });
</script>

<div class="flex-1 flex flex-col min-w-0 overflow-hidden">
  <!-- Header -->
  <div class="shrink-0 flex items-center justify-between px-4 py-2 border-b border-border">
    <div class="flex items-center gap-2">
      <FileCode class="w-4 h-4 text-muted-foreground" />
      <span class="text-sm font-medium">Config</span>
      <span class="text-xs text-muted-foreground truncate max-w-[300px]">{filePath}</span>
    </div>
    <div class="flex items-center gap-2">
      {#if dirty}
        <span class="text-xs text-amber-500">Modified</span>
      {/if}
      {#if saved}
        <span class="text-xs text-green-500 flex items-center gap-1">
          <Check class="w-3 h-3" />
          Saved
        </span>
      {/if}
      <button
        type="button"
        onclick={load}
        disabled={loading}
        class="inline-flex items-center gap-1 px-2.5 py-1 text-xs rounded-md border border-border hover:bg-secondary transition-colors disabled:opacity-50"
      >
        <RotateCcw class="w-3 h-3" />
        Reload
      </button>
      <button
        type="button"
        onclick={save}
        disabled={!dirty || saving}
        class="inline-flex items-center gap-1 px-2.5 py-1 text-xs rounded-md bg-primary text-primary-foreground hover:bg-primary/90 transition-colors disabled:opacity-50"
      >
        <Save class="w-3 h-3" />
        Save
      </button>
    </div>
  </div>

  <!-- Editor -->
  <div class="flex-1 min-h-0 p-4">
    {#if loading}
      <div class="flex items-center justify-center h-full">
        <div class="w-6 h-6 border-2 border-primary border-t-transparent rounded-full animate-spin"></div>
      </div>
    {:else}
      <textarea
        bind:value={content}
        oninput={() => { dirty = true; saved = false; }}
        onkeydown={handleKeydown}
        class="w-full h-full resize-none rounded-lg border border-border bg-background p-4 font-mono text-sm leading-relaxed focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        spellcheck={false}
      ></textarea>
    {/if}
  </div>
</div>
