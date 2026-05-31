<script lang="ts">
  import { Plus, ChevronDown, ChevronRight, Folder } from "lucide-svelte";
  import * as api from "../../api";
  import {
    sessionState,
    setActiveSession,
    loadSessionMessages,
    getSession,
  } from "../../state.svelte";

  let { collapsed = false }: { collapsed?: boolean } = $props();

  // ===== workspace grouping =====
  const groups = $derived.by(() => {
    const map = new Map<string, typeof sessionState.sessions>();
    for (const s of sessionState.sessions) {
      if (!s.projectPath) continue; // skip sessions without working dir
      const key = s.projectPath;
      const list = map.get(key) ?? [];
      list.push(s);
      map.set(key, list);
    }
    const entries = Array.from(map.entries());
    // active workspace first, then alphabetical
    const active = getSession(sessionState.activeSessionId ?? "");
    const activePath = active?.projectPath || "";
    entries.sort((a, b) => {
      const aIsActive = a[0] === activePath;
      const bIsActive = b[0] === activePath;
      if (aIsActive && !bIsActive) return -1;
      if (bIsActive && !aIsActive) return 1;
      return a[0].localeCompare(b[0]);
    });
    return entries;
  });

  // ===== expand/collapse =====
  let expanded = $state(new Set<string>());
  // always include the active workspace
  const allExpanded = $derived.by(() => {
    const active = getSession(sessionState.activeSessionId ?? "");
    const path = active?.projectPath || "";
    return new Set([...expanded, path].filter(Boolean));
  });

  function toggleGroup(path: string) {
    const next = new Set(expanded);
    if (next.has(path)) next.delete(path);
    else next.add(path);
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

  // ===== create / navigate =====
  function goToCreatePage() {
    setActiveSession(null);
  }

  async function quickCreateSession(projectPath: string) {
    try {
      const id = await api.createSession(projectPath, "safe");
      // refresh list from backend
      const list = await api.listSessions();
      for (const s of list) {
        if (!sessionState.sessions.find((sess) => sess.id === s.id)) {
          sessionState.sessions.push({
            id: s.id,
            projectPath: s.projectPath ?? "",
            alias: s.title,
            messages: [],
            streaming: false,
            unread: 0,
            checkpoints: [],
            tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
            activeTabId: "chat",
          });
        }
      }
      await activateSession(id);
    } catch (e: any) {
      console.error("Failed to create session:", e?.message ?? e);
    }
  }

  // ===== activate =====
  async function activateSession(id: string) {
    const prevId = sessionState.activeSessionId;
    try {
      if (prevId && prevId !== id) {
        await api.unsubscribe(prevId);
      }
      await api.subscribe(id);
      setActiveSession(id);
      const raw = await api.getMessages(id);
      const session = getSession(id);
      if (session) {
        loadSessionMessages(id, raw);
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
    <!-- global new-session button -->
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
      {#each groups as [projectPath, sessions], groupIdx (projectPath)}
        <div class="flex flex-col items-center gap-1.5">
          {#each sessions as session (session.id)}
            {@const isActive = session.id === sessionState.activeSessionId}
            <div class="relative">
              <button
                class="w-8 h-8 rounded-lg flex items-center justify-center text-[10px] font-bold
                       transition-colors
                       {isActive
                         ? 'bg-primary text-primary-foreground shadow-sm'
                         : 'bg-secondary text-secondary-foreground hover:bg-secondary/80'}"
                onclick={() => activateSession(session.id)}
                title="{session.alias ?? formatShortId(session.id)} — {projectPath}"
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
      {#each groups as [projectPath, sessions] (projectPath)}
        {@const isActiveWorkspace =
          projectPath === getSession(sessionState.activeSessionId ?? "")?.projectPath}
        {@const isExpanded = allExpanded.has(projectPath)}

        <div class="rounded-md overflow-hidden mb-0.5">
          <!-- workspace header -->
          <div
            class="flex items-center gap-1.5 w-full rounded-md px-2 py-1.5 text-xs
                   transition-colors select-none
                   {isActiveWorkspace
              ? 'text-foreground bg-secondary/60'
              : 'text-muted-foreground hover:text-foreground hover:bg-secondary/40'}"
          >
            <button
              class="flex items-center gap-1.5 flex-1 min-w-0 text-left"
              onclick={() => toggleGroup(projectPath)}
              title={projectPath}
            >
              {#if isExpanded}
                <ChevronDown size={13} class="shrink-0" />
              {:else}
                <ChevronRight size={13} class="shrink-0" />
              {/if}
              <Folder size={13} class="shrink-0 opacity-70" />
              <span class="truncate font-medium">{formatPath(projectPath)}</span>
            </button>
            <button
              class="shrink-0 p-0.5 rounded hover:bg-secondary/80 transition-colors"
              onclick={() => quickCreateSession(projectPath)}
              title="Create session in this workspace"
            >
              <Plus size={12} />
            </button>
          </div>

          {#if isExpanded}
            <div class="ml-4 pl-2 border-l border-border/40 space-y-0.5 pb-1">
              {#each sessions as session (session.id)}
                <button
                  class="group flex items-center gap-2 rounded-lg px-3 py-2 text-left
                         transition-colors border-l-4 {session.id === sessionState.activeSessionId
                    ? 'bg-primary/10 border-primary'
                    : 'hover:bg-secondary/50 border-transparent'}"
                  onclick={() => activateSession(session.id)}
                >
                  <span class="flex-1 truncate text-sm font-medium">
                    {session.alias ?? formatShortId(session.id)}
                  </span>
                  <div class="flex items-center gap-1.5 shrink-0">
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
                  </div>
                </button>
              {/each}
            </div>
          {/if}
        </div>
      {/each}
    {/if}
  </div>
</div>
