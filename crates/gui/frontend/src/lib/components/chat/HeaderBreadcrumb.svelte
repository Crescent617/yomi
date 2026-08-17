<script lang="ts">
  import {
    Check,
    ChevronDown,
    ChevronRight,
    Folder,
    Loader2,
    Pencil,
    Search,
  } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import {
    getSession,
    projectState,
    showNotification,
  } from "../../state.svelte";
  import { activateSession } from "../../session";
  import * as api from "../../api";
  import { formatTimeAgo, focusAndSelect } from "../../utils";
  import { buildSessionBreadcrumb } from "./session-breadcrumb";

  let { session }: { session: SessionState } = $props();

  type Menu = "project" | "session" | null;
  type SessionOption = {
    id: string;
    title: string;
    updated_at: string;
  };

  let menu = $state<Menu>(null);
  let menuRef = $state<HTMLDivElement | null>(null);
  let searchRef = $state<HTMLInputElement | null>(null);
  let selectedProjectId = $state("");
  let sessionOptions = $state<SessionOption[]>([]);
  let search = $state("");
  let loading = $state(false);
  let sessionRequestVersion = 0;
  let renaming = $state(false);
  let renameValue = $state("");

  $effect(() => {
    sessionRequestVersion += 1;
    loading = false;
    sessionOptions = [];
    selectedProjectId = session.project_id ?? "";
    menu = null;
    search = "";
    renaming = false;
  });

  const currentProject = $derived(
    projectState.projects.find((project) => project.id === selectedProjectId) ??
      projectState.projects.find(
        (project) => project.id === session.project_id,
      ),
  );
  const projectLabel = $derived(
    currentProject?.name ??
      session.project_path.split(/[\\/]/).filter(Boolean).at(-1) ??
      "Project",
  );

  const chain = $derived(buildSessionBreadcrumb(session, getSession));

  const filteredSessions = $derived(
    sessionOptions.filter((option) =>
      option.title.toLowerCase().includes(search.trim().toLowerCase()),
    ),
  );

  function closeMenus(event: MouseEvent) {
    if (menuRef && !menuRef.contains(event.target as Node)) menu = null;
  }

  function handleWindowKeydown(event: KeyboardEvent) {
    if (event.key === "Escape" && menu && !renaming) menu = null;
  }

  async function openSessions(projectId = selectedProjectId) {
    const requestVersion = ++sessionRequestVersion;
    selectedProjectId = projectId;
    sessionOptions = [];
    menu = "session";
    search = "";
    loading = true;
    setTimeout(() => searchRef?.focus(), 0);
    try {
      const result = await api.listSessions(projectId, "all", undefined, 50);
      if (requestVersion !== sessionRequestVersion) return;
      sessionOptions = result.sessions.map((item) => ({
        id: item.id,
        title: item.title ?? "Untitled",
        updated_at: item.updated_at ?? item.created_at,
      }));
    } catch (error) {
      if (requestVersion !== sessionRequestVersion) return;
      showNotification(
        `Failed to load sessions: ${api.errorMessage(error)}`,
        "error",
      );
    } finally {
      if (requestVersion === sessionRequestVersion) loading = false;
    }
  }

  async function selectSession(id: string) {
    menu = null;
    await activateSession(id);
  }

  async function saveRename() {
    const value = renameValue.trim();
    if (!value || value === (session.alias ?? session.id.slice(-8))) {
      renaming = false;
      return;
    }
    try {
      await api.renameSession(session.id, value);
      session.alias = value;
      sessionOptions = sessionOptions.map((item) =>
        item.id === session.id ? { ...item, title: value } : item,
      );
    } catch (error) {
      showNotification(
        `Failed to rename session: ${api.errorMessage(error)}`,
        "error",
      );
    } finally {
      renaming = false;
    }
  }
</script>

<svelte:window onclick={closeMenus} onkeydown={handleWindowKeydown} />

<div
  bind:this={menuRef}
  class="relative flex min-w-0 items-center gap-0 text-sm"
