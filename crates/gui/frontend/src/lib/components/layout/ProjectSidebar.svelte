<script context="module" lang="ts">
  const loadedProjects = new Set<string>();
  const projectLoadPromises = new Map<string, Promise<boolean>>();
  let allSessionsLoadedShared = false;
  let allSessionsCursorShared: string | null = null;
  let allSessionsLoadPromise: Promise<void> | null = null;
</script>

<script lang="ts">
  import { onMount } from "svelte";
  import {
    Plus,
    MessageSquarePlus,
    MessageSquare,
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
  import SessionForkMenuItem from "./SessionForkMenuItem.svelte";
  import {
    sessionState,
    unreadSessions,
    projectState,
    sessionCursors,
    pinnedSessionMeta,
    getSession,
    showNotification,
    type SessionState,
  } from "../../state.svelte";
  import {
    setActiveSession,
    loadPinnedSessions,
    refreshCheckpoints,
    createSessionState,
    forkSession as forkSessionState,
    activateSession as stateActivateSession,
  } from "../../session";
  import { formatTimeAgo } from "../../utils";
  import ProjectDot from "../ui/ProjectDot.svelte";
  import { groupSessionsByTime } from "./session-time-groups";
  import { clock } from "../../clock.svelte";
  import {
    guiPreferences,
    saveGuiPreferences,
    snapshotGuiPreferences,
    type SidebarViewPreference,
  } from "../../settings.svelte";
  import { slide, fly } from "svelte/transition";
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
  let forkingSessionId = $state<string | null>(null);
  let renameValue = $state("");
  let deletingProject = $state<{ id: string; name: string } | null>(null);
  let deletingSession = $state<{ id: string; title: string } | null>(null);
  const sidebarView = $derived(guiPreferences.layout.sidebar_view);
  let allSessionsLoaded = $state(allSessionsLoadedShared);
  let allSessionsLoading = $state(false);
  let allSessionsError = $state(false);
  let allSessionsCursor = $state<string | null>(allSessionsCursorShared);

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
      .filter(
        (session) =>
          session.project_id === project_id && !session.parent_session_id,
      )
      .sort((a, b) => (b.updated_at ?? "").localeCompare(a.updated_at ?? ""));
  }

  const allSessions = $derived(
    sessionState.sessions
      .filter((session) => !session.parent_session_id)
      .sort((a, b) => (b.updated_at ?? "").localeCompare(a.updated_at ?? "")),
  );

  const sessionTimeGroups = $derived(
    groupSessionsByTime(allSessions, clock.now),
  );

  function mergeSessionInfo(s: api.SessionInfo): SessionState {
    let session = sessionState.sessions.find((item) => item.id === s.id);
    if (!session) {
      session = createSessionState({
        id: s.id,
        project_path: s.project_path ?? "",
        project_id: s.project_id,
        alias: s.title ?? "Untitled",
        updated_at: s.updated_at ?? s.created_at,
        permission_level: s.auto_approve_level ?? "caution",
        model_key: s.model_key,
      });
      sessionState.sessions.push(session);
    } else {
      session.project_path = s.project_path ?? session.project_path;
      session.project_id = s.project_id ?? session.project_id;
      session.alias = s.title ?? session.alias ?? "Untitled";
      session.permission_level =
        s.auto_approve_level ?? session.permission_level;
      session.updated_at = s.updated_at ?? s.created_at ?? session.updated_at;
      session.model_key = s.model_key ?? session.model_key;
    }
    return session;
  }

  async function loadAllSessions(load_more = false) {
    if (!load_more && allSessionsLoadedShared) {
      allSessionsLoaded = true;
      allSessionsCursor = allSessionsCursorShared;
      allSessionsError = false;
      return;
    }
    if (load_more && !allSessionsCursorShared) return;
    if (allSessionsLoadPromise) {
      allSessionsLoading = true;
      try {
        await allSessionsLoadPromise;
        allSessionsLoaded = allSessionsLoadedShared;
        allSessionsCursor = allSessionsCursorShared;
        allSessionsError = false;
      } catch {
        allSessionsError = true;
      } finally {
        allSessionsLoading = false;
      }
      return;
    }

    allSessionsLoading = true;
    allSessionsError = false;
    const cursor = load_more
      ? (allSessionsCursorShared ?? undefined)
      : undefined;
    allSessionsLoadPromise = (async () => {
      const result = await api.listSessions(undefined, cursor, 30);
      for (const session of result.sessions) mergeSessionInfo(session);
      allSessionsCursorShared = result.next_cursor;
      allSessionsLoadedShared = true;
    })();

    try {
      await allSessionsLoadPromise;
      allSessionsCursor = allSessionsCursorShared;
      allSessionsLoaded = true;
    } catch (e: unknown) {
      console.error(
        "Failed to load all sessions:",
        e instanceof Error ? e.message : e,
      );
      allSessionsError = true;
    } finally {
      allSessionsLoadPromise = null;
      allSessionsLoading = false;
    }
  }

  $effect(() => {
    if (sidebarView === "sessions") void loadAllSessions();
  });

  function switchSidebarView(view: SidebarViewPreference) {
    if (sidebarView === view) return;
    guiPreferences.layout.sidebar_view = view;
    closeMenus();
    void saveGuiPreferences(snapshotGuiPreferences());
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
        for (const s of result.sessions) mergeSessionInfo(s);
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
      delete unreadSessions[id];
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
      for (const sid of removedIds) {
        delete unreadSessions[sid];
        delete pinnedSessionMeta[sid];
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

  async function forkSession(session_id: string) {
    if (forkingSessionId) return;
    forkingSessionId = session_id;
    closeMenus();
    try {
      const session = await forkSessionState(session_id);
      showNotification("Session forked", "success");
      await activateSession(session.id);
    } catch (e: unknown) {
      console.error(
        "Failed to fork session:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to fork session", "error");
    } finally {
      forkingSessionId = null;
    }
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
    target: "pinned" | "session",
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
      role="presentation"
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
      class="scrollbar-hidden shrink-0 max-h-[33%] overflow-y-auto px-2 py-1"
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
              class="group flex w-full items-center gap-2 rounded-sm border-l-2 py-1 pl-3 pr-1 transition-colors {active
                ? 'border-primary bg-primary/8 text-foreground'
                : 'border-transparent text-muted-foreground hover:border-border hover:bg-secondary/40 hover:text-foreground'}"
            >
              <button
                type="button"
                class="min-w-0 flex flex-1 items-center gap-2 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                onclick={() => activateSession(session_id)}
              >
                <span
                  class="flex-1 truncate text-sm font-medium"
                  title={session?.alias ?? "Untitled"}
                >
                  {session?.alias ?? "Untitled"}
                </span>
                {#if session?.project_id}
                  <span
                    class="max-w-[6rem] shrink-0 truncate text-[10px] text-muted-foreground"
                    >{projectName(session.project_id)}</span
                  >
                {/if}
              </button>
              <div class="relative h-5 w-5 shrink-0">
                <div
                  class="absolute inset-0 flex items-center justify-center gap-1 transition-opacity {pinnedMenu?.session_id ===
                  session_id
                    ? 'opacity-0'
                    : 'group-hover:opacity-0'}"
                >
                  {#if unreadSessions[session_id]}
                    <span
                      class="h-1.5 w-1.5 rounded-full bg-primary"
                      aria-label="Unread"
                      title="Unread"
                    ></span>
                  {/if}
                  {#if session}
                    <StatusDot phase={session.phase} />
                  {/if}
                </div>
                <button
                  type="button"
                  aria-label="Pinned session actions"
                  title="Pinned session actions"
                  class="absolute inset-0 flex items-center justify-center rounded opacity-0 transition-opacity hover:bg-secondary/80 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring {pinnedMenu?.session_id ===
                  session_id
                    ? 'opacity-100'
                    : 'group-hover:opacity-100'}"
                  onclick={(e: MouseEvent) => openMenu(e, "pinned", session_id)}
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
                    <SessionForkMenuItem
                      {session_id}
                      disabled={forkingSessionId !== null}
                      onfork={(id) => void forkSession(id)}
                    />
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
        {/each}
      </div>
    </div>
    <div class="h-px bg-border mx-2 mb-1 shrink-0"></div>
  {/if}

  {#if !collapsed}
    <div class="shrink-0 px-2 pb-1" aria-label="Session view">
      <div
        class="relative grid h-8 grid-cols-2 gap-0.5 rounded-md bg-secondary/60 p-0.5"
      >
        <span
          class="pointer-events-none absolute bottom-0.5 left-0.5 top-0.5 w-[calc(50%-0.1875rem)] rounded-[4px] bg-background shadow-sm transition-transform duration-200 ease-out motion-reduce:transition-none {sidebarView ===
          'projects'
            ? 'translate-x-[calc(100%+0.125rem)]'
            : 'translate-x-0'}"
          aria-hidden="true"
        ></span>
        <button
          type="button"
          aria-pressed={sidebarView === "sessions"}
          aria-label="Sessions"
          title="Sessions"
          class="relative z-10 flex min-w-0 items-center justify-center rounded-[4px] px-2 transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring {sidebarView ===
          'sessions'
            ? 'text-foreground'
            : 'text-muted-foreground hover:text-foreground'}"
          onclick={() => switchSidebarView("sessions")}
        >
          <MessageSquare size={15} />
        </button>
        <button
          type="button"
          aria-pressed={sidebarView === "projects"}
          aria-label="Projects"
          title="Projects"
          class="relative z-10 flex min-w-0 items-center justify-center rounded-[4px] px-2 transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring {sidebarView ===
          'projects'
            ? 'text-foreground'
            : 'text-muted-foreground hover:text-foreground'}"
          onclick={() => switchSidebarView("projects")}
        >
          <Folder size={15} />
        </button>
      </div>
    </div>
  {/if}

  <div
    class="scrollbar-hidden flex-1 min-h-0 overflow-y-auto py-1 {collapsed
      ? 'px-1'
      : 'px-2'}"
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
              {#if unreadSessions[session.id]}
                <span
                  class="absolute -top-0.5 -left-0.5 h-1.5 w-1.5 rounded-full bg-primary ring-2 ring-card"
                  aria-label="Unread"
                  title="Unread"
                ></span>
              {/if}
              <span class="absolute -top-0.5 -right-0.5">
                <StatusDot phase={session.phase} />
              </span>
            </button>
          {/each}
        </div>
      {/each}
    {:else if sidebarView === "sessions"}
      <div class="space-y-0.5" in:fly={{ x: -8, duration: 160, opacity: 0.3 }}>
        {#each sessionTimeGroups as group (group.label ?? "recent")}
          <section aria-label={group.label ?? "Recent sessions"}>
            {#if group.label}
              <h3
                id={`session-group-${group.label.replaceAll(" ", "-").toLowerCase()}`}
                class="sticky -top-1 z-10 flex items-center justify-center gap-2 bg-card/95 px-3 py-1 text-[10px] font-medium leading-none text-muted-foreground backdrop-blur-sm"
              >
                <span class="h-px w-8 bg-border" aria-hidden="true"></span>
                <span class="shrink-0">{group.label}</span>
                <span class="h-px w-8 bg-border" aria-hidden="true"></span>
              </h3>
            {/if}
            <div class="space-y-0.5">
              {#each group.sessions as session (session.id)}
                {@const project = projectState.projects.find(
                  (item) => item.id === session.project_id,
                )}
                <div
                  class="group relative flex min-h-11 w-full items-center gap-2 rounded-sm border-l-2 py-1.5 pl-2 pr-0.5 transition-colors {session.id ===
                  sessionState.activeSessionId
                    ? 'border-primary bg-primary/8 text-foreground'
                    : 'border-transparent text-muted-foreground hover:border-border hover:bg-secondary/40 hover:text-foreground'}"
                >
                  <div class="min-w-0 flex-1">
                    {#if renamingSessionId === session.id}
                      <input
                        type="text"
                        use:focusAndSelect
                        bind:value={renameValue}
                        onkeydown={(e: KeyboardEvent) => {
                          e.stopPropagation();
                          if (e.key === "Enter")
                            confirmRenameSession(session.id);
                          if (e.key === "Escape") renamingSessionId = null;
                        }}
                        onclick={(e: MouseEvent) => e.stopPropagation()}
                        onblur={() => confirmRenameSession(session.id)}
                        class="w-full min-w-0 rounded border border-border bg-background px-1 py-0.5 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
                      />
                    {:else}
                      <button
                        type="button"
                        class="block w-full text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                        onclick={() => activateSession(session.id)}
                      >
                        <span
                          class="block truncate text-sm font-medium"
                          title={session.alias ?? "Untitled"}
                        >
                          {session.alias ?? "Untitled"}
                        </span>
                        <span
                          class="mt-0.5 flex min-w-0 items-center gap-1 text-[10px] text-muted-foreground"
                        >
                          {#if session.project_id}
                            {#if project}
                              <ProjectDot
                                name={project.name}
                                dir={project.dir}
                              />
                            {/if}
                            <span class="truncate"
                              >{projectName(session.project_id)}</span
                            >
                            {#if session.updated_at}
                              <span aria-hidden="true">·</span>
                            {/if}
                          {/if}
                          {#if session.updated_at}
                            <span
                              class="shrink-0"
                              title={new Date(
                                session.updated_at,
                              ).toLocaleString()}
                            >
                              {formatTimeAgo(session.updated_at, clock.now)}
                            </span>
                          {/if}
                        </span>
                      </button>
                    {/if}
                  </div>
                  <div class="relative h-5 w-5 shrink-0">
                    <div
                      class="absolute inset-0 flex items-center justify-center gap-1 transition-opacity {projectMenu?.session_id ===
                      session.id
                        ? 'opacity-0'
                        : 'group-hover:opacity-0'}"
                    >
                      {#if unreadSessions[session.id]}
                        <span
                          class="h-1.5 w-1.5 rounded-full bg-primary"
                          aria-label="Unread"
                          title="Unread"
                        ></span>
                      {/if}
                      <StatusDot phase={session.phase} />
                    </div>
                    <button
                      type="button"
                      aria-label="Session actions"
                      title="Session actions"
                      class="absolute inset-0 flex items-center justify-center rounded opacity-0 transition-opacity hover:bg-secondary/80 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring {projectMenu?.session_id ===
                      session.id
                        ? 'opacity-100'
                        : 'group-hover:opacity-100'}"
                      onclick={(e: MouseEvent) =>
                        openMenu(e, "session", session.id)}
                    >
                      <MoreVertical size={13} />
                    </button>
                  </div>
                  {#if projectMenu?.session_id === session.id}
                    <div
                      class="fixed z-50 w-36 rounded-md border border-border bg-popover py-1 shadow-md"
                      style="top: {projectMenu.y}px; left: {projectMenu.x}px; transform: translateX(-100%);"
                    >
                      <button
                        class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs text-foreground hover:bg-secondary/50"
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
                        class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs text-foreground hover:bg-secondary/50"
                        onclick={(e: Event) => {
                          e.stopPropagation();
                          renamingSessionId = session.id;
                          renameValue = session.alias ?? "Untitled";
                          projectMenu = null;
                        }}
                      >
                        <Pencil size={12} /> Rename
                      </button>
                      <SessionForkMenuItem
                        session_id={session.id}
                        disabled={forkingSessionId !== null}
                        onfork={(id) => void forkSession(id)}
                      />
                      <button
                        class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs text-foreground hover:bg-secondary/50"
                        onclick={(e: Event) => {
                          e.stopPropagation();
                          copySessionId(session.id);
                          projectMenu = null;
                        }}
                      >
                        <Copy size={12} /> Copy ID
                      </button>
                      <button
                        class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs text-destructive hover:bg-destructive/10"
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
              {/each}
            </div>
          </section>
        {/each}

        {#if allSessionsLoading}
          <div class="px-3 py-3 text-center text-xs text-muted-foreground">
            Loading sessions...
          </div>
        {:else if allSessionsError}
          <button
            type="button"
            class="w-full px-3 py-3 text-center text-xs text-error transition-colors hover:text-foreground"
            onclick={() =>
              loadAllSessions(allSessionsLoaded && Boolean(allSessionsCursor))}
          >
            Couldn’t load all sessions — retry
          </button>
        {:else if allSessionsLoaded && allSessions.length === 0}
          <div class="px-3 py-6 text-center">
            <MessageSquare
              size={18}
              class="mx-auto mb-2 text-muted-foreground"
            />
            <p class="text-xs font-medium text-foreground">No sessions yet</p>
            <p class="mt-0.5 text-[11px] text-muted-foreground">
              Start a new chat to see it here.
            </p>
          </div>
        {/if}

        {#if allSessionsCursor && !allSessionsLoading}
          <button
            type="button"
            class="w-full rounded-sm px-3 py-2 text-center text-xs text-muted-foreground transition-colors hover:bg-secondary/40 hover:text-foreground"
            onclick={() => loadAllSessions(true)}
          >
            Load more
          </button>
        {/if}
      </div>
    {:else}
      <div in:fly={{ x: 8, duration: 160, opacity: 0.3 }}>
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
                    class="group relative flex w-full items-center gap-2 rounded-sm border-l-2 py-1 pl-2 pr-0.5 transition-colors {session.id ===
                    sessionState.activeSessionId
                      ? 'border-primary bg-primary/8 text-foreground'
                      : 'border-transparent text-muted-foreground hover:border-border hover:bg-secondary/40 hover:text-foreground'}"
                  >
                    {#if renamingSessionId === session.id}
                      <input
                        type="text"
                        use:focusAndSelect
                        bind:value={renameValue}
                        onkeydown={(e: KeyboardEvent) => {
                          if (e.key === "Enter")
                            confirmRenameSession(session.id);
                          if (e.key === "Escape") renamingSessionId = null;
                        }}
                        onblur={() => confirmRenameSession(session.id)}
                        class="flex-1 min-w-0 bg-background border border-border rounded px-1 py-0.5 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
                      />
                    {:else}
                      <button
                        type="button"
                        class="min-w-0 flex-1 truncate text-left text-sm font-medium focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                        title={session.alias ?? "Untitled"}
                        onclick={() => activateSession(session.id)}
                      >
                        {session.alias ?? "Untitled"}
                      </button>
                    {/if}
                    <div class="flex shrink-0 items-center gap-1.5">
                      {#if session.updated_at}
                        <span
                          class="text-[10px] text-muted-foreground"
                          title={new Date(session.updated_at).toLocaleString()}
                        >
                          {formatTimeAgo(session.updated_at, clock.now)}
                        </span>
                      {/if}
                      <div class="relative h-5 w-5 shrink-0">
                        <div
                          class="absolute inset-0 flex items-center justify-center gap-1 transition-opacity {projectMenu?.session_id ===
                          session.id
                            ? 'opacity-0'
                            : 'group-hover:opacity-0'}"
                        >
                          {#if unreadSessions[session.id]}
                            <span
                              class="h-1.5 w-1.5 rounded-full bg-primary"
                              aria-label="Unread"
                              title="Unread"
                            ></span>
                          {/if}
                          <StatusDot phase={session.phase} />
                        </div>
                        <button
                          type="button"
                          aria-label="Session actions"
                          title="Session actions"
                          class="absolute inset-0 flex items-center justify-center rounded opacity-0 transition-opacity hover:bg-secondary/80 focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring {projectMenu?.session_id ===
                          session.id
                            ? 'opacity-100'
                            : 'group-hover:opacity-100'}"
                          onclick={(e: MouseEvent) =>
                            openMenu(e, "session", session.id)}
                        >
                          <MoreVertical size={12} />
                        </button>
                      </div>
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
                          <SessionForkMenuItem
                            session_id={session.id}
                            disabled={forkingSessionId !== null}
                            onfork={(id) => void forkSession(id)}
                          />
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
      </div>
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
