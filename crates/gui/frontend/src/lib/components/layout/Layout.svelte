<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import { sessionState, projectState, sessionCursors, appState } from "../../state.svelte";
  import SessionBand from "./SessionBand.svelte";
  import ChatView from "../chat/ChatView.svelte";

  let isDesktop = $state(false);

  onMount(() => {
    isDesktop = window.innerWidth >= 1024;
    const onResize = () => {
      isDesktop = window.innerWidth >= 1024;
    };
    window.addEventListener("resize", onResize);
    loadProjects();
    return () => window.removeEventListener("resize", onResize);
  });

  async function loadProjects() {
    try {
      const list = await api.listProjects();
      projectState.projects = list.map((p) => ({
        id: p.id,
        name: p.name,
        dir: p.dir,
        createdAt: p.createdAt,
        updatedAt: p.updatedAt,
      }));
      // Load sessions for each project
      for (const project of projectState.projects) {
        await loadSessionsForProject(project.id);
      }
    } catch (e) {
      console.error("Failed to load projects:", e);
    }
  }

  async function loadSessionsForProject(projectId: string) {
    try {
      const result = await api.listSessions(projectId, undefined, 20);
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
          });
        }
      }
      if (result.hasMore && result.sessions.length > 0) {
        const last = result.sessions[result.sessions.length - 1];
        sessionCursors.set(projectId, last.endedAt ?? last.createdAt);
      } else {
        sessionCursors.set(projectId, null);
      }
    } catch (e) {
      console.error("Failed to load sessions for project:", projectId, e);
    }
  }
</script>

<div class="h-screen w-screen flex bg-background text-foreground overflow-hidden">
  {#if isDesktop}
    <aside
      class="flex flex-col border-r border-border transition-all {appState.sidebarCollapsed
        ? 'w-16'
        : 'w-64'} h-full"
    >
      <div class="p-3 border-b border-border shrink-0">
        <h1 class="font-bold text-lg truncate">Yomi</h1>
      </div>
      <div class="flex-1 min-h-0 overflow-hidden flex flex-col">
        <SessionBand collapsed={appState.sidebarCollapsed} />
      </div>
    </aside>
  {/if}

  <main class="flex-1 flex flex-col min-w-0">
    <ChatView />
  </main>
</div>
