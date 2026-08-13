<script lang="ts">
  import { onMount, tick } from "svelte";
  import {
    AlertCircle,
    ChevronLeft,
    ChevronRight,
    FileCode,
    RefreshCw,
    RotateCcw,
    Save,
    Zap,
  } from "lucide-svelte";
  import type { EditorView } from "codemirror";
  import * as api from "../../api";
  import { createEditor } from "../../editor/cmSetup";
  import {
    errorLineField,
    jumpToLine,
    setErrorLine,
  } from "../../editor/error-line";
  import { appState, showNotification } from "../../state.svelte";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";

  type SaveDiagnostic = {
    message: string;
    line: number | null;
    column: number | null;
  };

  let content = $state("");
  let disk_content = $state("");
  let filePath = $state("");
  let loading = $state(true);
  let reloading = $state(false);
  let saving = $state(false);
  let saveError = $state<SaveDiagnostic | null>(null);
  let full_config = $state("");

  let restarting = $state(false);
  let restartConfirmOpen = $state(false);
  let effectiveCollapsed = $state(false);

  let container = $state<HTMLElement>();
  let editor: EditorView | undefined;

  const dirty = $derived(content !== disk_content);

  const restartMessage =
    "Restart the daemon to apply config changes?\n\nAll running sessions and tasks will be interrupted. Chat history is preserved.";

  const daemonButtonTitle = $derived(
    dirty
      ? "Save config changes before restarting"
      : "Restart the daemon to apply config changes",
  );

  const saveStatus = $derived.by(() => {
    if (saving) return "Saving…";
    if (saveError) return "Save failed";
    if (dirty) return "Unsaved changes";
    if (appState.config_restart_required) {
      return "Saved to daemon · Restart required";
    }
    if (appState.config_applied) return "Applied";
    return "Saved to daemon";
  });

  const saveStatusClass = $derived(
    saveError
      ? "text-error"
      : dirty || appState.config_restart_required
        ? "text-warning"
        : appState.config_applied
          ? "text-success"
          : "text-muted-foreground",
  );

  $effect(() => {
    appState.config_dirty = dirty;
  });

  function parseSaveDiagnostic(error: unknown): SaveDiagnostic {
    const message = api.errorMessage(error);
    const location = message.match(/line\s+(\d+)\s*,\s*column\s+(\d+)/i);
    return {
      message,
      line: location ? Number(location[1]) : null,
      column: location ? Number(location[2]) : null,
    };
  }

  function flagErrorLine(line: number | null) {
    editor?.dispatch({ effects: setErrorLine.of(line) });
  }

  function jumpToDiagnostic() {
    if (!editor) return;
    if (saveError?.line) {
      jumpToLine(editor, saveError.line, saveError.column ?? 1);
    } else {
      editor.focus();
    }
  }

  function handleBeforeUnload(event: BeforeUnloadEvent) {
    if (!dirty) return;
    event.preventDefault();
    event.returnValue = "";
  }

  async function doRestartDaemon() {
    restartConfirmOpen = false;
    if (restarting || dirty) return;
    restarting = true;
    try {
      await api.restartDaemon();
      appState.config_restart_required = false;
      appState.config_applied = true;
      showNotification("Configuration applied", "success");
      const toml = await api.getConfigToml().catch(() => null);
      if (toml) full_config = toml.full_config;
    } catch (e: unknown) {
      showNotification(
        `Failed to restart daemon: ${api.errorMessage(e)}`,
        "error",
      );
    } finally {
      restarting = false;
    }
  }

  async function loadFromDisk(initial = false): Promise<boolean> {
    if (initial) loading = true;
    else reloading = true;
    const previousDiskContent = disk_content;
    try {
      const toml = await api.getConfigToml();
      content = toml.content;
      disk_content = toml.content;
      filePath = toml.path;
      full_config = toml.full_config;
      saveError = null;
      if (editor && editor.state.doc.toString() !== toml.content) {
        // Hard reload: rebuild the editor so the discarded document and
        // its undo history are both gone — a plain dispatch would let
        // Cmd+Z resurrect edits the user explicitly discarded.
        editor.destroy();
        editor = await createConfigEditor(toml.content);
      }
      flagErrorLine(null);
      if (!initial && toml.content !== previousDiskContent) {
        appState.config_restart_required = true;
        appState.config_applied = false;
      }
      return true;
    } catch (e: unknown) {
      console.error("Failed to load config:", e);
      showNotification(
        `Failed to ${initial ? "load config" : "reload from daemon"}: ${api.errorMessage(e)}`,
        "error",
      );
      return false;
    } finally {
      if (initial) loading = false;
      else reloading = false;
    }
  }

  async function reload() {
    if (loading || reloading || saving) return;
    if (
      dirty &&
      !window.confirm(
        "Reload from daemon and discard your unsaved config changes?",
      )
    ) {
      return;
    }
    if (await loadFromDisk()) {
      showNotification("Reloaded from daemon", "success");
    }
  }

  async function save() {
    if (!dirty || saving || loading || reloading) return;
    const snapshot = content;
    saving = true;
    saveError = null;
    flagErrorLine(null);
    try {
      await api.saveConfigToml(snapshot);
      disk_content = snapshot;
      appState.config_restart_required = true;
      appState.config_applied = false;
      showNotification("Config saved to daemon", "success");
    } catch (e: unknown) {
      console.error("Failed to save config:", e);
      saveError = parseSaveDiagnostic(e);
      flagErrorLine(saveError.line);
      showNotification("Save failed", "error");
    } finally {
      saving = false;
    }
  }

  async function createConfigEditor(doc: string): Promise<EditorView> {
    return createEditor(container!, {
      doc,
      filename: "config.toml",
      extensions: [errorLineField],
      onChange: (value) => {
        content = value;
        if (saveError) {
          saveError = null;
          flagErrorLine(null);
        }
      },
      onSave: () => void save(),
    });
  }

  onMount(() => {
    void (async () => {
      await loadFromDisk(true);
      await tick();
      if (!container) return;
      editor = await createConfigEditor(content);
      // A reload may have updated `content` while the async language
      // import was in flight; resync the fresh editor if they diverged.
      const created = editor.state.doc.toString();
      if (created !== content) {
        editor.dispatch({
          changes: { from: 0, to: created.length, insert: content },
        });
      }
    })();
    window.addEventListener("beforeunload", handleBeforeUnload);
    return () => {
      window.removeEventListener("beforeunload", handleBeforeUnload);
      appState.config_dirty = false;
      editor?.destroy();
    };
  });
