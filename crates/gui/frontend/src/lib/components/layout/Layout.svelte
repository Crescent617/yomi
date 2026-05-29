<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../api";
  import { sessionState, setActiveSession, appState, loadSessionMessages } from "../../state.svelte";
  import SessionBand from "./SessionBand.svelte";
  import ExplorerTree from "../explorer/ExplorerTree.svelte";
  import ChatView from "../chat/ChatView.svelte";

  let isDesktop = $state(false);

  onMount(() => {
    isDesktop = window.innerWidth >= 1024;
    const onResize = () => {
      isDesktop = window.innerWidth >= 1024;
    };
    window.addEventListener("resize", onResize);
    loadSessions();
    return () => window.removeEventListener("resize", onResize);
  });

  async function loadSessions() {
    try {
      const list = await api.listSessions();
      const remoteIds = new Set(list.map((s) => s.id));

      // Remove sessions deleted on backend
      sessionState.sessions = sessionState.sessions.filter(s => remoteIds.has(s.id));

      // Add or update
      for (const s of list) {
        const idx = sessionState.sessions.findIndex(sess => sess.id === s.id);
        if (idx >= 0) {
          const existing = sessionState.sessions[idx];
          sessionState.sessions[idx] = {
            ...existing,
            projectPath: s.projectPath ?? existing.projectPath ?? "",
            alias: s.title ?? existing.alias,
          };
        } else {
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

      // Auto-activate first session
      if (list.length > 0 && !sessionState.activeSessionId) {
        const first = list[0];
        setActiveSession(first.id);
        try {
          await api.subscribe(first.id);
          const raw = await api.getMessages(first.id);
          const session = sessionState.sessions.find(sess => sess.id === first.id);
          if (session) {
            loadSessionMessages(first.id, raw);
          }
        } catch (e: any) {
          console.error("Failed to auto-activate session:", e?.message ?? e);
        }
      }
    } catch (e) {
      console.error("Failed to load sessions:", e);
    }
  }
</script>

<div class="h-screen w-screen flex bg-background text-foreground overflow-hidden">
  {#if isDesktop}
    <aside
      class="flex flex-col border-r border-border transition-all {appState.sidebarCollapsed
        ? 'w-16'
        : 'w-64'}"
    >
      <div class="p-3 border-b border-border">
        <h1 class="font-bold text-lg truncate">Yomi</h1>
      </div>
      <SessionBand collapsed={appState.sidebarCollapsed} />
      {#if !appState.sidebarCollapsed}
        <ExplorerTree />
      {/if}
    </aside>
  {/if}

  <main class="flex-1 flex flex-col min-w-0">
    <ChatView />
  </main>
</div>
