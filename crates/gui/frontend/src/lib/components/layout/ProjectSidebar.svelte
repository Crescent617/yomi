<script lang="ts">
  import { onMount } from "svelte";
  import { Plus, Folder, FolderOpen, MoreVertical, Pencil, Trash2, Copy } from "lucide-svelte";
  import * as api from "../../api";
  import {
    sessionState,
    projectState,
    sessionCursors,
    setActiveSession,
    loadSessionMessages,
    getSession,
    showNotification,
    syncSessionStatus,
    refreshCheckpoints,
  } from "../../state.svelte";

  let { collapsed = false }: { collapsed?: boolean } = $props();

  let expanded = $state<Record<string, boolean>>({});
  let loading = $state<Record<string, boolean>>({});
  let showMenu = $state<string | null>(null);
  let showSessionMenu = $state<string | null>(null);
  let renamingProjectId = $state<string | null>(null);
  let renamingSessionId = $state<string | null>(null);
  let renameValue = $state("");

  onMount(() => {
    if (projectState.projects.length === 0) {
      api.listProjects().then((list) => {
        projectState.projects = list.map((p) => ({ ...p }));
        // Auto-expand first 3 projects and load their sessions
        const firstN = list.slice(0, 3).map((p) => p.id);
        if (firstN.length > 0) {
          expanded = Object.fromEntries(firstN.map((id) => [id, true]));
          for (const id of firstN) {
            loadSessions(id);
          }
        }
      }).catch(console.error);
    } else if (projectState.projects.length > 0) {
      // Projects already loaded (e.g. HMR), expand first 3
      const firstN = projectState.projects.slice(0, 3).map((p) => p.id);
      expanded = Object.fromEntries(firstN.map((id) => [id, true]));
      for (const id of firstN) {
        loadSessions(id);
      }
    }
  });

  function getSessions(projectId: string) {
    return sessionState.sessions
      .filter((s) => s.projectId === projectId)
      .sort((a, b) => (b.updatedAt ?? "").localeCompare(a.updatedAt ?? ""));
  }

  function hasMore(projectId: string) {
    // Only show "Load more" when we have an actual page token (cursor is a string).
    // Cursor lifecycle:
    //   undefined = not loaded yet (initial load triggered by expand)
    //   string    = has next page
    //   null      = no more pages
    return typeof sessionCursors.get(projectId) === "string";
  }

  async function toggle(projectId: string) {
    const next = { ...expanded, [projectId]: !expanded[projectId] };
    expanded = next;
    if (next[projectId]) {
      await loadSessions(projectId);
    }
  }

  async function loadSessions(projectId: string) {
    if (loading[projectId]) return;
    const cursor = sessionCursors.get(projectId);
    // cursor === null  → already reached end, skip
    // cursor === undefined → first load (triggered by expand), load page 1
    if (cursor === null) return;

    loading = { ...loading, [projectId]: true };
    try {
      const result = await api.listSessions(projectId, cursor, 20);
      for (const s of result.sessions) {
        const existing = sessionState.sessions.find((sess) => sess.id === s.id);
        if (!existing) {
          sessionState.sessions.push({
            id: s.id,
            projectPath: s.projectPath ?? "",
            projectId: s.projectId,
            alias: s.title ?? "Untitled",
            messages: [],
          phase: "idle",
            unread: 0,
            checkpoints: [],
            tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
            activeTabId: "chat",
            pendingPermissions: [],
            pendingAskUser: null,
            queuedInput: null,
            updatedAt: s.endedAt ?? s.createdAt,
            permissionLevel: s.autoApproveLevel ?? "caution",
          });
        } else {
          existing.alias = s.title ?? existing.alias ?? "Untitled";
          existing.permissionLevel = s.autoApproveLevel ?? existing.permissionLevel;
          existing.updatedAt = s.endedAt ?? s.createdAt ?? existing.updatedAt;
        }
      }
      const last = result.sessions[result.sessions.length - 1];
      if (result.hasMore && last) {
        sessionCursors.set(projectId, last.endedAt ?? last.createdAt);
      } else {
        sessionCursors.set(projectId, null);
      }
    } catch (e: unknown) {
      console.error("Failed to load sessions:", e instanceof Error ? e.message : e);
      // Keep cursor as-is so user can retry. If this was the first load,
      // cursor is still undefined and expand will retry on next toggle.
    } finally {
      loading = { ...loading, [projectId]: false };
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
      console.error("Failed to activate session:", e instanceof Error ? e.message : e);
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
      if (sessionState.activeSessionId === id) setActiveSession(null);
      showNotification("Session deleted", "success", 2000);
    } catch (e: unknown) {
      console.error("Failed to delete session:", e instanceof Error ? e.message : e);
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
      console.error("Failed to delete project:", e instanceof Error ? e.message : e);
      showNotification("Failed to delete project", "error", 3000);
    }
  }

  async function quickCreateSession(projectId: string) {
    const project = projectState.projects.find((p) => p.id === projectId);
    if (!project) return;
    try {
      const config = await api.getConfig();
      const id = await api.createSession(project.dir, config?.autoApprove ?? "caution", projectId);
      sessionState.sessions.push({
        id,
        projectPath: project.dir,
        projectId,
        alias: "Untitled",
        messages: [],
        phase: "idle",
        unread: 0,
        checkpoints: [],
        tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
        activeTabId: "chat",
        pendingPermissions: [],
        pendingAskUser: null,
        queuedInput: null,
        updatedAt: new Date().toISOString(),
        permissionLevel: config?.autoApprove ?? "caution",
      });
      await activateSession(id);
    } catch (e: unknown) {
      console.error("Failed to create session:", e instanceof Error ? e.message : e);
      showNotification("Failed to create session", "error", 3000);
    }
  }

  async function confirmRenameProject(projectId: string) {
    const name = renameValue.trim();
    if (!name) { renamingProjectId = null; return; }
    try {
      await api.renameProject(projectId, name);
      const p = projectState.projects.find((x) => x.id === projectId);
      if (p) p.name = name;
      showNotification("Project renamed", "success", 2000);
    } catch (e: unknown) {
      console.error("Failed to rename project:", e instanceof Error ? e.message : e);
      showNotification("Failed to rename project", "error", 3000);
    }
    renamingProjectId = null;
  }

  async function confirmRenameSession(sessionId: string) {
    const name = renameValue.trim();
    if (!name) { renamingSessionId = null; return; }
    try {
      await api.renameSession(sessionId, name);
      const s = sessionState.sessions.find((x) => x.id === sessionId);
      if (s) s.alias = name;
      showNotification("Session renamed", "success", 2000);
    } catch (e: unknown) {
      console.error("Failed to rename session:", e instanceof Error ? e.message : e);
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
</script>

<div class="flex flex-col h-full {collapsed ? 'items-center' : ''}">
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
      <button class="w-8 h-8 rounded-lg flex items-center justify-center text-muted-foreground hover:bg-secondary hover:text-foreground transition-colors" onclick={() => setActiveSession(null)}>
        <Plus size={16} />
      </button>
    </div>
  {/if}

  <div class="flex-1 min-h-0 overflow-y-auto py-1 {collapsed ? 'px-1' : 'px-2'}">
    {#if collapsed}
      {#each projectState.projects as project (project.id)}
        <div class="flex flex-col items-center gap-1">
          <!-- Project divider -->
          <div class="w-8 h-8 rounded-lg flex items-center justify-center text-[10px] font-bold bg-transparent text-muted-foreground mt-1 mb-0.5" title={project.name}>
            {project.name.slice(0, 2).toUpperCase()}
          </div>
          {#each getSessions(project.id) as session (session.id)}
            <button
              class="w-8 h-8 rounded-lg flex items-center justify-center text-[10px] font-bold transition-colors {session.id === sessionState.activeSessionId ? 'bg-primary text-primary-foreground' : 'bg-secondary text-secondary-foreground hover:bg-secondary/80'}"
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
        {@const isActive = getSession(sessionState.activeSessionId ?? "")?.projectId === project.id}
        <div class="rounded-md mb-0.5">
          <div class="flex items-center gap-1.5 w-full rounded-md px-2 py-1.5 text-xs transition-colors select-none {isActive ? 'text-foreground bg-secondary/60' : 'text-muted-foreground hover:text-foreground hover:bg-secondary/40'}">
            <button class="flex items-center gap-1.5 flex-1 min-w-0 text-left" onclick={() => toggle(project.id)}>
              {#if expanded[project.id]}
                <FolderOpen size={13} class="shrink-0 opacity-70" />
              {:else}
                <Folder size={13} class="shrink-0 opacity-70" />
              {/if}
              {#if renamingProjectId === project.id}
                <input
                  type="text"
                  bind:value={renameValue}
                  onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter') confirmRenameProject(project.id); if (e.key === 'Escape') renamingProjectId = null; }}
                  onblur={() => confirmRenameProject(project.id)}
                  class="flex-1 min-w-0 bg-background border border-border rounded px-1 py-0.5 text-xs focus:outline-none focus:ring-1 focus:ring-ring"
                  autofocus
                />
              {:else}
                <span class="truncate font-medium">{project.name}</span>
              {/if}
              {#if getSessions(project.id).some((s) => (s.phase !== "idle" && s.phase !== "closed") || s.compacting)}
                <span class="w-1.5 h-1.5 rounded-full {getSessions(project.id).some((s) => s.phase === "streaming" || s.phase === "executing_tool") ? 'bg-primary' : 'bg-amber-500'} animate-pulse shrink-0"></span>
              {/if}
            </button>

            <div class="relative">
              <button class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-colors" onclick={(e: Event) => { e.stopPropagation(); showMenu = showMenu === project.id ? null : project.id; }}>
                <MoreVertical size={12} />
              </button>
              {#if showMenu === project.id}
                <div class="absolute right-0 top-full mt-1 z-20 w-32 rounded-md border border-border bg-popover shadow-md py-1">
                  <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left" onclick={(e: Event) => { e.stopPropagation(); renamingProjectId = project.id; renameValue = project.name; showMenu = null; }}>
                    <Pencil size={12} /> Rename
                  </button>
                  <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/10 text-left" onclick={(e: Event) => { e.stopPropagation(); deleteProject(project.id); }}>
                    <Trash2 size={12} /> Delete
                  </button>
                </div>
                <div class="fixed inset-0 z-10" onclick={() => showMenu = null}></div>
              {/if}
            </div>

            <button class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-colors" onclick={() => quickCreateSession(project.id)}>
              <Plus size={12} />
            </button>
          </div>

          {#if expanded[project.id]}
            <div class="ml-4 pl-2 border-l border-border/40 space-y-0.5 pb-1">
              {#each getSessions(project.id) as session (session.id)}
                <div class="group w-full flex items-center gap-2 rounded-lg px-3 py-2 cursor-pointer transition-colors {session.id === sessionState.activeSessionId ? 'bg-primary/10 text-foreground' : 'hover:bg-secondary/50 text-muted-foreground hover:text-foreground'}"
                  onclick={() => activateSession(session.id)} role="button" tabindex="0"
                  onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter' || e.key === ' ') activateSession(session.id); }}
                >
                  {#if renamingSessionId === session.id}
                    <input
                      type="text"
                      bind:value={renameValue}
                      onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter') confirmRenameSession(session.id); if (e.key === 'Escape') renamingSessionId = null; }}
                      onblur={() => confirmRenameSession(session.id)}
                      class="flex-1 min-w-0 bg-background border border-border rounded px-1 py-0.5 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
                      autofocus
                    />
                  {:else}
                    <span class="flex-1 truncate text-sm font-medium" title={session.alias ?? "Untitled"}>
                      {session.alias ?? "Untitled"}
                    </span>
                  {/if}
                  <div class="flex items-center gap-1.5 shrink-0">
                    {#if (session.phase !== "idle" && session.phase !== "closed") || session.compacting}
                      <span class="w-1.5 h-1.5 rounded-full {session.phase === "streaming" || session.phase === "executing_tool" ? 'bg-primary' : 'bg-amber-500'} animate-pulse"></span>
                    {/if}
                    <div class="relative">
                      <button class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-colors opacity-0 group-hover:opacity-100" onclick={(e: Event) => { e.stopPropagation(); showSessionMenu = showSessionMenu === session.id ? null : session.id; }}>
                        <MoreVertical size={12} />
                      </button>
                      {#if showSessionMenu === session.id}
                        <div class="absolute right-0 top-full mt-1 z-20 w-32 rounded-md border border-border bg-popover shadow-md py-1">
                          <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left" onclick={(e: Event) => { e.stopPropagation(); renamingSessionId = session.id; renameValue = session.alias ?? "Untitled"; showSessionMenu = null; }}>
                            <Pencil size={12} /> Rename
                          </button>
                          <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left" onclick={(e: Event) => { e.stopPropagation(); copySessionId(session.id); showSessionMenu = null; }}>
                            <Copy size={12} /> Copy ID
                          </button>
                          <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-destructive hover:bg-destructive/10 text-left" onclick={(e: Event) => { e.stopPropagation(); deleteSession(session.id); showSessionMenu = null; }}>
                            <Trash2 size={12} /> Delete
                          </button>
                        </div>
                        <div class="fixed inset-0 z-10" onclick={() => showSessionMenu = null}></div>
                      {/if}
                    </div>
                  </div>
                </div>
              {/each}
              {#if loading[project.id]}
                <div class="px-3 py-1.5 text-xs text-muted-foreground">Loading...</div>
              {/if}
              {#if hasMore(project.id)}
                <button class="w-full text-left px-3 py-1.5 text-xs text-muted-foreground hover:text-foreground transition-colors" onclick={() => loadSessions(project.id)}>
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
