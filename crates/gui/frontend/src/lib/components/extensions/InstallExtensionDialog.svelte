<script lang="ts">
  import { open } from "@tauri-apps/plugin-dialog";
  import {
    AlertCircle,
    CheckCircle2,
    FolderOpen,
    Loader2,
    ArrowRight,
  } from "lucide-svelte";
  import {
    errorMessage,
    installExtension,
    type InstallReport,
  } from "../../api";
  import { extensionOps } from "../../extensions.svelte";
  import { CRON_ADOPT_LABEL, MOUNT_STATUS_LABEL } from "./model";
  import Modal from "../ui/Modal.svelte";

  interface Props {
    onClose: () => void;
    /** Called after a successful install and the user dismisses the report. */
    onInstalled: () => void;
  }

  let { onClose, onInstalled }: Props = $props();

  let source = $state("");
  let busy = $state(false);
  let error = $state("");
  let report = $state<InstallReport | null>(null);

  function close() {
    if (busy) return;
    if (report) onInstalled();
    else onClose();
  }

  async function pickFolder() {
    const dir = await open({ directory: true, multiple: false });
    if (typeof dir === "string") source = dir;
  }

  async function submit(event?: SubmitEvent) {
    event?.preventDefault();
    if (busy) return;
    const value = source.trim();
    if (!value) {
      error = "Enter a source (GitHub shorthand/URL or a local folder).";
      return;
    }
    busy = true;
    extensionOps.busy = true;
    error = "";
    try {
      report = await installExtension(value);
    } catch (e) {
      error = errorMessage(e);
    } finally {
      busy = false;
      extensionOps.busy = false;
    }
  }
</script>

<Modal open={true} size="md" onClose={close} title="Install extension">
  {#if report}
    <div class="space-y-4">
      <div
        class="flex items-start gap-2 rounded-md border border-success/20 bg-success/10 px-3 py-2 text-sm text-success"
      >
        <CheckCircle2 class="mt-0.5 size-4 shrink-0" />
        <span>Installed <b>{report.name}</b> v{report.version}</span>
      </div>

      <ul class="space-y-1 font-mono text-xs text-muted-foreground">
        {#each report.cron as c (c.name)}
          <li>
            cron {c.name}
            <span class="text-foreground">({CRON_ADOPT_LABEL[c.status]})</span>
          </li>
        {/each}
        {#each [...report.hooks, ...report.bins] as m (m.path)}
          <li>
            {m.path}
            <span class="text-foreground">({MOUNT_STATUS_LABEL[m.status]})</span
            >
          </li>
        {/each}
        {#if report.init}
          <li>
            init {report.init.path} ran{#if report.init.output.trim()}: {report.init.output.trim()}{/if}
          </li>
        {/if}
        {#each report.snippets as s (s)}
          <li>
            snippet {s}
            <span class="text-muted-foreground">(into system prompt)</span>
          </li>
        {/each}
      </ul>
    </div>
  {:else}
    <form id="install-extension-form" onsubmit={submit} class="space-y-4">
      <p class="-mt-1 text-sm text-muted-foreground">
        Install a package from GitHub or a local folder. Installing copies the
        package into your data directory; reinstall to update.
      </p>

      {#if error}
        <div
          class="flex items-start gap-2 rounded-md border border-error/20 bg-error/10 px-3 py-2 text-sm text-error"
          role="alert"
        >
          <AlertCircle class="mt-0.5 size-4 shrink-0" />
          <span class="min-w-0 break-words">{error}</span>
        </div>
      {/if}

      <div>
        <label for="ext-source" class="mb-1.5 block text-sm font-medium">
          Source
        </label>
        <div class="flex gap-2">
          <input
            id="ext-source"
            type="text"
            bind:value={source}
            disabled={busy}
            placeholder="owner/repo[/subdir][@ref]"
            spellcheck={false}
            class="h-9 w-full min-w-0 rounded-md border border-input bg-background px-3 font-mono text-sm outline-none transition-shadow placeholder:text-muted-foreground focus:ring-2 focus:ring-ring disabled:opacity-60"
          />
          <button
            type="button"
            onclick={pickFolder}
            disabled={busy}
            class="inline-flex h-9 shrink-0 items-center gap-1.5 rounded-md border border-border px-3 text-sm transition-colors hover:bg-secondary disabled:opacity-50"
          >
            <FolderOpen class="size-4" /> Folder…
          </button>
        </div>
        <div
          class="mt-2 space-y-1 rounded-md bg-code-bg px-3 py-2 font-mono text-xs text-muted-foreground"
        >
          <div>
            <span class="text-muted-foreground/70">github&nbsp;</span> owner/repo[/subdir][@ref]
          </div>
          <div>
            <span class="text-muted-foreground/70"
              >url&nbsp;&nbsp;&nbsp;&nbsp;</span
            > https://github.com/owner/repo
          </div>
          <div>
            <span class="text-muted-foreground/70">local&nbsp;&nbsp;</span> /abs/path/to/package
          </div>
        </div>
        <p class="mt-1.5 text-xs text-muted-foreground">
          A local folder is copied, not linked — reinstall after editing it. Use
          an absolute path (or <span class="font-mono">./</span>) to avoid
          ambiguity with <span class="font-mono">owner/repo</span>.
        </p>
      </div>
    </form>
  {/if}

  {#snippet footer()}
    {#if report}
      <button
        type="button"
        onclick={() => onInstalled()}
        class="inline-flex h-9 items-center gap-2 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90"
      >
        Done
      </button>
    {:else}
      <button
        type="button"
        onclick={close}
        disabled={busy}
        class="h-9 rounded-md border border-input px-4 text-sm transition-colors hover:bg-secondary disabled:opacity-50"
      >
        Cancel
      </button>
      <button
        type="submit"
        form="install-extension-form"
        disabled={busy}
        class="inline-flex h-9 min-w-28 items-center justify-center gap-2 rounded-md bg-primary px-4 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
      >
        {#if busy}
          <Loader2 class="size-4 animate-spin" /> Installing…
        {:else}
          Install <ArrowRight class="size-4" />
        {/if}
      </button>
    {/if}
  {/snippet}
</Modal>