</script>

<div class="flex-1 flex flex-col min-w-0 overflow-hidden">
  <div
    class="shrink-0 flex flex-wrap items-center justify-between gap-2 px-4 py-2 border-b border-border bg-card"
  >
    <div class="flex items-center gap-2 min-w-0">
      <FileCode class="w-4 h-4 text-muted-foreground shrink-0" />
      <span class="text-sm font-medium shrink-0">Kernel</span>
      <span class="text-xs text-muted-foreground truncate max-w-[300px]"
        >{filePath}</span
      >
    </div>

    <div class="flex flex-wrap items-center justify-end gap-2">
      {#if saveStatus}
        <span class="text-xs {saveStatusClass}">{saveStatus}</span>
      {/if}
      <button
        type="button"
        onclick={reload}
        disabled={loading || reloading || saving}
        class="inline-flex items-center gap-1 rounded-md border border-border bg-secondary/40 px-2.5 py-1 text-xs text-foreground transition-colors hover:bg-secondary/80 disabled:opacity-50"
      >
        <RotateCcw class="w-3 h-3 {reloading ? 'animate-spin' : ''}" />
        Reload
      </button>
      <button
        type="button"
        onclick={save}
        disabled={!dirty || saving || loading || reloading}
        class="inline-flex items-center gap-1 rounded-md border border-primary/30 bg-primary/10 px-2.5 py-1 text-xs font-medium text-primary transition-colors hover:border-primary/40 hover:bg-primary/15 disabled:opacity-50"
      >
        <Save class="w-3 h-3" />
        Save
      </button>
      <div class="h-5 w-px bg-border"></div>
      <span title={daemonButtonTitle}>
        <button
          type="button"
          onclick={() => (restartConfirmOpen = true)}
          disabled={restarting || dirty}
          class="inline-flex items-center gap-1 rounded-md border border-warning/30 bg-warning/10 px-2.5 py-1 text-xs font-medium text-warning transition-colors hover:border-warning/40 hover:bg-warning/15 disabled:pointer-events-none disabled:opacity-50"
        >
          <RefreshCw class="w-3 h-3 {restarting ? 'animate-spin' : ''}" />
          {restarting ? "Restarting…" : "Restart"}
        </button>
      </span>
    </div>
  </div>

  <div class="flex-1 flex flex-col md:flex-row min-h-0 overflow-hidden">
    <section
      class="order-1 flex-1 md:w-3/5 min-w-0 min-h-[18rem] flex flex-col"
      aria-label="Config editor"
    >
      {#if loading}
        <div class="flex items-center justify-center h-full">
          <div
            class="w-6 h-6 border-2 border-primary border-t-transparent rounded-full animate-spin"
          ></div>
        </div>
      {:else}
        <div
          class="relative flex-1 min-h-0 overflow-hidden bg-background focus-within:shadow-[inset_0_0_0_1px_var(--color-ring)]"
        >
          <div bind:this={container} class="absolute inset-0"></div>
        </div>

        {#if saveError}
          <button
            type="button"
            onfocus={jumpToDiagnostic}
            onclick={jumpToDiagnostic}
            class="flex w-full shrink-0 items-start gap-2 border-t border-error/30 bg-error/10 px-3 py-2 text-left text-xs text-error"
          >
            <AlertCircle class="mt-0.5 h-3.5 w-3.5 shrink-0" />
            <span>
              {#if saveError.line}
                <span class="font-medium"
                  >Line {saveError.line}, column {saveError.column ?? 1} ·
                </span>
              {/if}
              <span class="whitespace-pre-wrap">{saveError.message}</span>
            </span>
          </button>
        {/if}
      {/if}
    </section>

    <section
      class="order-2 shrink-0 min-w-0 border-t md:border-t-0 md:border-l border-border flex flex-col {effectiveCollapsed
        ? 'md:w-10'
        : 'md:w-2/5 h-1/3 md:h-auto'}"
      aria-label="Effective config"
    >
      <div
        class="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-border bg-card"
      >
        {#if !effectiveCollapsed}
          <Zap class="w-4 h-4 text-muted-foreground shrink-0" />
          <span class="text-sm font-medium truncate">Effective Config</span>
        {/if}
        <button
          type="button"
          onclick={() => (effectiveCollapsed = !effectiveCollapsed)}
          class="ml-auto rounded p-1 text-muted-foreground hover:bg-secondary hover:text-foreground"
          title={effectiveCollapsed
            ? "Expand Effective Config"
            : "Collapse Effective Config"}
        >
          {#if effectiveCollapsed}
            <ChevronLeft class="w-4 h-4" />
          {:else}
            <ChevronRight class="w-4 h-4" />
          {/if}
        </button>
      </div>
      {#if !effectiveCollapsed}
        <div class="flex-1 min-h-0 overflow-auto p-3">
          {#if loading}
            <div class="flex items-center justify-center py-8">
              <div
                class="w-4 h-4 border-2 border-primary border-t-transparent rounded-full animate-spin"
              ></div>
            </div>
          {:else if full_config}
            <pre
              class="min-w-max whitespace-pre text-xs font-mono text-muted-foreground leading-relaxed">{full_config}</pre>
          {:else}
            <div class="text-sm text-muted-foreground">
              Failed to load effective config
            </div>
          {/if}
        </div>
      {/if}
    </section>
  </div>

  <ConfirmDialog
    open={restartConfirmOpen}
    title="Restart"
    message={restartMessage}
    confirmText="Restart"
    onConfirm={doRestartDaemon}
    onCancel={() => (restartConfirmOpen = false)}
  />
</div>
