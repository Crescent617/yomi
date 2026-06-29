<script lang="ts">
  import { onMount } from "svelte";
  import {
    Plus,
    Folder,
    FolderOpen,
    MoreVertical,
    Pencil,
    Trash2,
    Copy,
    Pin,
    PinOff,
  } from "lucide-svelte";
  import * as api from "../../api";
  import {
    sessionState,
    projectState,
    sessionCursors,
    pinnedSessionMeta,
    setActiveSession,
    loadSessionMessages,
    loadPinnedSessions,
    getSession,
    showNotification,
    syncSessionStatus,
    refreshCheckpoints,
  } from "../../state.svelte";

  let { collapsed = false }: { collapsed?: boolean } = $props();

  let expanded = $state<Record<string, boolean>>({});
  let loading = $state<Record<string, boolean>>({});
  let showMenu = $state<string | null>(null);
  let pinnedMenu = $state<{ session_id: string; x: number; y: number } | null>(
    null,
  );
  let projectMenu = $state<{ session_id: string; x: number; y: number } | null>(
    null,
  );
  let renamingProjectId = $state<string | null>(null);
  let renamingSessionId = $state<string | null>(null);
  let renameValue = $state("");

  onMount(() => {
    if (projectState.projects.length === 0) {
      api
        .listProjects()
        .then((list) => {
          projectState.projects = list.map((p) => ({ ...p }));
          // Auto-expand first 3 projects and load their sessions
          const firstN = list.slice(0, 3).map((p) => p.id);
          if (firstN.length > 0) {
            expanded = Object.fromEntries(firstN.map((id) => [id, true]));
            for (const id of firstN) {
              loadSessions(id);
            }
          }
          loadPinnedSessions();
        })
        .catch(console.error);
    } else if (projectState.projects.length > 0) {
      // Projects already loaded (e.g. HMR), expand first 3
      const firstN = projectState.projects.slice(0, 3).map((p) => p.id);
      expanded = Object.fromEntries(firstN.map((id) => [id, true]));
      for (const id of firstN) {
        loadSessions(id);
      }
      loadPinnedSessions();
    }
  });

  function getSessions(project_id: string) {
    return sessionState.sessions
      .filter((s) => s.project_id === project_id)
      .sort((a, b) => (b.updated_at ?? "").localeCompare(a.updated_at ?? ""));
  }

  const pinnedList = $derived(
    Object.entries(pinnedSessionMeta)
      .map(([session_id, meta]) => {
        const session = getSession(session_id);
        return { session_id, session, meta };
      })
      .sort((a, b) =>
        (b.meta.pinned_at ?? "").localeCompare(a.meta.pinned_at ?? ""),
      ),
  );

  function projectName(project_id?: string | null): string {
    if (!project_id) return "Default";
    return (
      projectState.projects.find((p) => p.id === project_id)?.name ??
      project_id.slice(0, 8)
    );
  }

  async function toggle(project_id: string) {
    expanded[project_id] = !expanded[project_id];
    if (expanded[project_id]) {
      await loadSessions(project_id);
    }
  }

  async function loadSessions(project_id: string) {
    if (loading[project_id]) return;

    loading[project_id] = true;
    try {
      const cursor = sessionCursors[project_id];
      const result = await api.listSessions(project_id, cursor, 5);
      for (const s of result.sessions) {
        const existing = sessionState.sessions.find((sess) => sess.id === s.id);
        if (!existing) {
          sessionState.sessions.push({
            id: s.id,
            project_path: s.project_path ?? "",
            project_id: s.project_id,
            alias: s.title ?? "Untitled",
            messages: [],
            phase: "idle",
            unread: 0,
            checkpoints: [],
            tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
            active_tab_id: "chat",
            pending_permissions: [],
            pending_ask_user: null,
            queued_input: null,
            updated_at: s.updated_at ?? s.created_at,
            permission_level: s.auto_approve_level ?? "caution",
          });
        } else {
          existing.alias = s.title ?? existing.alias ?? "Untitled";
          existing.permission_level =
            s.auto_approve_level ?? existing.permission_level;
          existing.updated_at =
            s.updated_at ?? s.created_at ?? existing.updated_at;
        }
      }
      if (result.next_cursor) {
        sessionCursors[project_id] = result.next_cursor;
      } else {
        delete sessionCursors[project_id];
      }
    } catch (e: unknown) {
      console.error(
        "Failed to load sessions:",
        e instanceof Error ? e.message : e,
      );
    } finally {
      loading[project_id] = false;
    }
  }

  async function activateSession(id: string) {
    const prev = sessionState.activeSessionId;
    try {
      await api.subscribe(id);
      setActiveSession(id);
      // Sync initial runtime status from backend (streaming / compacting)
      const status = await api.getSessionStatus(id);
      const session = getSession(id);
      if (session) {
        syncSessionStatus(id, status);
      }
      const msgs = await api.getMessages(id);
      if (getSession(id)) loadSessionMessages(id, msgs);
      refreshCheckpoints(id);
    } catch (e: unknown) {
      console.error(
        "Failed to activate session:",
        e instanceof Error ? e.message : e,
      );
      if (prev && prev !== id) {
        setActiveSession(prev);
      } else {
        setActiveSession(null);
      }
    }
  }

  async function deleteSession(id: string) {
    if (!confirm("Delete this session?")) return;
    try {
      await api.unsubscribe(id);
      await api.deleteSession(id);
      sessionState.sessions = sessionState.sessions.filter((s) => s.id !== id);
      delete pinnedSessionMeta[id];
      loadPinnedSessions();
      if (sessionState.activeSessionId === id) setActiveSession(null);
      showNotification("Session deleted", "success", 2000);
    } catch (e: unknown) {
      console.error(
        "Failed to delete session:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to delete session", "error", 3000);
    }
  }

  async function deleteProject(id: string) {
    showMenu = null;
    if (getSessions(id).length > 0) {
      showNotification("Cannot delete project with sessions", "error", 3000);
      return;
    }
    try {
      await api.deleteProject(id);
      projectState.projects = projectState.projects.filter((p) => p.id !== id);
      showNotification("Project deleted", "success", 2000);
    } catch (e: unknown) {
      console.error(
        "Failed to delete project:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to delete project", "error", 3000);
    }
  }

  async function quickCreateSession(project_id: string) {
    const project = projectState.projects.find((p) => p.id === project_id);
    if (!project) return;
    try {
      const config = await api.getConfig();
      const id = await api.createSession(
        project.dir,
        config?.auto_approve ?? "caution",
        project_id,
      );
      sessionState.sessions.push({
        id,
        project_path: project.dir,
        project_id,
        alias: "Untitled",
        messages: [],
        phase: "idle",
        unread: 0,
        checkpoints: [],
        tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
        active_tab_id: "chat",
        pending_permissions: [],
        pending_ask_user: null,
        queued_input: null,
        updated_at: new Date().toISOString(),
        permission_level: config?.auto_approve ?? "caution",
      });
      await activateSession(id);
    } catch (e: unknown) {
      console.error(
        "Failed to create session:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to create session", "error", 3000);
    }
  }

  async function confirmRenameProject(project_id: string) {
    const name = renameValue.trim();
    if (!name) {
      renamingProjectId = null;
      return;
    }
    try {
      await api.renameProject(project_id, name);
      const p = projectState.projects.find((x) => x.id === project_id);
      if (p) p.name = name;
      showNotification("Project renamed", "success", 2000);
    } catch (e: unknown) {
      console.error(
        "Failed to rename project:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to rename project", "error", 3000);
    }
    renamingProjectId = null;
  }

  async function confirmRenameSession(session_id: string) {
    const name = renameValue.trim();
    if (!name) {
      renamingSessionId = null;
      return;
    }
    try {
      await api.renameSession(session_id, name);
      const s = sessionState.sessions.find((x) => x.id === session_id);
      if (s) s.alias = name;
      showNotification("Session renamed", "success", 2000);
    } catch (e: unknown) {
      console.error(
        "Failed to rename session:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to rename session", "error", 3000);
    }
    renamingSessionId = null;
  }

  async function copySessionId(id: string) {
    try {
      await navigator.clipboard.writeText(id);
      showNotification("Session ID copied", "success", 1500);
    } catch {
      showNotification("Failed to copy", "error", 1500);
    }
  }

  async function togglePin(session_id: string) {
    const session = getSession(session_id);
    if (!session) return;

    if (session.is_pinned) {
      session.is_pinned = false;
      delete pinnedSessionMeta[session_id];
      try {
        await api.unpinSession(session_id);
      } catch (e: unknown) {
        session.is_pinned = true;
        pinnedSessionMeta[session_id] = {
          pinned_at: new Date().toISOString(),
        };
        console.error(
          "Failed to unpin session:",
          e instanceof Error ? e.message : e,
        );
        showNotification("Failed to unpin session", "error", 3000);
      }
    } else {
      const now = new Date().toISOString();
      session.is_pinned = true;
      pinnedSessionMeta[session_id] = { pinned_at: now };
      try {
        await api.pinSession(session_id);
      } catch (e: unknown) {
        session.is_pinned = false;
        delete pinnedSessionMeta[session_id];
        console.error(
          "Failed to pin session:",
          e instanceof Error ? e.message : e,
        );
        showNotification("Failed to pin session", "error", 3000);
      }
    }
  }

  function openMenu(
    e: MouseEvent,
    target: "pinned" | "project",
    session_id: string,
  ) {
    e.stopPropagation();
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    const menu = { session_id, x: rect.right, y: rect.bottom + 4 };
    if (target === "pinned") {
      pinnedMenu = menu;
      projectMenu = null;
    } else {
      projectMenu = menu;
      pinnedMenu = null;
    }
  }

  function closeMenus() {
    pinnedMenu = null;
    projectMenu = null;
  }
