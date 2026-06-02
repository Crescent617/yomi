<script lang="ts">
  import { Plus, Folder, FolderOpen, MoreVertical, Pencil, Trash2, X } from "lucide-svelte";
  import * as api from "../../api";
  import {
    sessionState,
    projectState,
    sessionCursors,
    setActiveSession,
    loadSessionMessages,
    getSession,
    showNotification,
  } from "../../state.svelte";

  let { collapsed = false }: { collapsed?: boolean } = $props();

  // ===== project-based grouping =====
  const groups = $derived.by(() => {
    const map = new Map<string, { name: string; sessions: typeof sessionState.sessions }>();
    for (const s of sessionState.sessions) {
      if (!s.projectId) continue; // skip orphan sessions
      const project = projectState.projects.find((p) => p.id === s.projectId);
      const key = s.projectId;
      const name = project?.name ?? formatPath(s.projectPath);
      const existing = map.get(key);
      if (existing) {
        existing.sessions.push(s);
      } else {
        map.set(key, { name, sessions: [s] });
      }
    }
    // Sort sessions within each group by updatedAt desc (most recent first)
    for (const [, group] of map) {
      group.sessions.sort((a, b) => (b.updatedAt ?? "").localeCompare(a.updatedAt ?? ""));
    }
    const entries = Array.from(map.entries());
    // Sort projects by most-recent session updatedAt desc (stable: does not change on click)
    entries.sort((a, b) => {
      const aLatest = a[1].sessions[0]?.updatedAt ?? "";
      const bLatest = b[1].sessions[0]?.updatedAt ?? "";
      return bLatest.localeCompare(aLatest);
    });
    return entries;
  });

  // ===== expand/collapse =====
  let expanded = $state(new Set<string>());
  // always include the active project, and the most-recent project when no active session
  const allExpanded = $derived.by(() => {
    const active = getSession(sessionState.activeSessionId ?? "");
    const activeKey = active?.projectId ?? "";
    const next = new Set(expanded);
    if (activeKey) {
      next.add(activeKey);
    } else {
      // No active session: auto-expand the project with the most recent session
      let latestProjectId = "";
      let latestTime = "";
      for (const s of sessionState.sessions) {
        if (!s.projectId) continue;
        const t = s.updatedAt ?? "";
        if (t > latestTime) {
          latestTime = t;
          latestProjectId = s.projectId;
        }
      }
      if (latestProjectId) next.add(latestProjectId);
    }
    return next;
  });

  function toggleGroup(key: string) {
    const next = new Set(expanded);
    if (next.has(key)) next.delete(key);
    else {
      next.add(key);
      // Load sessions for this project when expanding
      loadMoreSessions(key);
    }
    expanded = next;
  }

  // ===== helpers =====
  function formatPath(path: string): string {
    if (!path || path === "unknown") return "unknown";
    const sep = path.includes("/") ? "/" : "\\";
    const parts = path.split(sep);
    return parts[parts.length - 1] || path;
  }

  function formatShortId(id: string) {
    return id.slice(0, 8);
  }

  // ===== per-project session loading =====
  const loadingProjects = new Set<string>();

  async function loadMoreSessions(projectId: string) {
    if (loadingProjects.has(projectId)) return;
    loadingProjects.add(projectId);
    const cursor = sessionCursors.get(projectId) ?? undefined;
    try {
      const result = await api.listSessions(
        projectId || undefined,
        cursor,
        20,
      );
      for (const s of result.sessions) {
        if (!sessionState.sessions.find((sess) => sess.id === s.id)) {
          sessionState.sessions.push({
            id: s.id,
            projectPath: s.projectPath ?? "",
            projectId: s.projectId,
            alias: s.title,
            messages: [],
            streaming: false,
            unread: 0,
            checkpoints: [],
            tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
            activeTabId: "chat",
            pendingPermissions: [],
            pendingAskUser: null,
            queuedInput: null,
            updatedAt: s.endedAt ?? s.createdAt,
          });
        }
      }
      // Update cursor for next page
      if (result.hasMore && result.sessions.length > 0) {
        const last = result.sessions[result.sessions.length - 1];
        sessionCursors.set(projectId, last.endedAt ?? last.createdAt);
      } else {
        sessionCursors.set(projectId, null); // no more
      }
    } catch (e: any) {
      console.error("Failed to load sessions:", e?.message ?? e);
    } finally {
      loadingProjects.delete(projectId);
    }
  }

  // ===== project menu =====
  let showProjectMenu = $state<string | null>(null);
  let renamingProjectId = $state<string | null>(null);
  let renameValue = $state("");

  function startRenameProject(projectId: string, currentName: string) {
    renamingProjectId = projectId;
    renameValue = currentName;
    showProjectMenu = null;
  }

  async function confirmRenameProject(projectId: string) {
    const name = renameValue.trim();
    if (!name) {
      renamingProjectId = null;
      return;
    }
    try {
      await api.renameProject(projectId, name);
      const p = projectState.projects.find((x) => x.id === projectId);
      if (p) p.name = name;
      showNotification("Project renamed", "success", 2000);
    } catch (e: any) {
      console.error("Failed to rename project:", e?.message ?? e);
      showNotification("Failed to rename project", "error", 3000);
    } finally {
      renamingProjectId = null;
    }
  }

  async function deleteProject(projectId: string) {
    showProjectMenu = null;
    const hasSessions = sessionState.sessions.some((s) => s.projectId === projectId);
    if (hasSessions) {
      showNotification("Cannot delete project with sessions", "error", 3000);
      return;
    }
    try {
      await api.deleteProject(projectId);
      projectState.projects = projectState.projects.filter((p) => p.id !== projectId);
      showNotification("Project deleted", "success", 2000);
    } catch (e: any) {
      console.error("Failed to delete project:", e?.message ?? e);
      showNotification("Failed to delete project", "error", 3000);
    }
  }

  // ===== create / navigate =====
  function goToCreatePage() {
    setActiveSession(null);
  }

  async function quickCreateSession(targetProjectId: string) {
    try {
      const project = projectState.projects.find((p) => p.id === targetProjectId);
      if (!project) {
        showNotification("Project not found", "error", 3000);
        return;
      }
      const id = await api.createSession(project.dir, "safe", targetProjectId);
      // refresh sessions for this project
      await loadMoreSessions(targetProjectId);
      await activateSession(id);
    } catch (e: any) {
      console.error("Failed to create session:", e?.message ?? e);
      showNotification("Failed to create session", "error", 3000);
    }
  }

  // ===== activate =====
  async function deleteSession(sessionId: string) {
    if (!confirm("Delete this session?")) return;
    try {
      await api.deleteSession(sessionId);
      sessionState.sessions = sessionState.sessions.filter((s) => s.id !== sessionId);
      if (sessionState.activeSessionId === sessionId) {
        setActiveSession(null);
      }
      showNotification("Session deleted", "success", 2000);
    } catch (e: any) {
      console.error("Failed to delete session:", e?.message ?? e);
      showNotification("Failed to delete session", "error", 3000);
    }
  }

  async function activateSession(id: string) {
    const prevId = sessionState.activeSessionId;
    try {
      if (prevId && prevId !== id) {
        await api.unsubscribe(prevId);
      }
      await api.subscribe(id);
      setActiveSession(id);
      const msgs = await api.getMessages(id);
      const session = getSession(id);
      if (session) {
        loadSessionMessages(id, msgs);
      }
    } catch (e: any) {
      console.error("Failed to activate session:", e?.message ?? e);
      if (prevId && prevId !== id) {
        try {
          await api.subscribe(prevId);
          setActiveSession(prevId);
        } catch {
          setActiveSession(null);
        }
      } else {
        setActiveSession(null);
      }
    }
  }
