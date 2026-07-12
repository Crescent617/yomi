<script context="module" lang="ts">
  const loadedProjects = new Set<string>();
  const projectLoadPromises = new Map<string, Promise<boolean>>();
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import {
    Plus,
    MessageSquarePlus,
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
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";
  import StatusDot from "./StatusDot.svelte";
  import {
    sessionState,
    projectState,
    sessionCursors,
    pinnedSessionMeta,
    getSession,
    showNotification,
  } from "../../state.svelte";
  import {
    setActiveSession,
    loadPinnedSessions,
    refreshCheckpoints,
    createSessionState,
    activateSession as stateActivateSession,
  } from "../../session";
  import { formatTimeAgo } from "../../utils";
  import { clock } from "../../clock.svelte";
  import { slide } from "svelte/transition";
  let { collapsed = false }: { collapsed?: boolean } = $props();

  // Playful label for the new-chat button, rolled once per mount.
  // Tooltip stays explicit so clarity never depends on the joke landing.
  const NEW_CHAT_LABELS = [
    "New Chat",
    "Let's Build",
    "New Quest",
    "Fresh Start",
    "Start Cooking",
    "Once More",
    "Blank Canvas",
  ];
  const newChatLabel =
    NEW_CHAT_LABELS[Math.floor(Math.random() * NEW_CHAT_LABELS.length)];

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
  let deletingProject = $state<{ id: string; name: string } | null>(null);
  let deletingSession = $state<{ id: string; title: string } | null>(null);

  function focusAndSelect(node: HTMLInputElement) {
    node.focus();
    node.select();
  }

  onMount(() => {
    if (projectState.projects.length === 0) {
      api
        .listProjects()
        .then((list) => {
          projectState.projects = list.map((p) => ({ ...p }));
          autoExpandRecent();
          loadPinnedSessions();
        })
        .catch(console.error);
    } else if (projectState.projects.length > 0) {
      // Projects already loaded (e.g. HMR)
      autoExpandRecent();
      loadPinnedSessions();
    }
  });

  /** Auto-expand the 3 most recently active projects after loading their sessions. */
  async function autoExpandRecent() {
    const recentIds = [...projectState.projects]
      .sort((a, b) => (b.updated_at ?? "").localeCompare(a.updated_at ?? ""))
      .slice(0, 3)
      .map((p) => p.id);
    if (recentIds.length > 0) {
      const loaded = await Promise.all(recentIds.map((id) => loadSessions(id)));
      expanded = Object.fromEntries(
        recentIds.filter((_, index) => loaded[index]).map((id) => [id, true]),
      );
    }
  }

  function getSessions(project_id: string) {
    return sessionState.sessions
      .filter((s) => s.project_id === project_id)
      .sort((a, b) => (b.updated_at ?? "").localeCompare(a.updated_at ?? ""));
  }

  /** Projects sorted by most recently updated. */
  const sortedProjects = $derived(
    [...projectState.projects].sort((a, b) =>
      (b.updated_at ?? "").localeCompare(a.updated_at ?? ""),
    ),
  );

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
    if (expanded[project_id]) {
      expanded[project_id] = false;
      return;
    }
    if (loading[project_id]) return;
    const loaded = await loadSessions(project_id);
    if (loaded) expanded[project_id] = true;
  }

  async function loadSessions(
    project_id: string,
    load_more = false,
  ): Promise<boolean> {
    if (!load_more && loadedProjects.has(project_id)) return true;

    const existing = projectLoadPromises.get(project_id);
    if (existing) {
      loading[project_id] = true;
      try {
        return await existing;
      } finally {
        loading[project_id] = false;
      }
    }

    const request = (async () => {
      try {
        if (!load_more) delete sessionCursors[project_id];
        const cursor = load_more ? sessionCursors[project_id] : undefined;
        const result = await api.listSessions(project_id, cursor, 5);
        for (const s of result.sessions) {
          const existing = sessionState.sessions.find(
            (sess) => sess.id === s.id,
          );
          if (!existing) {
            sessionState.sessions.push(
              createSessionState({
                id: s.id,
                project_path: s.project_path ?? "",
                project_id: s.project_id,
                alias: s.title ?? "Untitled",
                updated_at: s.updated_at ?? s.created_at,
                permission_level: s.auto_approve_level ?? "caution",
                model_key: s.model_key,
              }),
            );
          } else {
            existing.alias = s.title ?? existing.alias ?? "Untitled";
            existing.permission_level =
              s.auto_approve_level ?? existing.permission_level;
            existing.updated_at =
              s.updated_at ?? s.created_at ?? existing.updated_at;
            existing.model_key = s.model_key ?? existing.model_key;
          }
        }
        loadedProjects.add(project_id);
        if (result.next_cursor) {
          sessionCursors[project_id] = result.next_cursor;
        } else {
          delete sessionCursors[project_id];
        }
        return true;
      } catch (e: unknown) {
        if (!load_more) delete sessionCursors[project_id];
        console.error(
          "Failed to load sessions:",
          e instanceof Error ? e.message : e,
        );
        return false;
      } finally {
        projectLoadPromises.delete(project_id);
      }
    })();

    projectLoadPromises.set(project_id, request);
    loading[project_id] = true;
    try {
      return await request;
    } finally {
      loading[project_id] = false;
    }
  }

  async function activateSession(id: string) {
    const prev = sessionState.activeSessionId;
    try {
      await stateActivateSession(id);
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

  function requestDeleteSession(id: string) {
    const session = getSession(id);
    deletingSession = { id, title: session?.alias ?? id.slice(-8) };
  }

  async function confirmDeleteSession() {
    if (!deletingSession) return;
    const { id } = deletingSession;
    deletingSession = null;
    try {
      await api.unsubscribe(id);
      await api.deleteSession(id);
      sessionState.sessions = sessionState.sessions.filter((s) => s.id !== id);
      delete pinnedSessionMeta[id];
      loadPinnedSessions();
      if (sessionState.activeSessionId === id) setActiveSession(null);
      showNotification("Session deleted", "success");
    } catch (e: unknown) {
      console.error(
        "Failed to delete session:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to delete session", "error");
    }
  }

  function requestDeleteProject(id: string) {
    showMenu = null;
    const project = projectState.projects.find((p) => p.id === id);
    deletingProject = { id, name: project?.name ?? id.slice(0, 8) };
  }

  async function confirmDeleteProject() {
    if (!deletingProject) return;
    const { id } = deletingProject;
    deletingProject = null;
    try {
      // Unsubscribe from event streams of this project's sessions first
      const projectSessions = sessionState.sessions.filter(
        (s) => s.project_id === id,
      );
      for (const s of projectSessions) {
        try {
          await api.unsubscribe(s.id);
        } catch {
          // best-effort; the session may not be subscribed
        }
      }

      const result = await api.deleteProject(id);

      // Prune local state: project, its sessions, pinned meta, cursors
      const removedIds = new Set(projectSessions.map((s) => s.id));
      projectState.projects = projectState.projects.filter((p) => p.id !== id);
      sessionState.sessions = sessionState.sessions.filter(
        (s) => s.project_id !== id,
      );
      for (const sid of Object.keys(pinnedSessionMeta)) {
        if (removedIds.has(sid)) delete pinnedSessionMeta[sid];
      }
      delete sessionCursors[id];
      loadedProjects.delete(id);
      loadPinnedSessions();
      if (
        sessionState.activeSessionId &&
        removedIds.has(sessionState.activeSessionId)
      ) {
        setActiveSession(null);
      }

      showNotification(
        result.sessions_deleted > 0
          ? `Project deleted (${result.sessions_deleted} sessions removed)`
          : "Project deleted",
        "success",
      );

      // Re-fetch from backend as the source of truth (the optimistic prune
      // above only covers sessions already loaded in the frontend).
      api
        .listProjects()
        .then((list) => {
          projectState.projects = list.map((p) => ({ ...p }));
        })
        .catch(console.error);
    } catch (e: unknown) {
      console.error(
        "Failed to delete project:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to delete project", "error");
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
        undefined,
      );
      sessionState.sessions.push(
        createSessionState({
          id,
          project_path: project.dir,
          project_id,
          alias: "Untitled",
          permission_level: config?.auto_approve ?? "caution",
        }),
      );
      await activateSession(id);
    } catch (e: unknown) {
      console.error(
        "Failed to create session:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to create session", "error");
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
      showNotification("Project renamed", "success");
    } catch (e: unknown) {
      console.error(
        "Failed to rename project:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to rename project", "error");
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
      showNotification("Session renamed", "success");
    } catch (e: unknown) {
      console.error(
        "Failed to rename session:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to rename session", "error");
    }
    renamingSessionId = null;
  }

  async function copySessionId(id: string) {
    try {
      await navigator.clipboard.writeText(id);
      showNotification("Session ID copied", "success");
    } catch {
      showNotification("Failed to copy", "error");
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
        showNotification("Failed to unpin session", "error");
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
        showNotification("Failed to pin session", "error");
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
        class="flex h-9 w-full items-center justify-center gap-2 rounded-md border border-border bg-secondary px-3 text-sm font-medium text-foreground transition-all hover:bg-secondary/80 active:scale-[0.99] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        onclick={() => setActiveSession(null)}
        aria-label={`New session — ${newChatLabel}`}
        title="New session"
      >
        <MessageSquarePlus size={16} />
        {newChatLabel}
      </button>
    </div>
  {:else}
    <div class="shrink-0 py-2">
      <button
        class="flex h-9 w-9 items-center justify-center rounded-md border border-border bg-secondary text-foreground transition-colors hover:bg-secondary/80 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        onclick={() => setActiveSession(null)}
        aria-label="New session"
        title="New session"
      >
        <MessageSquarePlus size={16} />
      </button>
    </div>
  {/if}

  {#if !collapsed && pinnedList.length > 0}
    <div
      class="shrink-0 max-h-[33%] overflow-y-auto px-2 py-1"
      onscroll={closeMenus}
    >
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
              class="group w-full flex items-center gap-2 rounded-sm border-l-2 px-3 py-1 cursor-pointer transition-colors {active
                ? 'border-primary bg-primary/8 text-foreground'
                : 'border-transparent hover:border-border hover:bg-secondary/40 text-muted-foreground hover:text-foreground'}"
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
                class="shrink-0 text-[10px] text-muted-foreground truncate max-w-[6rem]"
                >{projectName(session?.project_id)}</span
              >
              <div class="flex items-center gap-1.5 shrink-0">
                {#if session}
                  <StatusDot phase={session.phase} />
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
                          requestDeleteSession(session_id);
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
    onscroll={closeMenus}
  >
    {#if collapsed}
      {#each sortedProjects as project (project.id)}
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
              class="relative w-8 h-8 rounded-lg flex items-center justify-center text-[10px] font-bold transition-colors {session.id ===
              sessionState.activeSessionId
                ? 'bg-primary text-primary-foreground'
                : 'bg-secondary text-secondary-foreground hover:bg-secondary/80'}"
              onclick={() => activateSession(session.id)}
              title={session.alias ?? "Untitled"}
            >
              {(session.alias ?? "Untitled").slice(0, 2).toUpperCase()}
              <span class="absolute -top-0.5 -right-0.5">
                <StatusDot phase={session.phase} />
              </span>
            </button>
          {/each}
        </div>
      {/each}
    {:else}
      {#each sortedProjects as project (project.id)}
        {@const isActive =
          getSession(sessionState.activeSessionId ?? "")?.project_id ===
          project.id}
        <div class="border-b border-border/40 pb-0.5 mb-0.5">
          <div
            class="flex items-center gap-1.5 w-full px-2 py-1.5 text-xs transition-colors select-none {isActive
              ? 'text-foreground bg-secondary/35'
              : 'text-muted-foreground hover:text-foreground hover:bg-secondary/25'}"
          >
            <button
              class="flex items-center gap-1.5 flex-1 min-w-0 text-left"
              onclick={() => toggle(project.id)}
              aria-expanded={Boolean(expanded[project.id])}
              aria-controls={`project-sessions-${project.id}`}
            >
              {#if expanded[project.id]}
                <FolderOpen
                  size={13}
                  class="shrink-0 {isActive
                    ? 'text-primary'
                    : 'text-muted-foreground'}"
                />
              {:else}
                <Folder
                  size={13}
                  class="shrink-0 {isActive
                    ? 'text-primary'
                    : 'text-muted-foreground'}"
                />
              {/if}
              {#if renamingProjectId === project.id}
                <input
                  type="text"
                  use:focusAndSelect
                  bind:value={renameValue}
                  onkeydown={(e: KeyboardEvent) => {
                    if (e.key === "Enter") confirmRenameProject(project.id);
                    if (e.key === "Escape") renamingProjectId = null;
                  }}
                  onblur={() => confirmRenameProject(project.id)}
                  class="flex-1 min-w-0 bg-background border border-border rounded px-1 py-0.5 text-xs focus:outline-none focus:ring-1 focus:ring-ring"
                />
              {:else}
                <span class="truncate font-medium">{project.name}</span>
              {/if}
              {#if getSessions(project.id).some((s) => s.phase !== "idle" && s.phase !== "closed")}
                {@const projectRunning = getSessions(project.id).some(
                  (s) =>
                    s.phase === "streaming" ||
                    s.phase === "executing_tool" ||
                    s.phase === "compacting",
                )}
                <!-- Aggregate: attention (waiting) wins over running -->
                <StatusDot
                  phase={getSessions(project.id).some(
                    (s) =>
                      s.phase !== "idle" &&
                      s.phase !== "closed" &&
                      s.phase !== "streaming" &&
                      s.phase !== "executing_tool" &&
                      s.phase !== "compacting",
                  )
                    ? "waiting"
                    : projectRunning
                      ? "streaming"
                      : "idle"}
                />
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
                      requestDeleteProject(project.id);
                    }}
                  >
                    <Trash2 size={12} /> Delete
                  </button>
                </div>
                <button
                  type="button"
                  aria-label="Close project menu"
                  class="fixed inset-0 z-10"
                  onclick={() => (showMenu = null)}
                ></button>
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
            <div
              id={`project-sessions-${project.id}`}
              class="ml-2 space-y-0.5 pb-1"
              aria-busy={Boolean(loading[project.id])}
              transition:slide={{ duration: 200 }}
            >
              {#each getSessions(project.id) as session (session.id)}
                <div
                  class="group relative w-full flex items-center gap-2 rounded-sm px-2 py-1 cursor-pointer border-l-2 transition-colors {session.id ===
                  sessionState.activeSessionId
                    ? 'border-primary bg-primary/8 text-foreground'
                    : 'border-transparent hover:border-border hover:bg-secondary/40 text-muted-foreground hover:text-foreground'}"
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
                      use:focusAndSelect
                      bind:value={renameValue}
                      onkeydown={(e: KeyboardEvent) => {
                        if (e.key === "Enter") confirmRenameSession(session.id);
                        if (e.key === "Escape") renamingSessionId = null;
                      }}
                      onblur={() => confirmRenameSession(session.id)}
                      class="flex-1 min-w-0 bg-background border border-border rounded px-1 py-0.5 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
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
                    <StatusDot phase={session.phase} />
                    {#if session.updated_at}
                      {@const menuOpen = projectMenu?.session_id === session.id}
                      <!-- Fixed-width slot: time fades out, ⋮ fades in on top — no layout shift -->
                      <div
                        class="relative flex items-center justify-end min-w-[3.25rem] h-5"
                      >
                        <span
                          class="text-[10px] text-muted-foreground transition-opacity {menuOpen
                            ? 'opacity-0'
                            : 'group-hover:opacity-0'}"
                          title={new Date(session.updated_at).toLocaleString()}
                        >
                          {formatTimeAgo(session.updated_at, clock.now)}
                        </span>
                        <button
                          class="absolute right-0 p-0.5 rounded hover:bg-secondary/80 transition-opacity {menuOpen
                            ? 'opacity-100'
                            : 'opacity-0 group-hover:opacity-100'}"
                          onclick={(e: MouseEvent) =>
                            openMenu(e, "project", session.id)}
                        >
                          <MoreVertical size={12} />
                        </button>
                      </div>
                    {:else}
                      <button
                        class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-opacity {projectMenu?.session_id ===
                        session.id
                          ? 'opacity-100'
                          : 'opacity-0 group-hover:opacity-100'}"
                        onclick={(e: MouseEvent) =>
                          openMenu(e, "project", session.id)}
                      >
                        <MoreVertical size={12} />
                      </button>
                    {/if}
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
                            requestDeleteSession(session.id);
                            projectMenu = null;
                          }}
                        >
                          <Trash2 size={12} /> Delete
                        </button>
                      </div>
                    {/if}
                  </div>
                </div>
              {/each}
              {#if loading[project.id]}
                <div class="px-3 py-1.5 text-xs text-muted-foreground">
                  Loading...
                </div>
              {:else if getSessions(project.id).length === 0}
                <button
                  class="w-full text-left px-3 py-1.5 text-xs italic text-muted-foreground hover:text-foreground transition-colors"
                  onclick={() => quickCreateSession(project.id)}
                  title="Create a session in this project"
                >
                  No sessions — click to create
                </button>
              {/if}
              {#if project.id in sessionCursors}
                <button
                  class="w-full text-left px-3 py-1.5 text-xs italic text-muted-foreground hover:text-foreground transition-colors"
                  onclick={() => loadSessions(project.id, true)}
                  disabled={Boolean(loading[project.id])}
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

<ConfirmDialog
  open={deletingProject !== null}
  title="Delete project"
  message={`Delete project "${deletingProject?.name ?? ""}" and ALL its sessions?\n\nThis permanently removes every session in this project (including subagent sessions), their message history, todos, checkpoints and related data. Files in the project directory itself are not touched.\n\nThis cannot be undone.`}
  confirmText="Delete project"
  onConfirm={confirmDeleteProject}
  onCancel={() => (deletingProject = null)}
/>

<ConfirmDialog
  open={deletingSession !== null}
  title="Delete session"
  message={`Delete session "${deletingSession?.title ?? ""}"?\n\nThis permanently removes its message history, todos, checkpoints and related data.\n\nThis cannot be undone.`}
  confirmText="Delete session"
  onConfirm={confirmDeleteSession}
  onCancel={() => (deletingSession = null)}
/>
