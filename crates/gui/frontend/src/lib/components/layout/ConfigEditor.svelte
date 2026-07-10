<script lang="ts">
  import { onMount } from "svelte";
  import {
    Save,
    FileCode,
    RotateCcw,
    Check,
    Zap,
    PanelLeftOpen,
    RefreshCw,
  } from "lucide-svelte";
  import * as api from "../../api";
  import { showNotification } from "../../state.svelte";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";

  let {
    onToggleLeftPanel,
  }: {
    onToggleLeftPanel?: () => void;
  } = $props();

  let content = $state("");
  let filePath = $state("");
  let loading = $state(true);
  let saving = $state(false);
  let dirty = $state(false);
  let saved = $state(false);
  let full_config = $state("");

  let daemonManaged = $state<boolean | null>(null);
  let restarting = $state(false);
  let restartConfirmOpen = $state(false);

  const restartMessage =
    "Restart the daemon to apply config changes?\n\nAll running sessions and tasks will be interrupted. Chat history is preserved.";

  const daemonButtonTitle = $derived(
    daemonManaged === null
      ? "Checking daemon status…"
      : !daemonManaged
        ? "Daemon was started externally (e.g. via CLI) — restart it from that process"
        : dirty
          ? "Save config changes before restarting"
          : "Restart the daemon to apply config changes",
  );

  async function refreshDaemonStatus() {
    try {
      const status = await api.getDaemonStatus();
      daemonManaged = status.managed;
    } catch (e: unknown) {
      console.error("Failed to get daemon status:", e);
    }
  }

  async function doRestartDaemon() {
    restartConfirmOpen = false;
    restarting = true;
    try {
      await api.restartDaemon();
      showNotification("Daemon restarted, config reloaded", "success");
      // Runtime config may have changed with the new daemon.
      const c = await api.getConfig().catch(() => null);
      full_config = c?.full_config ?? "";
    } catch (e: unknown) {
      showNotification(
        `Failed to restart daemon: ${api.errorMessage(e)}`,
        "error",
      );
    } finally {
      restarting = false;
      // Re-sync with the backend: a restart may have adopted an external
      // daemon, which flips the managed flag.
      await refreshDaemonStatus();
    }
  }

  async function load() {
    loading = true;
    try {
      const [toml, config] = await Promise.all([
        api.getConfigToml(),
        api.getConfig().catch(() => null),
      ]);
      content = toml.content;
      filePath = toml.path;
      full_config = config?.full_config ?? "";
      dirty = false;
    } catch (e: unknown) {
      console.error("Failed to load config:", e);
      showNotification("Failed to load config", "error");
    } finally {
      loading = false;
    }
  }

  async function reload() {
    loading = true;
    try {
      await load();
      showNotification("Config refreshed", "success");
    } catch (e: unknown) {
      console.error("Failed to refresh config:", e);
      showNotification(`Failed to refresh: ${api.errorMessage(e)}`, "error");
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
      setTimeout(() => (saved = false), 2000);
      showNotification(
        "Config saved. Click “Restart Daemon” to apply changes.",
        "success",
      );
      // Refresh runtime config after save
      const c = await api.getConfig().catch(() => null);
      full_config = c?.full_config ?? "";
    } catch (e: unknown) {
      console.error("Failed to save config:", e);
      showNotification(`Failed to save: ${api.errorMessage(e)}`, "error");
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
    refreshDaemonStatus();
  });
</script>

<div class="flex-1 flex flex-col min-w-0 overflow-hidden">
  <!-- Header -->
  <div
    class="shrink-0 flex items-center justify-between px-4 py-2 border-b border-border"
  >
    <div class="flex items-center gap-2">
      {#if onToggleLeftPanel}
        <button
          type="button"
          onclick={() => onToggleLeftPanel()}
          class="lg:hidden p-1.5 rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground mr-1"
          title="Toggle sidebar"
        >
          <PanelLeftOpen size={16} />
        </button>
      {/if}
      <FileCode class="w-4 h-4 text-muted-foreground" />
      <span class="text-sm font-medium">Config</span>
      <span class="text-xs text-muted-foreground truncate max-w-[300px]"
        >{filePath}</span
      >
    </div>
    <div class="flex items-center gap-2">
      {#if dirty}
        <span class="text-xs text-warning">Modified</span>
      {/if}
      {#if saved}
        <span class="text-xs text-success flex items-center gap-1">
          <Check class="w-3 h-3" />
          Saved
        </span>
      {/if}
      <button
        type="button"
        onclick={reload}
        disabled={loading}
        class="inline-flex items-center gap-1 px-2.5 py-1 text-xs rounded-md border border-border text-foreground hover:bg-secondary transition-colors disabled:opacity-50"
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
      <!-- Wrapper span so the tooltip also shows while the button is disabled -->
      <span title={daemonButtonTitle}>
        <button
          type="button"
          onclick={() => (restartConfirmOpen = true)}
          disabled={!daemonManaged || restarting || dirty}
          class="inline-flex items-center gap-1 px-2.5 py-1 text-xs font-medium rounded-md bg-warning text-warning-foreground hover:bg-warning/90 transition-colors disabled:opacity-50 disabled:pointer-events-none"
        >
          <RefreshCw class="w-3 h-3 {restarting ? 'animate-spin' : ''}" />
          {restarting ? "Restarting…" : "Restart Daemon"}
        </button>
      </span>
    </div>
  </div>

  <!-- Two-column layout: runtime config (left) + editor (right) -->
  <div class="flex-1 flex min-h-0">
    <!-- Left: Full runtime config (read-only) -->
    <div
      class="flex-1 min-w-0 border-r border-border overflow-hidden flex flex-col"
    >
      <div
        class="shrink-0 flex items-center gap-2 px-3 py-2 border-b border-border"
      >
        <Zap class="w-4 h-4 text-muted-foreground" />
        <span class="text-sm font-medium">Runtime Config</span>
      </div>
      <div class="flex-1 overflow-y-auto p-3">
        {#if loading}
          <div class="flex items-center justify-center py-8">
            <div
              class="w-4 h-4 border-2 border-primary border-t-transparent rounded-full animate-spin"
            ></div>
          </div>
        {:else if full_config}
          <pre
            class="text-xs font-mono text-muted-foreground leading-relaxed whitespace-pre-wrap">{full_config}</pre>
        {:else}
          <div class="text-sm text-muted-foreground">
            Failed to load runtime config
          </div>
        {/if}
      </div>
    </div>

    <!-- Right: Editor -->
    <div class="flex-1 min-h-0 p-4">
      {#if loading}
        <div class="flex items-center justify-center h-full">
          <div
            class="w-6 h-6 border-2 border-primary border-t-transparent rounded-full animate-spin"
          ></div>
        </div>
      {:else}
        <textarea
          bind:value={content}
          oninput={() => {
            dirty = true;
            saved = false;
          }}
          onkeydown={handleKeydown}
          class="w-full h-full resize-none rounded-lg border border-border bg-background p-4 font-mono text-sm leading-relaxed focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          spellcheck={false}
        ></textarea>
      {/if}
    </div>
  </div>

  <ConfirmDialog
    open={restartConfirmOpen}
    title="Restart Daemon"
    message={restartMessage}
    confirmText="Restart"
    onConfirm={doRestartDaemon}
    onCancel={() => (restartConfirmOpen = false)}
  />
</div>
