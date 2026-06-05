<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { ChevronRight, Save, Undo2 } from "lucide-svelte";
  import { fsProvider } from "../../fs/factory";
  import type { FileEntry } from "../../fs/provider";
  import { createEditor } from "../../editor/cmSetup";
  import type { EditorView } from "codemirror";

  let {
    entry,
    onClose,
  }: {
    entry: FileEntry;
    onClose?: () => void;
  } = $props();

  let container: HTMLElement;
  let editor: EditorView;
  let originalContent = $state("");
  let currentContent = $state("");
  let dirty = $derived(currentContent !== originalContent);
  let saving = $state(false);
  let error = $state("");

  function breadcrumb(path: string): string[] {
    return path.split("/").filter(Boolean);
  }

  async function save() {
    if (!dirty) return;
    saving = true;
    try {
      await fsProvider.writeFile(entry.path, currentContent);
      originalContent = currentContent;
      saving = false;
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
      saving = false;
    }
  }

  function discard() {
    if (!dirty) {
      onClose?.();
      return;
    }
    if (confirm("Discard unsaved changes?")) {
      currentContent = originalContent;
      editor.dispatch({
        changes: {
          from: 0,
          to: editor.state.doc.length,
          insert: originalContent,
        },
      });
    }
  }

  onMount(async () => {
    try {
      originalContent = await fsProvider.readFile(entry.path);
      currentContent = originalContent;

      const isDark = document.documentElement.classList.contains("dark");
      editor = await createEditor(container, {
        doc: originalContent,
        filename: entry.name,
        theme: isDark ? "dark" : "light",
        onChange: (val) => {
          currentContent = val;
        },
        onSave: save,
      });
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  });

  onDestroy(() => {
    editor?.destroy();
  });
</script>

<div class="h-full flex flex-col">
  <!-- Breadcrumb + Status Bar -->
  <div class="flex items-center gap-1 px-4 py-2 border-b border-border text-sm">
    {#each breadcrumb(entry.path) as part, i (i)}
      <span class="text-muted-foreground">{part}</span>
      {#if i < breadcrumb(entry.path).length - 1}
        <ChevronRight size={14} class="text-muted-foreground" />
      {/if}
    {/each}
    <div class="ml-auto flex items-center gap-2">
      {#if dirty}
        <span class="text-xs text-destructive">● Modified</span>
      {:else}
        <span class="text-xs text-muted-foreground">Saved</span>
      {/if}
      <button
        class="inline-flex items-center gap-1 px-2 py-1 rounded text-xs {dirty ? 'bg-primary text-primary-foreground hover:bg-primary/90' : 'bg-muted text-muted-foreground'} transition-colors"
        onclick={save}
        disabled={!dirty || saving}
      >
        <Save size={12} />
        {saving ? "Saving..." : "Save"}
      </button>
      <button
        class="inline-flex items-center gap-1 px-2 py-1 rounded text-xs hover:bg-secondary transition-colors"
        onclick={discard}
      >
        <Undo2 size={12} />
        Close
      </button>
    </div>
  </div>

  {#if error}
    <div class="text-destructive text-sm px-4 py-2">{error}</div>
  {/if}

  <!-- Editor -->
  <div bind:this={container} class="flex-1 overflow-hidden"></div>
</div>
