<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import { sessionState, projectState, sessionCursors, appState, getActiveSession } from "../../state.svelte";
  import SessionBand from "./SessionBand.svelte";
  import ChatView from "../chat/ChatView.svelte";
  import ActivityBar from "./ActivityBar.svelte";
  import UsagePanel from "./UsagePanel.svelte";
  import ConfigEditor from "./ConfigEditor.svelte";
  import RightPanel from "./RightPanel.svelte";

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
            pendingPermissions: [],
            pendingAskUser: null,
            queuedInput: null,
            updatedAt: s.endedAt ?? s.createdAt,
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

  const activeSession = $derived(getActiveSession());
</script>

<div class="fixed inset-0 flex bg-background text-foreground overflow-hidden">
  {#if isDesktop}
    <ActivityBar />
  {/if}

  <div class="flex-1 flex min-h-0 overflow-hidden">
    {#if appState.activePanel === "chat"}
      {#if isDesktop}
        <aside
          class="flex flex-col border-r border-border shrink-0 {appState.sidebarCollapsed
            ? 'w-16'
            : 'w-64'}"
        >
          <SessionBand collapsed={appState.sidebarCollapsed} />
        </aside>
      {/if}

      <!-- ChatView directly as flex item -->
      <ChatView
        rightPanelCollapsed={appState.rightPanelCollapsed}
        onToggleRightPanel={() => appState.rightPanelCollapsed = !appState.rightPanelCollapsed}
      />

      {#if isDesktop && !appState.rightPanelCollapsed}
        <RightPanel session={activeSession} />
      {/if}
    {:else if appState.activePanel === "usage"}
      <UsagePanel />
    {:else if appState.activePanel === "config"}
      <ConfigEditor />
    {/if}
  </div>
</div>