</script>

<div class="flex flex-col h-full {collapsed ? 'items-center' : ''}">
  {#if !collapsed}
    <!-- new session button -->
    <div class="shrink-0 p-2 border-b border-border/50">
      <button
        class="w-full flex items-center justify-center gap-1.5 rounded-lg border border-border
               bg-secondary px-3 py-2 text-sm font-medium text-secondary-foreground
               hover:bg-secondary/80 active:scale-[0.98] transition-all"
        onclick={goToCreatePage}
        title="Create new session"
      >
        <Plus size={16} />
        New Session
      </button>
    </div>
  {:else}
    <!-- collapsed mini button -->
    <div class="shrink-0 py-2">
      <button
        class="w-8 h-8 rounded-lg flex items-center justify-center text-muted-foreground
               hover:bg-secondary hover:text-foreground transition-colors"
        onclick={goToCreatePage}
        title="New Session"
      >
        <Plus size={16} />
      </button>
    </div>
  {/if}

  <!-- session list -->
  <div class="flex-1 min-h-0 overflow-y-auto py-1 {collapsed ? 'px-1' : 'px-2'}">
    {#if collapsed}
      <!-- collapsed: flat mini avatars -->
      {#each groups as [key, group], groupIdx (key)}
        <div class="flex flex-col items-center gap-1.5">
          {#each group.sessions as session (session.id)}
            {@const isActive = session.id === sessionState.activeSessionId}
            <div class="relative">
              <button
                class="w-8 h-8 rounded-lg flex items-center justify-center text-[10px] font-bold
                       transition-colors
                       {isActive
                         ? 'bg-primary text-primary-foreground shadow-sm'
                         : 'bg-secondary text-secondary-foreground hover:bg-secondary/80'}"
                onclick={() => activateSession(session.id)}
                title="{session.alias ?? formatShortId(session.id)} — {group.name}"
              >
                {(session.alias ?? formatShortId(session.id)).slice(0, 2).toUpperCase()}
              </button>
              {#if session.unread > 0}
                <span
                  class="absolute -top-1 -right-1 min-w-[14px] h-3.5 px-0.5 rounded-full bg-destructive
                         text-destructive-foreground text-[8px] flex items-center justify-center font-bold leading-none"
                >
                  {session.unread > 9 ? "9+" : session.unread}
                </span>
              {/if}
            </div>
          {/each}
        </div>
        {#if groupIdx < groups.length - 1}
          <div class="w-4 h-px bg-border/50 my-1.5 mx-auto"></div>
        {/if}
      {/each}
    {:else}
      <!-- expanded: grouped tree -->
      {#each groups as [key, group] (key)}
        {@const isActiveProject =
          key === (getSession(sessionState.activeSessionId ?? "")?.projectId ?? "")}
        {@const isExpanded = allExpanded.has(key)}

        <div class="rounded-md overflow-hidden mb-0.5">
          <!-- project header -->
          <div
            class="flex items-center gap-1.5 w-full rounded-md px-2 py-1.5 text-xs
                   transition-colors select-none
                   {isActiveProject
              ? 'text-foreground bg-secondary/60'
              : 'text-muted-foreground hover:text-foreground hover:bg-secondary/40'}"
          >
            <button
              class="flex items-center gap-1.5 flex-1 min-w-0 text-left"
              onclick={() => toggleGroup(key)}
              title={group.name}
            >
              {#if isExpanded}
                <FolderOpen size={13} class="shrink-0 opacity-70" />
              {:else}
                <Folder size={13} class="shrink-0 opacity-70" />
              {/if}
              {#if renamingProjectId === key}
                <input
                  type="text"
                  bind:value={renameValue}
                  onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter') confirmRenameProject(key); if (e.key === 'Escape') renamingProjectId = null; }}
                  onblur={() => confirmRenameProject(key)}
                  class="flex-1 min-w-0 bg-background border border-border rounded px-1 py-0.5 text-xs focus:outline-none focus:ring-1 focus:ring-ring"
                  autofocus
                />
              {:else}
                <span class="truncate font-medium">{group.name}</span>
              {/if}
            </button>

            {#if key !== ""}
              <!-- more menu -->
              <div class="relative">
                <button
                  class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-colors"
                  onclick={(e: Event) => { e.stopPropagation(); showProjectMenu = showProjectMenu === key ? null : key; }}
                  title="Project options"
                >
                  <MoreVertical size={12} />
                </button>
                {#if showProjectMenu === key}
                  <div class="absolute right-0 top-full mt-1 z-20 w-32 rounded-md border border-border bg-popover shadow-md py-1">
                    <button
                      class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                      onclick={(e: Event) => { e.stopPropagation(); startRenameProject(key, group.name); }}
                    >
                      <Pencil size={12} />
                      Rename
                    </button>
                    <button
                      class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/10 text-left"
                      onclick={(e: Event) => { e.stopPropagation(); deleteProject(key); }}
                    >
                      <Trash2 size={12} />
                      Delete
                    </button>
                  </div>
                  <!-- click outside to close -->
                  <div class="fixed inset-0 z-10" onclick={() => showProjectMenu = null}></div>
                {/if}
              </div>
            {/if}

            <button
              class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-colors"
              onclick={() => quickCreateSession(key)}
              title="Create session in this project"
            >
              <Plus size={12} />
            </button>
          </div>

          {#if isExpanded}
            <div class="ml-4 pl-2 border-l border-border/40 space-y-0.5 pb-1">
              {#each group.sessions as session (session.id)}
                <div
                  class="group w-full flex items-center gap-2 rounded-lg px-3 py-2 cursor-pointer
                         transition-colors {session.id === sessionState.activeSessionId
                    ? 'bg-primary/10 text-foreground'
                    : 'hover:bg-secondary/50 text-muted-foreground hover:text-foreground'}"
                  onclick={() => activateSession(session.id)}
                  role="button"
                  tabindex="0"
                  onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter' || e.key === ' ') activateSession(session.id); }}
                >
                  <span class="flex-1 truncate text-sm font-medium" title={session.alias ?? formatShortId(session.id)}>
                    {session.alias ?? formatShortId(session.id)}
                  </span>
                  <div class="flex items-center gap-1.5 shrink-0 opacity-0 group-hover:opacity-100 transition-opacity">
                    {#if session.streaming}
                      <span class="w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span>
                    {/if}
                    {#if session.unread > 0}
                      <span
                        class="min-w-[1.1rem] h-4 px-1 rounded-full bg-destructive
                               text-destructive-foreground text-[10px] font-bold flex items-center justify-center"
                      >
                        {session.unread > 99 ? "99+" : session.unread}
                      </span>
                    {/if}
                    <button
                      type="button"
                      class="p-0.5 rounded hover:bg-destructive/10 text-muted-foreground hover:text-destructive transition-colors"
                      onclick={(e: Event) => { e.stopPropagation(); deleteSession(session.id); }}
                      title="Delete session"
                    >
                      <X size={12} />
                    </button>
                  </div>
                </div>
              {/each}
              {#if sessionCursors.get(key) !== null}
                <button
                  class="w-full text-left px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors"
                  onclick={() => loadMoreSessions(key)}
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
