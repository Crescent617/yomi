<script lang="ts">
  import { sessionState, getActiveSession, closeTab } from "../../state.svelte";
  import TabBar from "../layout/TabBar.svelte";
  import MessageList from "./MessageList.svelte";
  import ChatInput from "./ChatInput.svelte";
  import FilePreview from "../editor/FilePreview.svelte";
  import FileEditor from "../editor/FileEditor.svelte";

  const activeSession = $derived(getActiveSession());

  function switchTab(id: string) {
    if (!activeSession) return;
    activeSession.activeTabId = id;
  }

  function handleCloseTab(id: string) {
    if (!activeSession) return;
    closeTab(activeSession, id);
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
      <div class="flex items-center justify-center h-full text-muted-foreground">
        Select or create a session to start
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
