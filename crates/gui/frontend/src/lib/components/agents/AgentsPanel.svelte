<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import type { AgentTemplateInfo, AgentTemplateScope } from "../../api";
  import { sessionState } from "../../state.svelte";
  import { pushToast } from "../../toast.svelte";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";
  import InlineLoadingStatus from "../ui/InlineLoadingStatus.svelte";
  import LoadingSkeleton from "../ui/LoadingSkeleton.svelte";
  import SidebarToggle from "../layout/SidebarToggle.svelte";
  import { Bot, Plus, RefreshCw, Trash2, Copy } from "lucide-svelte";

  let {
    onToggleLeftPanel,
  }: {
    onToggleLeftPanel?: () => void;
  } = $props();

  const NAME_RE = /^[a-z0-9][a-z0-9-]{0,63}$/;
  const NEW_STUB =
    "# Role\n\nYou are a specialist. Describe the role's responsibilities, principles, and output expectations here.\n";

  let templates = $state<AgentTemplateInfo[]>([]);
  let loading = $state(true);
  let loadError = $state("");

  // Selection & editing
  let selectedName = $state<string | null>(null);
  let draft = $state("");
  let saving = $state(false);
  let actionError = $state("");

  // New-template form
  let creating = $state(false);
  let newName = $state("");
  let newScope = $state<AgentTemplateScope>("global");

  // Dialogs
  let deleteTarget = $state<AgentTemplateInfo | null>(null);
  let discardPending = $state<(() => void) | null>(null);

  const sessionId = $derived(sessionState.activeSessionId ?? undefined);
  const selected = $derived(templates.find((t) => t.name === selectedName));
  const dirty = $derived(selected !== undefined && draft !== selected?.body);

  const groups = $derived.by(() => {
    const by: Record<string, AgentTemplateInfo[]> = {
      builtin: [],
      global: [],
      workspace: [],
    };
    for (const t of templates) by[t.source]?.push(t);
    return by;
  });

  const nameError = $derived.by(() => {
    if (!creating) return "";
    if (!NAME_RE.test(newName))
      return "Use kebab-case: ^[a-z0-9][a-z0-9-]{0,63}$";
    const hit = templates.find((t) => t.name === newName);
    if (hit && hit.source === newScope)
      return `"${newName}" already exists in ${newScope}`;
    return "";
  });

  const overrideNote = $derived.by(() => {
    if (!creating || nameError) return "";
    const hit = templates.find((t) => t.name === newName);
    return hit ? `Will override the ${hit.source} template` : "";
  });

  async function load() {
    try {
      loadError = "";
      templates = await api.listAgentTemplates(sessionId);
    } catch (e) {
      loadError = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  onMount(load);

  /** Guard against losing an unsaved draft when switching selection. */
  function guarded(action: () => void) {
    if (dirty) {
      discardPending = action;
    } else {
      action();
    }
  }

  function select(t: AgentTemplateInfo) {
    guarded(() => {
      creating = false;
      actionError = "";
      selectedName = t.name;
      draft = t.body;
    });
  }

  function startCreate(prefill?: AgentTemplateInfo) {
    guarded(() => {
      selectedName = null;
      actionError = "";
      creating = true;
      newName = prefill?.name ?? "";
      newScope = "global";
      draft = prefill?.body ?? NEW_STUB;
    });
  }

  async function save() {
    const name = creating ? newName : selected?.name;
    const scope: AgentTemplateScope = creating
      ? newScope
      : selected?.source === "workspace"
        ? "workspace"
        : "global";
    if (!name || !draft.trim()) {
      actionError = "Body must not be empty";
      return;
    }
    if (creating && nameError) {
      actionError = nameError;
      return;
    }
    try {
      saving = true;
      actionError = "";
      await api.saveAgentTemplate(name, draft, scope, sessionId);
      const keep = name;
      await load();
      creating = false;
      selectedName = keep;
      draft = templates.find((t) => t.name === keep)?.body ?? draft;
      pushToast(`Template "${keep}" saved`, "success");
    } catch (e) {
      actionError = e instanceof Error ? e.message : String(e);
    } finally {
      saving = false;
    }
  }

  async function remove(t: AgentTemplateInfo) {
    deleteTarget = null;
    try {
      await api.deleteAgentTemplate(
        t.name,
        t.source === "workspace" ? "workspace" : "global",
        sessionId,
      );
      if (selectedName === t.name) {
        selectedName = null;
        draft = "";
      }
      await load();
      pushToast(`Template "${t.name}" deleted`, "success");
    } catch (e) {
      actionError = e instanceof Error ? e.message : String(e);
    }
  }

  const sourceBadge: Record<string, string> = {
    builtin: "bg-secondary text-muted-foreground",
    global: "bg-info/10 text-info",
    workspace: "bg-primary/10 text-primary",
  };
</script>

<div class="flex-1 flex flex-col min-w-0 overflow-hidden">
  <!-- Header -->
  <div
    class="flex h-14 shrink-0 items-center gap-2 border-b border-border px-4"
  >
    {#if onToggleLeftPanel}
      <SidebarToggle class="lg:hidden" onclick={() => onToggleLeftPanel()} />
    {/if}
    <Bot class="w-5 h-5 text-primary" />
    <h2 class="text-lg font-semibold">Agent Templates</h2>
    <span class="text-xs text-muted-foreground ml-1">
      · role prompts for subagent spawn
    </span>
    <div class="flex-1"></div>
    {#if loading && templates.length > 0}
      <InlineLoadingStatus label="Refreshing" />
    {/if}
    <button
      type="button"
      onclick={() => startCreate()}
      class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border hover:bg-secondary transition-colors text-sm"
    >
      <Plus size={14} />
      New
    </button>
    <button
      type="button"
      onclick={() => {
        loading = true;
        load();
      }}
      class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border hover:bg-secondary transition-colors text-sm"
      aria-label="Refresh templates"
    >
      <RefreshCw size={14} />
    </button>
  </div>

  <div class="flex-1 flex min-h-0">
    <!-- List -->
    <div class="w-64 shrink-0 border-r border-border overflow-y-auto p-3">
      {#if loading && templates.length === 0}
        <div class="space-y-2" role="status" aria-label="Loading templates">
          {#each Array(4) as _, i (i)}
            <LoadingSkeleton class="h-8 w-full" />
          {/each}
        </div>
      {:else if loadError}
        <div class="text-sm text-destructive p-2">{loadError}</div>
      {:else}
        {#each [["builtin", "Builtin"], ["global", "Global"], ["workspace", "Workspace"]] as [key, label] (key)}
          {@const items = groups[key]}
          {#if items.length > 0}
            <div class="micro-label px-2 pt-3 pb-1 text-muted-foreground">
              {label}
            </div>
            {#each items as t (t.name)}
              <button
                type="button"
                onclick={() => select(t)}
                class="w-full text-left px-2 py-1.5 rounded-md font-mono text-sm truncate transition-colors
                  {selectedName === t.name && !creating
                  ? 'bg-primary/10 text-primary'
                  : 'hover:bg-secondary'}"
              >
                {t.name}
              </button>
            {/each}
          {/if}
        {/each}
        {#if groups.global.length === 0 && groups.workspace.length === 0}
          <div class="px-2 pt-4 text-xs text-muted-foreground">
            No custom templates yet — duplicate a builtin to get started.
          </div>
        {/if}
      {/if}
    </div>

    <!-- Detail / editor -->
    <div class="flex-1 flex flex-col min-w-0 p-4 gap-3">
      {#if creating}
        <div class="flex items-center gap-3 flex-wrap">
          <input
            type="text"
            bind:value={newName}
            placeholder="template-name"
            class="w-56 px-3 py-1.5 rounded-lg border border-input bg-background font-mono text-sm focus:outline-none focus:ring-1 focus:ring-primary"
          />
          <div class="flex items-center gap-3 text-sm">
            <label class="flex items-center gap-1.5">
              <input type="radio" bind:group={newScope} value="global" />
              Global
            </label>
            <label
              class="flex items-center gap-1.5 {!sessionId && 'opacity-50'}"
              title={sessionId
                ? ""
                : "Needs an active session for workspace context"}
            >
              <input
                type="radio"
                bind:group={newScope}
                value="workspace"
                disabled={!sessionId}
              />
              Workspace
            </label>
          </div>
          {#if overrideNote}
            <span class="text-xs text-warning">{overrideNote}</span>
          {/if}
        </div>
        <textarea
          bind:value={draft}
          spellcheck="false"
          class="flex-1 w-full font-mono text-sm p-3 rounded-lg border border-border bg-background resize-none focus:outline-none focus:ring-1 focus:ring-primary"
        ></textarea>
      {:else if selected}
        <div class="flex items-center gap-2">
          <span class="font-mono text-sm font-semibold">{selected.name}</span>
          <span
            class="px-1.5 py-0.5 rounded text-[11px] font-mono {sourceBadge[
              selected.source
            ]}"
          >
            {selected.source}
          </span>
          {#if selected.source === "builtin"}
            <span class="text-xs text-muted-foreground">
              read-only — duplicate to customize
            </span>
          {/if}
        </div>
        <textarea
          bind:value={draft}
          readonly={selected.source === "builtin"}
          spellcheck="false"
          class="flex-1 w-full font-mono text-sm p-3 rounded-lg border border-border bg-background resize-none focus:outline-none focus:ring-1 focus:ring-primary
            {selected.source === 'builtin' ? 'opacity-70' : ''}"
        ></textarea>
      {:else}
        <div
          class="flex-1 flex items-center justify-center text-sm text-muted-foreground"
        >
          Select a template to view, or create a new one.
        </div>
      {/if}

      {#if creating || selected}
        <div class="flex items-center gap-2 shrink-0">
          {#if actionError}
            <span class="text-xs text-destructive mr-2">{actionError}</span>
          {/if}
          <div class="flex-1"></div>
          {#if creating}
            <button
              type="button"
              onclick={() => {
                creating = false;
                actionError = "";
              }}
              class="px-3 py-1.5 rounded-lg border border-border hover:bg-secondary transition-colors text-sm"
            >
              Cancel
            </button>
          {:else if selected?.source === "builtin"}
            <button
              type="button"
              onclick={() => selected && startCreate(selected)}
              class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border hover:bg-secondary transition-colors text-sm"
            >
              <Copy size={14} />
              Duplicate to Global
            </button>
          {:else if selected}
            <button
              type="button"
              onclick={() => (deleteTarget = selected)}
              class="inline-flex items-center gap-1.5 px-3 py-1.5 rounded-lg border border-border text-destructive hover:bg-destructive/10 transition-colors text-sm"
            >
              <Trash2 size={14} />
              Delete
            </button>
          {/if}
          <button
            type="button"
            onclick={save}
            disabled={saving || (creating ? !!nameError : !dirty)}
            class="px-4 py-1.5 rounded-lg bg-primary text-primary-foreground hover:bg-primary/90 transition-colors text-sm disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {saving ? "Saving…" : creating ? "Create" : "Save"}
          </button>
        </div>
      {/if}
    </div>
  </div>
</div>

<ConfirmDialog
  open={deleteTarget !== null}
  title="Delete template"
  message={deleteTarget
    ? `Delete "${deleteTarget.name}" (${deleteTarget.source})?${
        deleteTarget.source !== "builtin"
          ? "\nIf it overrides a lower layer, that version becomes effective again."
          : ""
      }`
    : ""}
  confirmText="Delete"
  onConfirm={() => deleteTarget && remove(deleteTarget)}
  onCancel={() => (deleteTarget = null)}
/>

<ConfirmDialog
  open={discardPending !== null}
  title="Discard changes"
  message="You have unsaved changes. Discard them?"
  confirmText="Discard"
  onConfirm={() => {
    discardPending?.();
    discardPending = null;
  }}
  onCancel={() => (discardPending = null)}
/>
