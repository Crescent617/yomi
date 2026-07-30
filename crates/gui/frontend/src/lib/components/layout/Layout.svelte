<script lang="ts">
  import { onMount, tick, untrack } from "svelte";
  import * as api from "../../api";
  import { projectState, appState, sessionState } from "../../state.svelte";
  import {
    defaultGuiPreferences,
    guiPreferences,
    scheduleGuiPreferencesSave,
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
  let isNarrow = $state(false);
  let isDraggingLeft = $state(false);
  let drawerEl = $state<HTMLElement | null>(null);
  // Opener to hand focus back to when the drawer closes (modal cycle).
  let drawerRestoreFocus: HTMLElement | null = null;

  const SIDEBAR_MIN_WIDTH = 160;
  const SIDEBAR_MAX_WIDTH = 400;

  function clampSidebarWidth(width: number): number {
    return Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width));
  }

  onMount(() => {
    startClock();
    loadProjects();
    void loadFavorites();
    // Mirror the Tailwind `lg` breakpoint so toggle behavior and icon state
    // follow the same boundary as the CSS that hides the activity rail.
    const narrowQuery = window.matchMedia("(max-width: 1023.98px)");
    isNarrow = narrowQuery.matches;
    const onNarrowChange = (e: MediaQueryListEvent) => {
      isNarrow = e.matches;
    };
    narrowQuery.addEventListener("change", onNarrowChange);
    return () => narrowQuery.removeEventListener("change", onNarrowChange);
  });

  // The mobile drawer is a transient navigation surface: any navigation
  // (panel switch, session activation) or a resize to desktop dismisses it.
  $effect(() => {
    void appState.activePanel;
    void sessionState.activeSessionId;
    void isNarrow;
    // Untracked: closeMobileSidebar reads mobileSidebarOpen (its no-op
    // guard) — as a dependency it would re-fire this effect the moment
    // the drawer opens, instantly closing it again.
    untrack(closeMobileSidebar);
  });

  $effect(() => {
    if (!mobileSidebarOpen) return;
    const onKeydown = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeMobileSidebar();
    };
    window.addEventListener("keydown", onKeydown);
    return () => window.removeEventListener("keydown", onKeydown);
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

  function openMobileSidebar() {
    drawerRestoreFocus =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    mobileSidebarOpen = true;
    // Move focus into the drawer so keyboard users land on its controls.
    void tick().then(() => drawerEl?.querySelector<HTMLElement>("button")?.focus());
  }

  function closeMobileSidebar() {
    if (!mobileSidebarOpen) return;
    mobileSidebarOpen = false;
    // Defer past Svelte's batch: focusing into a still-inert subtree is a
    // spec no-op, so the restore must wait for inert to come off.
    void tick().then(() => {
      if (drawerRestoreFocus?.isConnected) drawerRestoreFocus.focus();
      drawerRestoreFocus = null;
    });
  }

  function toggleMobileSidebar() {
    if (mobileSidebarOpen) {
      closeMobileSidebar();
    } else {
      openMobileSidebar();
    }
  }

  function toggleLeftSidebar() {
    guiPreferences.layout.sidebarCollapsed =
      !guiPreferences.layout.sidebarCollapsed;
    scheduleGuiPreferencesSave();
  }

  function handleToggleLeft() {
    if (isNarrow) {
      toggleMobileSidebar();
    } else {
      toggleLeftSidebar();
    }
  }

  function resetSidebarWidth() {
    guiPreferences.layout.sidebarWidth =
      defaultGuiPreferences.layout.sidebarWidth;
    scheduleGuiPreferencesSave();
  }

  function handleResizeKeydown(e: KeyboardEvent) {
    if (e.key !== "ArrowLeft" && e.key !== "ArrowRight") return;
    e.preventDefault();
    const delta = e.key === "ArrowRight" ? 16 : -16;
    guiPreferences.layout.sidebarWidth = clampSidebarWidth(
      guiPreferences.layout.sidebarWidth + delta,
    );
    scheduleGuiPreferencesSave();
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
      guiPreferences.layout.sidebarWidth = clampSidebarWidth(
        startWidth + delta,
      );
    }
    function onUp() {
      isDraggingLeft = false;
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      scheduleGuiPreferencesSave();
    }
    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
  }
</script>

<div class="fixed inset-0 flex bg-background text-foreground overflow-hidden">
  <!-- Desktop ActivityBar — always visible on lg+ -->
  <div class="hidden lg:flex shrink-0" inert={mobileSidebarOpen}>
    <ActivityBar />
  </div>

  <div
    class="flex-1 flex flex-col min-h-0 overflow-hidden relative"
    inert={mobileSidebarOpen}
  >
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
              class="group absolute right-0 top-0 bottom-0 z-10 w-1.5 cursor-col-resize focus-visible:outline-none"
              onmousedown={startDragLeft}
              ondblclick={resetSidebarWidth}
              onkeydown={handleResizeKeydown}
              aria-label="Resize sidebar"
              title="Drag to resize, double-click to reset, arrow keys to adjust"
            >
              <span
                class="absolute right-0 top-0 bottom-0 w-[2px] transition-colors group-hover:bg-primary/50 group-focus-visible:bg-primary/50 {isDraggingLeft
                  ? 'bg-primary/50'
                  : ''}"
              ></span>
            </button>
          {/if}
        </aside>

        <ChatView
          leftPanelCollapsed={isNarrow
            ? !mobileSidebarOpen
            : guiPreferences.layout.sidebarCollapsed}
          leftPanelAttention={!isNarrow &&
            guiPreferences.layout.sidebarCollapsed}
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

  <!-- Mobile navigation drawer (< lg) — shared by all panels: the activity
       rail plus, on the chat panel, the session sidebar. Always rendered so
       the slide/fade transitions can play; inert while closed. -->
  <div
    class="fixed inset-0 z-40 bg-overlay backdrop-blur-sm lg:hidden
           transition-opacity duration-200
           {mobileSidebarOpen
      ? 'opacity-100 pointer-events-auto'
      : 'opacity-0 pointer-events-none'}"
    onclick={closeMobileSidebar}
    role="presentation"
  ></div>
  <div
    class="fixed left-0 top-0 bottom-0 z-50 flex border-r border-border bg-card/70 shadow-xl
           transition-transform duration-300 ease-out lg:hidden
           {mobileSidebarOpen ? 'translate-x-0' : '-translate-x-full'}"
    style="max-width: 85vw;"
    role="dialog"
    aria-modal="true"
    aria-label="Navigation"
    inert={!mobileSidebarOpen}
    bind:this={drawerEl}
  >
    <ActivityBar onClose={closeMobileSidebar} onNavigate={closeMobileSidebar} />
    {#if appState.activePanel === "chat"}
      <div class="flex-1 flex flex-col min-w-0 overflow-hidden w-64">
        <ProjectSidebar collapsed={false} />
      </div>
    {/if}
  </div>
</div>

<ShareCardDialog />
