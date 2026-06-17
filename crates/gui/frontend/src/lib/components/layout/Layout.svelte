<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import { projectState, appState, getActiveSession } from "../../state.svelte";
  import ProjectSidebar from "./ProjectSidebar.svelte";
  import ChatView from "../chat/ChatView.svelte";
  import ActivityBar from "./ActivityBar.svelte";
  import UsagePanel from "./UsagePanel.svelte";
  import AutomationPanel from "../automation/AutomationPanel.svelte";
  import ConfigEditor from "./ConfigEditor.svelte";
  import RightPanel from "./RightPanel.svelte";
  import StatusBar from "./StatusBar.svelte";

  let mobileSidebarOpen = $state(false);
  let leftSidebarWidth = $state(256);
  let rightSidebarWidth = $state(560);
  let hasInitializedWidth = $state(false);
  let isDraggingLeft = $state(false);
  let isDraggingRight = $state(false);

  onMount(() => {
    if (!hasInitializedWidth) {
      rightSidebarWidth = Math.max(300, window.innerWidth / 2);
      hasInitializedWidth = true;
    }
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

  const activeSession = $derived(getActiveSession());

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
    const startWidth = appState.sidebarCollapsed ? 64 : leftSidebarWidth;
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

  function startDragRight(e: MouseEvent) {
    e.preventDefault();
    const startX = e.clientX;
    const startWidth = rightSidebarWidth;
    isDraggingRight = true;

    function onMove(ev: MouseEvent) {
      const delta = startX - ev.clientX;
      rightSidebarWidth = Math.max(300, startWidth + delta);
    }
    function onUp() {
      isDraggingRight = false;
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
          class="hidden lg:flex flex-col border-r border-border shrink-0 relative overflow-hidden
                 {isDraggingLeft
            ? ''
            : 'transition-[width] duration-300 ease-out'}"
          style="width: {appState.sidebarCollapsed ? 64 : leftSidebarWidth}px"
        >
          <ProjectSidebar collapsed={appState.sidebarCollapsed} />
          {#if !appState.sidebarCollapsed}
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
          class="fixed inset-0 z-40 bg-black/20 backdrop-blur-sm lg:hidden
                 transition-opacity duration-200
                 {mobileSidebarOpen
            ? 'opacity-100 pointer-events-auto'
            : 'opacity-0 pointer-events-none'}"
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
          leftPanelCollapsed={appState.sidebarCollapsed}
          onToggleRightPanel={() =>
            (appState.rightPanelCollapsed = !appState.rightPanelCollapsed)}
          onToggleLeftPanel={handleToggleLeft}
        />

        <!-- Desktop right panel -->
        <aside
          class="hidden lg:flex flex-col border-l border-border shrink-0 relative overflow-hidden
                 {isDraggingRight
            ? ''
            : 'transition-[width] duration-300 ease-out'}
                 {appState.rightPanelCollapsed ? 'border-l-0' : ''}"
          style="width: {appState.rightPanelCollapsed
            ? 0
            : rightSidebarWidth}px"
        >
          {#if !appState.rightPanelCollapsed}
            <div
              class="absolute left-0 top-0 bottom-0 w-[2px] cursor-col-resize hover:bg-primary/50 z-10"
              onmousedown={startDragRight}
              role="separator"
              aria-label="Resize right panel"
            ></div>
          {/if}
          <RightPanel session={activeSession} />
        </aside>

        <!-- Mobile right panel overlay -->
        <div
          class="fixed inset-0 z-40 bg-black/20 backdrop-blur-sm lg:hidden
                 transition-opacity duration-200
                 {appState.rightPanelCollapsed
            ? 'opacity-0 pointer-events-none'
            : 'opacity-100 pointer-events-auto'}"
          onclick={() => (appState.rightPanelCollapsed = true)}
          role="presentation"
        ></div>
        <div
          class="fixed right-0 top-0 bottom-0 z-50 w-[90vw] border-l border-border bg-background shadow-xl
                 transition-transform duration-300 ease-out lg:hidden flex flex-col
                 {appState.rightPanelCollapsed
            ? 'translate-x-full'
            : 'translate-x-0'}"
          style="max-width: 500px;"
        >
          <RightPanel
            session={activeSession}
            onClose={() => (appState.rightPanelCollapsed = true)}
          />
        </div>
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
