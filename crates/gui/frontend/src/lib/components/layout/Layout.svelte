<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import { projectState, appState } from "../../state.svelte";
  import ProjectSidebar from "./ProjectSidebar.svelte";
  import ChatView from "../chat/ChatView.svelte";
  import ActivityBar from "./ActivityBar.svelte";
  import UsagePanel from "./UsagePanel.svelte";
  import AutomationPanel from "../automation/AutomationPanel.svelte";
  import ConfigEditor from "./ConfigEditor.svelte";
  import StatusBar from "./StatusBar.svelte";
  import { startClock } from "../../clock.svelte";

  let mobileSidebarOpen = $state(false);
  let leftSidebarWidth = $state(256);
  let isDraggingLeft = $state(false);

  onMount(() => {
    startClock();
    loadProjects();
  });

  async function loadProjects() {
    try {
      const list = await api.listProjects();
      projectState.projects = list.map((p) => ({
        id: p.id,
        name: p.name,
        dir: p.dir,
        created_at: p.created_at,
        updated_at: p.updated_at,
      }));
    } catch (e) {
      console.error("Failed to load projects:", e);
    }
  }

  function closeMobileSidebar() {
    mobileSidebarOpen = false;
  }

  function toggleMobileSidebar() {
    mobileSidebarOpen = !mobileSidebarOpen;
  }

  function toggleLeftSidebar() {
    appState.sidebarCollapsed = !appState.sidebarCollapsed;
  }

  function handleToggleLeft() {
    if (window.innerWidth < 1024) {
      toggleMobileSidebar();
    } else {
      toggleLeftSidebar();
    }
  }

  function startDragLeft(e: MouseEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = leftSidebarWidth;
    isDraggingLeft = true;
    if (appState.sidebarCollapsed) {
      appState.sidebarCollapsed = false;
    }

    function onMove(ev: MouseEvent) {
      const delta = ev.clientX - startX;
      leftSidebarWidth = Math.max(160, Math.min(400, startWidth + delta));
    }
    function onUp() {
      isDraggingLeft = false;
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
    }
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }
</script>

<div class="fixed inset-0 flex bg-background text-foreground overflow-hidden">
  <!-- Desktop ActivityBar — always visible on lg+ -->
  <div class="hidden lg:flex shrink-0">
    <ActivityBar />
  </div>

  <div class="flex-1 flex flex-col min-h-0 overflow-hidden relative">
    <div class="flex-1 flex min-h-0 overflow-hidden relative">
      {#if appState.activePanel === "chat"}
        <!-- Desktop inline sidebar -->
        <aside
          class="hidden lg:flex flex-col border-border bg-card/70 shrink-0 relative overflow-hidden
                 {appState.sidebarCollapsed ? '' : 'border-r'}
                 {isDraggingLeft
            ? ''
            : 'transition-[width] duration-200 ease-out'}"
          style="width: {appState.sidebarCollapsed ? 0 : leftSidebarWidth}px"
          aria-hidden={appState.sidebarCollapsed}
        >
          {#if !appState.sidebarCollapsed}
            <ProjectSidebar collapsed={false} />
            <div
              class="absolute right-0 top-0 bottom-0 w-[2px] cursor-col-resize hover:bg-primary/50 z-10"
              onmousedown={startDragLeft}
              role="separator"
              aria-label="Resize sidebar"
            ></div>
          {/if}
        </aside>

        <!-- Mobile overlay sidebar -->
        <!-- Backdrop: always rendered for smooth fade-out -->
        <div
          class="fixed inset-0 z-40 bg-overlay backdrop-blur-sm lg:hidden
                 transition-opacity duration-200
                 {mobileSidebarOpen
            ? 'opacity-100 pointer-events-auto'
            : 'opacity-0 pointer-events-none'}"
          onclick={closeMobileSidebar}
          role="presentation"
        ></div>

        <!-- Mobile drawer (always rendered for animation, controlled by transform) -->
        <div
          class="fixed left-0 top-0 bottom-0 z-50 flex border-r border-border bg-card/70 shadow-xl
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
          leftPanelCollapsed={appState.sidebarCollapsed}
          onToggleLeftPanel={handleToggleLeft}
        />
      {:else if appState.activePanel === "usage"}
        <UsagePanel onToggleLeftPanel={toggleMobileSidebar} />
      {:else if appState.activePanel === "automation"}
        <AutomationPanel onToggleLeftPanel={toggleMobileSidebar} />
      {:else if appState.activePanel === "config"}
        <ConfigEditor onToggleLeftPanel={toggleMobileSidebar} />
      {/if}
    </div>
    <StatusBar />
  </div>
</div>
