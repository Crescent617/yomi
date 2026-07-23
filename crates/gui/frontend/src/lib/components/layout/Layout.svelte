<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import { projectState, appState } from "../../state.svelte";
  import {
    guiPreferences,
    scheduleGuiPreferencesSave,
    snapshotGuiPreferences,
  } from "../../settings.svelte";
  import ProjectSidebar from "./ProjectSidebar.svelte";
  import ChatView from "../chat/ChatView.svelte";
  import ActivityBar from "./ActivityBar.svelte";
  import UsagePanel from "./UsagePanel.svelte";
  import DebugPanel from "./DebugPanel.svelte";
  import AutomationPanel from "../automation/AutomationPanel.svelte";
  import FavoritesPanel from "./FavoritesPanel.svelte";
  import ConfigPanel from "./ConfigPanel.svelte";
  import StatusBar from "./StatusBar.svelte";
  import ShareCardDialog from "../chat/ShareCardDialog.svelte";
  import { startClock } from "../../clock.svelte";
  import { loadFavorites } from "../../favorites.svelte";

  let mobileSidebarOpen = $state(false);
  let isDraggingLeft = $state(false);

  onMount(() => {
    startClock();
    loadProjects();
    void loadFavorites();
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
    guiPreferences.layout.sidebarCollapsed =
      !guiPreferences.layout.sidebarCollapsed;
    const next = snapshotGuiPreferences();
    scheduleGuiPreferencesSave(next);
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
    const startWidth = guiPreferences.layout.sidebarWidth;
    isDraggingLeft = true;
    if (guiPreferences.layout.sidebarCollapsed) {
      guiPreferences.layout.sidebarCollapsed = false;
    }

    function onMove(ev: MouseEvent) {
      const delta = ev.clientX - startX;
      guiPreferences.layout.sidebarWidth = Math.max(
        160,
        Math.min(400, startWidth + delta),
      );
    }
    function onUp() {
      isDraggingLeft = false;
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      scheduleGuiPreferencesSave(snapshotGuiPreferences());
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
                 {guiPreferences.layout.sidebarCollapsed ? '' : 'border-r'}
                 {isDraggingLeft
            ? ''
            : 'transition-[width] duration-200 ease-out'}"
          style="width: {guiPreferences.layout.sidebarCollapsed
            ? 0
            : guiPreferences.layout.sidebarWidth}px"
          aria-hidden={guiPreferences.layout.sidebarCollapsed}
        >
          {#if !guiPreferences.layout.sidebarCollapsed}
            <ProjectSidebar collapsed={false} />
            <button
              type="button"
              class="absolute right-0 top-0 bottom-0 w-[2px] cursor-col-resize hover:bg-primary/50 z-10"
              onmousedown={startDragLeft}
              aria-label="Resize sidebar"
            ></button>
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
          leftPanelCollapsed={guiPreferences.layout.sidebarCollapsed}
          onToggleLeftPanel={handleToggleLeft}
        />
      {:else if appState.activePanel === "usage"}
        <UsagePanel onToggleLeftPanel={toggleMobileSidebar} />
      {:else if appState.activePanel === "favorites"}
        <FavoritesPanel onToggleLeftPanel={toggleMobileSidebar} />
      {:else if appState.activePanel === "automation"}
        <AutomationPanel onToggleLeftPanel={toggleMobileSidebar} />
      {:else if appState.activePanel === "debug"}
        <DebugPanel onToggleLeftPanel={toggleMobileSidebar} />
      {:else if appState.activePanel === "config"}
        <ConfigPanel onToggleLeftPanel={toggleMobileSidebar} />
      {/if}
    </div>
    <StatusBar />
  </div>
</div>

<ShareCardDialog />