</script>

<div class="flex flex-col h-full {collapsed ? 'items-center' : ''}">
  {#if pinnedMenu || projectMenu}
    <div
      class="fixed inset-0 z-40"
      onclick={closeMenus}
      aria-hidden="true"
    ></div>
  {/if}
  {#if !collapsed}
    <div class="shrink-0 p-2">
      <button
        class="w-full flex items-center justify-center gap-1.5 rounded-lg border border-border bg-secondary px-3 py-2 text-sm font-medium text-secondary-foreground hover:bg-secondary/80 active:scale-[0.98] transition-all"
        onclick={() => setActiveSession(null)}
      >
        <Plus size={16} />
        New Session
      </button>
    </div>
  {:else}
    <div class="shrink-0 py-2">
      <button
        class="w-8 h-8 rounded-lg flex items-center justify-center text-muted-foreground hover:bg-secondary hover:text-foreground transition-colors"
        onclick={() => setActiveSession(null)}
      >
        <Plus size={16} />
      </button>
    </div>
  {/if}

  {#if !collapsed && pinnedList.length > 0}
    <div class="shrink-0 max-h-[33%] overflow-y-auto px-2 py-1">
      <div
        class="flex items-center gap-1.5 px-2 py-1.5 text-xs font-semibold text-muted-foreground uppercase tracking-wider"
      >
        <Pin size={12} />
        Pinned
      </div>
      <div class="space-y-0">
        {#each pinnedList as { session_id, session, meta: _meta } (session_id)}
          {@const active = sessionState.activeSessionId === session_id}
          <div class="relative">
            <div
              class="group w-full flex items-center gap-2 rounded-lg px-3 py-1 cursor-pointer transition-colors {active
                ? 'bg-primary/10 text-foreground'
                : 'hover:bg-secondary/50 text-muted-foreground hover:text-foreground'}"
              onclick={() => activateSession(session_id)}
              role="button"
              tabindex="0"
              onkeydown={(e: KeyboardEvent) => {
                if (e.key === "Enter" || e.key === " ")
                  activateSession(session_id);
              }}
            >
              <span
                class="flex-1 truncate text-sm font-medium"
                title={session?.alias ?? "Untitled"}
              >
                {session?.alias ?? "Untitled"}
              </span>
              <span
                class="shrink-0 text-[10px] text-muted-foreground/70 truncate max-w-[6rem]"
                >{projectName(session?.project_id)}</span
              >
              <div class="flex items-center gap-1.5 shrink-0">
                {#if session && session.phase !== "idle" && session.phase !== "closed"}
                  <span
                    class="w-1.5 h-1.5 rounded-full {session.phase ===
                      'streaming' ||
                    session.phase === 'executing_tool' ||
                    session.phase === 'compacting'
                      ? 'bg-primary'
                      : 'bg-amber-500'} animate-pulse"
                  ></span>
                {/if}
                <div class="relative">
                  <button
                    class="shrink-0 p-0.5 rounded opacity-0 group-hover:opacity-100 hover:bg-secondary/80 transition-all"
                    onclick={(e: MouseEvent) =>
                      openMenu(e, "pinned", session_id)}
                  >
                    <MoreVertical size={12} />
                  </button>
                  {#if pinnedMenu?.session_id === session_id}
                    <div
                      class="fixed z-50 w-36 rounded-md border border-border bg-popover shadow-md py-1"
                      style="top: {pinnedMenu.y}px; left: {pinnedMenu.x}px; transform: translateX(-100%);"
                    >
                      <button
                        class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                        onclick={(e: Event) => {
                          e.stopPropagation();
                          togglePin(session_id);
                          pinnedMenu = null;
                        }}
                      >
                        <PinOff size={12} /> Unpin
                      </button>
                      <button
                        class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                        onclick={(e: Event) => {
                          e.stopPropagation();
                          copySessionId(session_id);
                          pinnedMenu = null;
                        }}
                      >
                        <Copy size={12} /> Copy ID
                      </button>
                      <button
                        class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/10 text-left"
                        onclick={(e: Event) => {
                          e.stopPropagation();
                          deleteSession(session_id);
                          pinnedMenu = null;
                        }}
                      >
                        <Trash2 size={12} /> Delete
                      </button>
                    </div>
                  {/if}
                </div>
              </div>
            </div>
          </div>
        {/each}
      </div>
    </div>
    <div class="h-px bg-border mx-2 mb-1 shrink-0"></div>
  {/if}
  <div
    class="flex-1 min-h-0 overflow-y-auto py-1 {collapsed ? 'px-1' : 'px-2'}"
  >
    {#if collapsed}
      {#each projectState.projects as project (project.id)}
        <div class="flex flex-col items-center gap-1">
          <!-- Project divider -->
          <div
            class="w-8 h-8 rounded-lg flex items-center justify-center text-[10px] font-bold bg-transparent text-muted-foreground mt-1 mb-0.5"
            title={project.name}
          >
            {project.name.slice(0, 2).toUpperCase()}
          </div>
          {#each getSessions(project.id) as session (session.id)}
            <button
              class="w-8 h-8 rounded-lg flex items-center justify-center text-[10px] font-bold transition-colors {session.id ===
              sessionState.activeSessionId
                ? 'bg-primary text-primary-foreground'
                : 'bg-secondary text-secondary-foreground hover:bg-secondary/80'}"
              onclick={() => activateSession(session.id)}
              title={session.alias ?? "Untitled"}
            >
              {(session.alias ?? "Untitled").slice(0, 2).toUpperCase()}
            </button>
          {/each}
        </div>
      {/each}
    {:else}
      {#each projectState.projects as project (project.id)}
        {@const isActive =
          getSession(sessionState.activeSessionId ?? "")?.project_id ===
          project.id}
        <div class="rounded-md mb-0.5">
          <div
            class="flex items-center gap-1.5 w-full rounded-md px-2 py-1.5 text-xs transition-colors select-none {isActive
              ? 'text-foreground bg-secondary/60'
              : 'text-muted-foreground hover:text-foreground hover:bg-secondary/40'}"
          >
            <button
              class="flex items-center gap-1.5 flex-1 min-w-0 text-left"
              onclick={() => toggle(project.id)}
            >
              {#if expanded[project.id]}
                <FolderOpen size={13} class="shrink-0 opacity-70" />
              {:else}
                <Folder size={13} class="shrink-0 opacity-70" />
              {/if}
              {#if renamingProjectId === project.id}
                <input
                  type="text"
                  bind:value={renameValue}
                  onkeydown={(e: KeyboardEvent) => {
                    if (e.key === "Enter") confirmRenameProject(project.id);
                    if (e.key === "Escape") renamingProjectId = null;
                  }}
                  onblur={() => confirmRenameProject(project.id)}
                  class="flex-1 min-w-0 bg-background border border-border rounded px-1 py-0.5 text-xs focus:outline-none focus:ring-1 focus:ring-ring"
                  autofocus
                />
              {:else}
                <span class="truncate font-medium">{project.name}</span>
              {/if}
              {#if getSessions(project.id).some((s) => s.phase !== "idle" && s.phase !== "closed")}
                <span
                  class="w-1.5 h-1.5 rounded-full {getSessions(project.id).some(
                    (s) =>
                      s.phase === 'streaming' ||
                      s.phase === 'executing_tool' ||
                      s.phase === 'compacting',
                  )
                    ? 'bg-primary'
                    : 'bg-amber-500'} animate-pulse shrink-0"
                ></span>
              {/if}
            </button>

            <div class="relative">
              <button
                class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-colors"
                onclick={(e: Event) => {
                  e.stopPropagation();
                  showMenu = showMenu === project.id ? null : project.id;
                }}
              >
                <MoreVertical size={12} />
              </button>
              {#if showMenu === project.id}
                <div
                  class="absolute right-0 top-full mt-1 z-20 w-32 rounded-md border border-border bg-popover shadow-md py-1"
                >
                  <button
                    class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                    onclick={(e: Event) => {
                      e.stopPropagation();
                      renamingProjectId = project.id;
                      renameValue = project.name;
                      showMenu = null;
                    }}
                  >
                    <Pencil size={12} /> Rename
                  </button>
                  <button
                    class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/10 text-left"
                    onclick={(e: Event) => {
                      e.stopPropagation();
                      deleteProject(project.id);
                    }}
                  >
                    <Trash2 size={12} /> Delete
                  </button>
                </div>
                <div
                  class="fixed inset-0 z-10"
                  onclick={() => (showMenu = null)}
                ></div>
              {/if}
            </div>

            <button
              class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-colors"
              onclick={() => quickCreateSession(project.id)}
            >
              <Plus size={12} />
            </button>
          </div>

          {#if expanded[project.id]}
            <div class="ml-3 pl-2 border-l border-border/40 space-y-0 pb-1">
              {#each getSessions(project.id) as session (session.id)}
                <div
                  class="group w-full flex items-center gap-2 rounded-lg pl-2 pr-3 py-1 cursor-pointer transition-colors {session.id ===
                  sessionState.activeSessionId
                    ? 'bg-primary/10 text-foreground'
                    : 'hover:bg-secondary/50 text-muted-foreground hover:text-foreground'}"
                  onclick={() => activateSession(session.id)}
                  role="button"
                  tabindex="0"
                  onkeydown={(e: KeyboardEvent) => {
                    if (e.key === "Enter" || e.key === " ")
                      activateSession(session.id);
                  }}
                >
                  {#if renamingSessionId === session.id}
                    <input
                      type="text"
                      bind:value={renameValue}
                      onkeydown={(e: KeyboardEvent) => {
                        if (e.key === "Enter") confirmRenameSession(session.id);
                        if (e.key === "Escape") renamingSessionId = null;
                      }}
                      onblur={() => confirmRenameSession(session.id)}
                      class="flex-1 min-w-0 bg-background border border-border rounded px-1 py-0.5 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
                      autofocus
                    />
                  {:else}
                    <span
                      class="flex-1 truncate text-sm font-medium"
                      title={session.alias ?? "Untitled"}
                    >
                      {session.alias ?? "Untitled"}
                    </span>
                  {/if}
                  <div class="flex items-center gap-1.5 shrink-0">
                    {#if session.phase !== "idle" && session.phase !== "closed"}
                      <span
                        class="w-1.5 h-1.5 rounded-full {session.phase ===
                          'streaming' ||
                        session.phase === 'executing_tool' ||
                        session.phase === 'compacting'
                          ? 'bg-primary'
                          : 'bg-amber-500'} animate-pulse"
                      ></span>
                    {/if}
                    <div class="relative">
                      <button
                        class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-colors opacity-0 group-hover:opacity-100"
                        onclick={(e: MouseEvent) =>
                          openMenu(e, "project", session.id)}
                      >
                        <MoreVertical size={12} />
                      </button>
                      {#if projectMenu?.session_id === session.id}
                        <div
                          class="fixed z-50 w-36 rounded-md border border-border bg-popover shadow-md py-1"
                          style="top: {projectMenu.y}px; left: {projectMenu.x}px; transform: translateX(-100%);"
                        >
                          <button
                            class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                            onclick={(e: Event) => {
                              e.stopPropagation();
                              togglePin(session.id);
                              projectMenu = null;
                            }}
                          >
                            {#if session.is_pinned}
                              <PinOff size={12} /> Unpin
                            {:else}
                              <Pin size={12} /> Pin to top
                            {/if}
                          </button>
                          <button
                            class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                            onclick={(e: Event) => {
                              e.stopPropagation();
                              renamingSessionId = session.id;
                              renameValue = session.alias ?? "Untitled";
                              projectMenu = null;
                            }}
                          >
                            <Pencil size={12} /> Rename
                          </button>
                          <button
                            class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                            onclick={(e: Event) => {
                              e.stopPropagation();
                              copySessionId(session.id);
                              projectMenu = null;
                            }}
                          >
                            <Copy size={12} /> Copy ID
                          </button>
                          <button
                            class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/10 text-left"
                            onclick={(e: Event) => {
                              e.stopPropagation();
                              deleteSession(session.id);
                              projectMenu = null;
                            }}
                          >
                            <Trash2 size={12} /> Delete
                          </button>
                        </div>
                      {/if}
                    </div>
                  </div>
                </div>
              {/each}
              {#if loading[project.id]}
                <div class="px-3 py-1.5 text-xs text-muted-foreground">
                  Loading...
                </div>
              {/if}
              {#if project.id in sessionCursors}
                <button
                  class="w-full text-left px-3 py-1.5 text-xs italic text-muted-foreground hover:text-foreground transition-colors"
                  onclick={() => loadSessions(project.id)}
                >
                  Load more...
                </button>
              {/if}
            </div>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
</div>
