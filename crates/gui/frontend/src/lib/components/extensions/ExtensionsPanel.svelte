<script lang="ts">
  import { onMount } from "svelte";
  import {
    AlertCircle,
    ChevronRight,
    FolderOpen,
    Package,
    Plus,
    RefreshCw,
    RotateCcw,
    Trash2,
  } from "lucide-svelte";
  import {
    errorMessage,
    installExtension,
    listExtensions,
    openDefault,
    removeExtension,
    type ExtensionInfo,
  } from "../../api";
  import { automationStore } from "../../automation.svelte";
  import { extensionOps } from "../../extensions.svelte";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";
  import IconButton from "../ui/IconButton.svelte";
  import LoadingSkeleton from "../ui/LoadingSkeleton.svelte";
  import InstallExtensionDialog from "./InstallExtensionDialog.svelte";
  import {
    healthMeta,
    isLocal,
    relativeTime,
    removalSummary,
    removeNotice,
    resourceSummary,
    shortRev,
  } from "./model";

  let extensions = $state<ExtensionInfo[]>([]);
  let loading = $state(false);
  let error = $state("");
  let notice = $state("");
  let expanded = $state<string | null>(null);
  let showInstall = $state(false);
  let removeTarget = $state<ExtensionInfo | null>(null);
  let busyName = $state<string | null>(null);

  const anyCron = (list: ExtensionInfo[]) =>
    list.some((e) => (e.resources?.cron.length ?? 0) > 0);

  onMount(() => void load());

  async function load() {
    loading = true;
    error = "";
    notice = "";
    try {
      extensions = await listExtensions();
      if (expanded && !extensions.some((e) => e.name === expanded)) {
        expanded = null;
      }
    } catch (e) {
      error = errorMessage(e);
    } finally {
      loading = false;
    }
  }

  async function reinstall(ext: ExtensionInfo) {
    if (!ext.source || busyName) return;
    busyName = ext.name;
    extensionOps.busy = true;
    error = "";
    notice = "";
    try {
      await installExtension(ext.source);
      await load();
      if (anyCron(extensions)) await automationStore.load();
    } catch (e) {
      // A failed install may have partially landed on disk (the kernel's
      // documented converge-failure mode) — refresh so the list is honest.
      // Set the error after the reload: `load()` clears `error`.
      await load();
      error = errorMessage(e);
    } finally {
      busyName = null;
      extensionOps.busy = false;
    }
  }

  async function confirmRemove() {
    const ext = removeTarget;
    if (!ext || busyName) return;
    busyName = ext.name;
    extensionOps.busy = true;
    error = "";
    notice = "";
    try {
      const report = await removeExtension(ext.name);
      removeTarget = null;
      await load();
      // load() clears `notice`; set it after the reload.
      notice = removeNotice(report) ?? "";
      await automationStore.load();
    } catch (e) {
      removeTarget = null;
      await load();
      error = errorMessage(e);
    } finally {
      busyName = null;
      extensionOps.busy = false;
    }
  }

  async function closeInstall() {
    showInstall = false;
    // Reload on any dismissal too: a failed/timed-out install may have
    // partially landed on disk, so the list must not stay stale.
    await load();
  }

  async function onInstalled() {
    showInstall = false;
    await load();
    if (anyCron(extensions)) await automationStore.load();
  }
</script>