>
  <button
    type="button"
    onclick={(event) => {
      event.stopPropagation();
      menu = menu === "project" ? null : "project";
    }}
    class="flex min-w-0 max-w-44 items-center gap-1 rounded-sm px-1 py-0.5 font-medium text-foreground transition-colors hover:bg-secondary/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    aria-expanded={menu === "project"}
    title="Switch project"
  >
    <Folder class="size-3.5 shrink-0" />
    <span class="truncate">{projectLabel}</span>
    <ChevronDown class="size-3 shrink-0 opacity-60" />
  </button>

  {#each chain as item (item.id)}
    <span class="px-0.5 text-xs text-foreground" aria-hidden="true">/</span>
    {#if item.id === session.id}
      <button
        type="button"
        onclick={(event) => {
          event.stopPropagation();
          void openSessions(session.project_id ?? selectedProjectId);
        }}
        class="flex min-w-0 max-w-64 items-center gap-1 rounded-sm px-1 py-0.5 font-normal text-foreground transition-colors hover:bg-secondary/60 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        aria-expanded={menu === "session"}
        title="Switch session"
      >
        <span class="truncate">{item.label}</span>
        <ChevronDown class="size-3 shrink-0 text-foreground" />
      </button>
    {:else}
      <button
        type="button"
        onclick={() => selectSession(item.id)}
        class="max-w-40 truncate rounded-sm px-1 py-0.5 text-foreground transition-colors hover:bg-secondary/60 hover:text-foreground"
        title={`Open ${item.label}`}
      >
        {item.label}
      </button>
    {/if}
  {/each}

  {#if menu === "project"}
    <div
      class="absolute left-0 top-full z-50 mt-1.5 w-64 overflow-hidden rounded-md border border-border bg-popover py-1 shadow-xl"
    >
      <div
        class="px-2 py-1 text-[10px] font-medium uppercase tracking-wide text-foreground"
      >
        Projects
      </div>
      {#each projectState.projects as project (project.id)}
        <button
          type="button"
          onclick={(event) => {
            event.stopPropagation();
            void openSessions(project.id);
          }}
          class="flex w-full items-center gap-2 px-2.5 py-1.5 text-left text-xs transition-colors hover:bg-secondary/60"
        >
          <Folder class="size-3.5 shrink-0 text-foreground" />
          <span class="min-w-0 flex-1 truncate">{project.name}</span>
          {#if project.id === session.project_id}<Check
              class="size-3.5 text-primary"
            />{/if}
          <ChevronRight class="size-3 text-foreground" />
        </button>
      {/each}
    </div>
  {:else if menu === "session"}
    <div
      class="absolute left-0 top-full z-50 mt-1.5 w-80 overflow-hidden rounded-md border border-border bg-popover shadow-xl"
    >
      <div class="border-b border-border p-2">
        <div class="relative">
          <Search
            class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-foreground"
          />
          <input
            bind:this={searchRef}
            bind:value={search}
            placeholder="Search sessions..."
            class="h-8 w-full rounded-md border border-input bg-background pl-8 pr-2 text-xs outline-none focus:ring-1 focus:ring-ring"
          />
        </div>
      </div>
      <div class="max-h-72 overflow-y-auto py-1">
        {#if loading}
          <div
            class="flex items-center justify-center gap-2 py-6 text-xs text-foreground"
          >
            <Loader2 class="size-3.5 animate-spin" />Loading sessions
          </div>
        {:else if filteredSessions.length === 0}
          <div class="py-6 text-center text-xs text-foreground">
            No sessions found
          </div>
        {:else}
          {#each filteredSessions as option (option.id)}
            <button
              type="button"
              onclick={() => selectSession(option.id)}
              class="flex w-full items-center gap-2 px-2.5 py-2 text-left transition-colors hover:bg-secondary/60"
            >
              <span
                class="min-w-0 flex-1 truncate text-xs {option.id === session.id
                  ? 'font-medium text-foreground'
                  : 'text-foreground'}">{option.title}</span
              >
              <span class="shrink-0 text-[10px] text-foreground"
                >{formatTimeAgo(option.updated_at)}</span
              >
              {#if option.id === session.id}<Check
                  class="size-3.5 shrink-0 text-primary"
                />{/if}
            </button>
          {/each}
        {/if}
      </div>
      {#if selectedProjectId === session.project_id}
        <div class="border-t border-border p-1">
          {#if renaming}
            <div class="flex gap-1 p-1">
              <input
                bind:value={renameValue}
                use:focusAndSelect
                onkeydown={(event: KeyboardEvent) => {
                  if (event.key === "Enter") void saveRename();
                  if (event.key === "Escape") {
                    event.stopPropagation();
                    renaming = false;
                  }
                }}
                class="h-8 min-w-0 flex-1 rounded-md border border-input bg-background px-2 text-xs outline-none focus:ring-1 focus:ring-ring"
              />
              <button
                type="button"
                onclick={saveRename}
                class="h-8 rounded-md border border-primary/30 bg-primary/10 px-2.5 text-xs font-medium text-primary"
                >Save</button
              >
            </div>
          {:else}
            <button
              type="button"
              onclick={() => {
                renaming = true;
                renameValue = session.alias ?? session.id.slice(-8);
              }}
              class="flex w-full items-center gap-2 rounded px-2 py-1.5 text-xs text-foreground transition-colors hover:bg-secondary/60 hover:text-foreground"
              ><Pencil class="size-3.5" />Rename current session</button
            >
          {/if}
        </div>
      {/if}
    </div>
  {/if}
</div>
