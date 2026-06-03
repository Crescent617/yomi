<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import { projectState, appState, getActiveSession } from "../../state.svelte";
  import ProjectSidebar from "./ProjectSidebar.svelte";
  import ChatView from "../chat/ChatView.svelte";
  import ActivityBar from "./ActivityBar.svelte";
  import UsagePanel from "./UsagePanel.svelte";
  import ConfigEditor from "./ConfigEditor.svelte";
  import RightPanel from "./RightPanel.svelte";

  let mobileSidebarOpen = $state(false);

  onMount(() => {
    loadProjects();
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
    } catch (e) {
      console.error("Failed to load projects:", e);
    }
  }

  const activeSession = $derived(getActiveSession());

  function closeMobileSidebar() {
    mobileSidebarOpen = false;
  }

  function toggleMobileSidebar() {
    mobileSidebarOpen = !mobileSidebarOpen;
  }
</script>

<div class="fixed inset-0 flex bg-background text-foreground overflow-hidden">
  <!-- Desktop ActivityBar — always visible on lg+ -->
  <div class="hidden lg:flex shrink-0">
    <ActivityBar />
  </div>

  <div class="flex-1 flex min-h-0 overflow-hidden relative">
    {#if appState.activePanel === "chat"}
      <!-- Desktop inline sidebar -->
      <aside
        class="hidden lg:flex flex-col border-r border-border shrink-0 transition-[width] duration-300 ease-out
               {appState.sidebarCollapsed ? 'w-16' : 'w-64'}"
      >
        <ProjectSidebar collapsed={appState.sidebarCollapsed} />
      </aside>

      <!-- Mobile overlay sidebar -->
      <!-- Backdrop: always rendered for smooth fade-out -->
      <div
        class="fixed inset-0 z-40 bg-black/20 backdrop-blur-sm lg:hidden
               transition-opacity duration-200
               {mobileSidebarOpen ? 'opacity-100 pointer-events-auto' : 'opacity-0 pointer-events-none'}"
        onclick={closeMobileSidebar}
        role="presentation"
      ></div>

      <!-- Mobile drawer (always rendered for animation, controlled by transform) -->
      <div
        class="fixed left-0 top-0 bottom-0 z-50 flex border-r border-border bg-background shadow-xl
               transition-transform duration-300 ease-out lg:hidden
               {mobileSidebarOpen ? 'translate-x-0' : '-translate-x-full'}"
        style="max-width: 85vw;"
      >
        <ActivityBar />
        <div class="flex-1 flex flex-col min-w-0 overflow-hidden w-64">
          <ProjectSidebar collapsed={false} />
        </div>
      </div>

      <ChatView
        rightPanelCollapsed={appState.rightPanelCollapsed}
        onToggleRightPanel={() => appState.rightPanelCollapsed = !appState.rightPanelCollapsed}
        onToggleLeftPanel={toggleMobileSidebar}
      />

      {#if !appState.rightPanelCollapsed}
        <aside
          class="hidden lg:flex flex-col border-l border-border shrink-0 w-72 transition-all duration-300 ease-out"
        >
          <RightPanel session={activeSession} />
        </aside>
      {/if}
    {:else if appState.activePanel === "usage"}
      <UsagePanel onToggleLeftPanel={toggleMobileSidebar} />
    {:else if appState.activePanel === "config"}
      <ConfigEditor onToggleLeftPanel={toggleMobileSidebar} />
    {/if}
  </div>
</div>