<div class="flex h-full min-h-0 flex-1 flex-col overflow-hidden">
  <!-- Section toolbar -->
  <div
    class="flex shrink-0 items-center justify-between gap-2 border-b border-border px-4 py-2 lg:px-6"
  >
    <span class="micro-label text-muted-foreground">
      {loading && extensions.length === 0
        ? "Loading"
        : `${extensions.length} installed`}
    </span>
    <div class="flex items-center gap-1.5">
      <IconButton
        label="Refresh"
        icon={RefreshCw}
        onclick={load}
        spinning={loading}
        disabled={loading || busyName !== null}
      />
      <button
        type="button"
        onclick={() => (showInstall = true)}
        class="inline-flex h-8 items-center gap-1.5 rounded-md border border-border px-3 text-xs font-medium text-foreground transition-colors hover:bg-secondary"
      >
        <Plus class="size-3.5" /> Install
      </button>
    </div>
  </div>

  <div class="min-h-0 flex-1 overflow-y-auto px-4 py-4 lg:px-6">
    {#if error}
      <div
        class="mb-4 flex items-start gap-2 rounded-md border border-error/20 bg-error/10 px-3 py-2 text-sm text-error"
        role="alert"
      >
        <AlertCircle class="mt-0.5 size-4 shrink-0" />
        <span class="min-w-0 flex-1 break-words">{error}</span>
        <button
          type="button"
          onclick={load}
          disabled={loading}
          class="shrink-0 rounded border border-error/30 px-2 py-0.5 text-xs font-medium transition-colors hover:bg-error/10 disabled:opacity-50"
        >
          Retry
        </button>
      </div>
    {/if}

    {#if notice}
      <div
        class="mb-4 flex items-start gap-2 rounded-md border border-warning/20 bg-warning/10 px-3 py-2 text-sm text-warning"
        role="status"
      >
        <AlertCircle class="mt-0.5 size-4 shrink-0" />
        <span class="min-w-0 break-words">{notice}</span>
      </div>
    {/if}

    {#if loading && extensions.length === 0}
      <div class="space-y-3" role="status" aria-label="Loading extensions">
        {#each Array(3) as _, i (i)}
          <div class="space-y-2 rounded-lg border border-border p-4">
            <LoadingSkeleton class="h-4 w-40" />
            <LoadingSkeleton class="h-3 w-3/5" />
          </div>
        {/each}
      </div>
    {:else if extensions.length === 0}
      <!-- Empty state: install guidance -->
      <div class="mx-auto max-w-xl py-10 text-center">
        <Package class="mx-auto size-8 text-muted-foreground/60" />
        <h2 class="mt-3 text-base font-semibold">No extensions installed</h2>
        <p class="mt-1 text-sm text-muted-foreground">
          An extension package bundles agent resources — scheduled jobs, shell
          commands on PATH, prompt snippets — into one thing you can install and
          remove as a unit.
        </p>

        <div
          class="mt-5 rounded-lg border border-border bg-code-bg px-4 py-3 text-left"
        >
          <div class="micro-label text-muted-foreground">Install source</div>
          <div class="mt-1.5 space-y-0.5 font-mono text-xs text-foreground">
            <div>
              <span class="text-muted-foreground">github</span> owner/repo[/subdir][@ref]
            </div>
            <div>
              <span class="text-muted-foreground">url&nbsp;&nbsp;&nbsp;</span> https://github.com/owner/repo
            </div>
            <div>
              <span class="text-muted-foreground">local&nbsp;</span> /abs/path/to/package
            </div>
          </div>
          <div class="mt-2.5 border-t border-border pt-2.5">
            <div class="micro-label text-muted-foreground">Try</div>
            <div class="mt-1 select-all font-mono text-xs text-foreground">
              Crescent617/yomi-extensions/ext/memory-system
            </div>
          </div>
        </div>

        <div class="mt-4 flex items-center justify-center gap-2">
          <button
            type="button"
            onclick={() => (showInstall = true)}
            class="inline-flex h-9 items-center gap-1.5 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
          >
            <Plus class="size-4" /> Install
          </button>
        </div>
        <p class="mt-3 text-xs text-muted-foreground">
          Don't have one? Ask the agent in chat — it can author a package for
          you.
        </p>
      </div>
    {:else}
      <ul class="space-y-2">
        {#each extensions as ext (ext.name)}
          {@const h = healthMeta(ext.health)}
          {@const isOpen = expanded === ext.name}
          {@const detailId = `ext-detail-${ext.name}`}
          <li class="rounded-lg border border-border">
            <div class="flex items-start">
              <button
                type="button"
                class="flex min-w-0 flex-1 items-start gap-3 px-4 py-3 text-left transition-colors hover:bg-secondary/40"
                aria-expanded={isOpen}
                aria-controls={detailId}
                onclick={() => (expanded = isOpen ? null : ext.name)}
              >
                <ChevronRight
                  class="mt-0.5 size-4 shrink-0 text-muted-foreground transition-transform {isOpen
                    ? 'rotate-90'
                    : ''}"
                />
                <div class="min-w-0 flex-1">
                  <div class="flex items-baseline gap-2">
                    <span class="font-mono text-sm font-medium">{ext.name}</span
                    >
                    <span class="font-mono text-xs text-muted-foreground"
                      >{ext.version}</span
                    >
                  </div>
                  {#if ext.description}
                    <div class="mt-0.5 truncate text-sm text-muted-foreground">
                      {ext.description}
                    </div>
                  {/if}
                  <div class="mt-1 flex items-center gap-2">
                    <span
                      class="truncate font-mono text-xs text-muted-foreground"
                      title={ext.source ?? ext.dir}
                    >
                      {ext.source ?? ext.dir}
                    </span>
                  </div>
                  {#if ext.resources}
                    <div class="micro-label mt-1.5 text-muted-foreground">
                      {resourceSummary(ext.resources)}
                    </div>
                  {/if}
                </div>
              </button>

              <!-- Row actions: quiet ghost icons (muted until hover), so the
                   common Reinstall/Remove need no expansion. Kept as siblings
                   of the toggle button (never nested inside it). -->
              <div
                class="flex shrink-0 items-center gap-2 self-center pl-2 pr-3"
              >
                <span class="inline-flex items-center gap-1.5 text-xs {h.text}">
                  <span class="size-1.5 rounded-full {h.dot}"></span>
                  {h.label}
                </span>
                <div class="flex items-center -space-x-2">
                  {#if ext.source && ext.health !== "foreign"}
                    <IconButton
                      label="Reinstall"
                      icon={RotateCcw}
                      onclick={() => reinstall(ext)}
                      disabled={busyName !== null}
                      spinning={busyName === ext.name}
                    />
                  {/if}
                  {#if ext.health !== "foreign"}
                    <IconButton
                      label="Remove"
                      icon={Trash2}
                      tone="destructive"
                      onclick={() => (removeTarget = ext)}
                      disabled={busyName !== null}
                    />
                  {/if}
                </div>
              </div>
            </div>

            {#if isOpen}
              <div id={detailId} class="border-t border-border px-4 py-3 pl-11">
                {#if h.hint}
                  <p class="mb-3 flex items-start gap-2 text-xs {h.text}">
                    <AlertCircle class="mt-0.5 size-3.5 shrink-0" />
                    <span>{h.hint}</span>
                  </p>
                {/if}

                <dl class="grid grid-cols-[7rem_1fr] gap-x-3 gap-y-1.5 text-xs">
                  <dt class="text-muted-foreground">Path</dt>
                  <dd class="min-w-0">
                    <span class="break-all font-mono">{ext.dir}</span>
                    <button
                      type="button"
                      class="ml-2 inline-flex items-center gap-1 align-middle text-muted-foreground hover:text-foreground"
                      onclick={() => openDefault(ext.dir).catch(() => {})}
                      title="Reveal in file manager"
                      aria-label="Reveal in file manager"
                    >
                      <FolderOpen class="size-3.5" />
                    </button>
                  </dd>

                  {#if ext.source}
                    <dt class="text-muted-foreground">Source</dt>
                    <dd class="break-all font-mono">
                      {#if isLocal(ext.source)}
                        <span class="text-muted-foreground"
                          >local:
                        </span>{ext.source}
                        <span class="ml-1 text-muted-foreground"
                          >(copy — reinstall after editing)</span
                        >
                      {:else}
                        {ext.source}
                      {/if}
                    </dd>

                    <dt class="text-muted-foreground">Revision</dt>
                    <dd class="font-mono" title={ext.rev ?? ""}>
                      {isLocal(ext.source) ? "—" : shortRev(ext.rev)}
                    </dd>

                    <dt class="text-muted-foreground">Content hash</dt>
                    <dd
                      class="truncate font-mono text-muted-foreground"
                      title={ext.content_hash ?? ""}
                    >
                      {ext.content_hash ? ext.content_hash.slice(0, 12) : "—"}
                    </dd>
                  {/if}

                  {#if ext.installed_at}
                    <dt class="text-muted-foreground">Installed</dt>
                    <dd class="text-muted-foreground">
                      {relativeTime(ext.installed_at)}
                    </dd>
                  {/if}
                </dl>

                {#if ext.resources}
                  {@const r = ext.resources}
                  <div class="mt-3 space-y-1.5">
                    {#if r.cron.length}
                      <div class="micro-label text-muted-foreground">Cron</div>
                      {#each r.cron as job (job)}
                        <div class="font-mono text-xs">{job}</div>
                      {/each}
                    {/if}
                    {#if r.hooks.length}
                      <div class="micro-label mt-2 text-muted-foreground">
                        Hooks
                      </div>
                      {#each r.hooks as hook (hook)}
                        <div class="font-mono text-xs">{hook}</div>
                      {/each}
                    {/if}
                    {#if r.bins.length}
                      <div class="micro-label mt-2 text-muted-foreground">
                        Commands on PATH
                      </div>
                      {#each r.bins as bin (bin)}
                        <div class="font-mono text-xs">{bin}</div>
                      {/each}
                    {/if}
                    {#if r.snippets.length}
                      <div class="micro-label mt-2 text-muted-foreground">
                        Prompt snippets
                      </div>
                      {#each r.snippets as snip (snip)}
                        <div class="font-mono text-xs">{snip}</div>
                      {/each}
                    {/if}
                  </div>
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</div>

{#if showInstall}
  <InstallExtensionDialog onClose={closeInstall} {onInstalled} />
{/if}

<ConfirmDialog
  open={removeTarget !== null}
  title={`Remove ${removeTarget?.name ?? ""}?`}
  message={removeTarget
    ? `This deletes the package and everything yomi installed for it. Mounts you replaced by hand are left in place.\n\n${removalSummary(removeTarget)}`
    : ""}
  confirmText={removeTarget && busyName === removeTarget.name
    ? "Removing…"
    : "Remove"}
  disabled={busyName !== null}
  onConfirm={confirmRemove}
  onCancel={() => {
    if (!busyName) removeTarget = null;
  }}
/>
