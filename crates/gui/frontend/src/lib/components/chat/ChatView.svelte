<script lang="ts">
  import { sessionState, getActiveSession, closeTab, setActiveSession } from "../../state.svelte";
  import * as api from "../../api";
  import TabBar from "../layout/TabBar.svelte";
  import MessageList from "./MessageList.svelte";
  import ChatInput from "./ChatInput.svelte";
  import FilePreview from "../editor/FilePreview.svelte";
  import FileEditor from "../editor/FileEditor.svelte";
  import { Plus, FolderOpen, Shield, AlertTriangle, Skull } from "lucide-svelte";

  const activeSession = $derived(getActiveSession());

  let projectPath = $state("");
  let permissionLevel = $state("safe");
  let creating = $state(false);

  // Load default cwd on mount when no session active
  $effect(() => {
    if (!sessionState.activeSessionId && projectPath === "") {
      api.getCwd().then(p => { projectPath = p; }).catch(() => { projectPath = ""; });
    }
  });

  async function handleCreate() {
    if (!projectPath.trim() || creating) return;
    creating = true;
    try {
      const id = await api.createSession(projectPath.trim(), permissionLevel);
      // Load sessions to refresh list
      const list = await api.listSessions();
      for (const s of list) {
        if (!sessionState.sessions.find(sess => sess.id === s.id)) {
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
      // Activate new session
      setActiveSession(id);
      await api.subscribe(id);
      const raw = await api.getMessages(id);
      const session = sessionState.sessions.find(s => s.id === id);
      if (session) {
        session.messages = raw.map((m: any) => {
          const role = m.role === "User" || m.role === "user" ? "user" : "assistant";
          let content = "";
          if (Array.isArray(m.content)) {
            content = m.content.map((b: any) => {
              if (typeof b === "string") return b;
              if (b.Text) return b.Text;
              if (b.text) return b.text;
              return "";
            }).join("");
          } else if (typeof m.content === "string") {
            content = m.content;
          }
          return { id: crypto.randomUUID(), role, content, thinking: null };
        });
      }
    } catch (e: any) {
      console.error("Failed to create session:", e?.message ?? e);
      alert("Failed to create session: " + (e?.message ?? "Unknown error"));
    } finally {
      creating = false;
    }
  }

  function switchTab(id: string) {
    if (!activeSession) return;
    activeSession.activeTabId = id;
  }

  function handleCloseTab(id: string) {
    if (!activeSession) return;
    closeTab(activeSession, id);
  }

  function levelLabel(level: string): string {
    switch (level) {
      case "safe": return "Safe";
      case "caution": return "Caution";
      case "dangerous": return "Dangerous";
      default: return level;
    }
  }

  function levelDescription(level: string): string {
    switch (level) {
      case "safe": return "All tools require approval";
      case "caution": return "Safe tools auto-approved";
      case "dangerous": return "Most tools auto-approved";
      default: return "";
    }
  }

  function levelIcon(level: string) {
    switch (level) {
      case "safe": return Shield;
      case "caution": return AlertTriangle;
      case "dangerous": return Skull;
      default: return Shield;
    }
  }

  function levelColor(level: string): string {
    switch (level) {
      case "safe": return "text-green-600 border-green-600 bg-green-600/10";
      case "caution": return "text-amber-600 border-amber-600 bg-amber-600/10";
      case "dangerous": return "text-red-600 border-red-600 bg-red-600/10";
      default: return "";
    }
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
      handleCreate();
    }
  }
</script>

<div class="flex flex-col h-full">
  <!-- Header -->
  <div class="flex items-center justify-between px-4 py-2 border-b border-border">
    <div class="flex items-center gap-2">
      {#if activeSession}
        <span class="font-medium truncate">{activeSession.alias ?? activeSession.id.slice(0, 8)}</span>
        <span class="text-xs text-muted-foreground">{activeSession.projectPath}</span>
      {:else}
        <span class="text-muted-foreground">No active session</span>
      {/if}
    </div>
    {#if activeSession?.streaming}
      <div class="flex items-center gap-1.5 text-xs text-primary">
        <span class="w-1.5 h-1.5 rounded-full bg-primary animate-pulse"></span>
        Streaming...
      </div>
    {/if}
  </div>

  <!-- Tabs -->
  {#if activeSession}
    <TabBar
      tabs={activeSession.tabs}
      activeTabId={activeSession.activeTabId}
      onSwitch={switchTab}
      onClose={handleCloseTab}
    />
  {/if}

  <!-- Content -->
  <div class="flex-1 overflow-hidden">
    {#if !sessionState.activeSessionId}
      <!-- Centered create session screen -->
      <div class="flex items-center justify-center h-full">
        <div class="w-full max-w-lg px-6">
          <div class="text-center mb-8">
            <h1 class="text-3xl font-bold mb-2">Yomi</h1>
            <p class="text-muted-foreground">Create a new session to start coding</p>
          </div>

          <div class="space-y-4">
            <!-- Project path input -->
            <div class="space-y-1.5">
              <label class="text-sm font-medium">Project Path</label>
              <div class="relative">
                <FolderOpen class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
                <input
                  type="text"
                  bind:value={projectPath}
                  onkeydown={onKeydown}
                  placeholder="Enter project path..."
                  class="w-full pl-9 pr-3 py-2.5 rounded-lg border border-border bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring focus:border-ring"
                />
              </div>
            </div>

            <!-- Permission level -->
            <div class="space-y-1.5">
              <label class="text-sm font-medium">Permission Level</label>
              <div class="grid grid-cols-3 gap-2">
                {#each ["safe", "caution", "dangerous"] as level}
                  {@const Icon = levelIcon(level)}
                  <button
                    type="button"
                    onclick={() => permissionLevel = level}
                    class="flex flex-col items-center gap-1.5 p-3 rounded-lg border text-sm transition-all {permissionLevel === level ? levelColor(level) : 'border-border hover:border-muted-foreground text-muted-foreground'}"
                  >
                    <Icon class="w-5 h-5" />
                    <span class="font-medium">{levelLabel(level)}</span>
                    <span class="text-xs opacity-70">{levelDescription(level)}</span>
                  </button>
                {/each}
              </div>
            </div>

            <!-- Create button -->
            <button
              type="button"
              onclick={handleCreate}
              disabled={creating || !projectPath.trim()}
              class="w-full flex items-center justify-center gap-2 py-2.5 px-4 rounded-lg bg-primary text-primary-foreground font-medium text-sm transition-all hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {#if creating}
                <span class="w-4 h-4 border-2 border-primary-foreground/30 border-t-primary-foreground rounded-full animate-spin"></span>
                Creating...
              {:else}
                <Plus class="w-4 h-4" />
                Create Session
              {/if}
            </button>

            <p class="text-xs text-muted-foreground text-center">
              Press Ctrl+Enter to create
            </p>
          </div>
        </div>
      </div>
    {:else if activeSession?.activeTabId === "chat"}
      <div class="flex flex-col h-full">
        <MessageList />
        <ChatInput />
      </div>
    {:else if activeSession}
      {@const activeTab = activeSession.tabs.find(t => t.id === activeSession.activeTabId)}
      {#if activeTab?.type === "preview" && activeTab.entry}
        <FilePreview
          entry={activeTab.entry}
          onEdit={(e) => { /* TODO: open edit tab */ }}
          onAskAI={(path) => { /* TODO: send to chat */ }}
        />
      {:else if activeTab?.type === "edit" && activeTab.entry}
        <FileEditor entry={activeTab.entry} />
      {/if}
    {:else}
      <div class="flex items-center justify-center h-full text-muted-foreground">
        Loading session...
      </div>
    {/if}
  </div>
</div>
