<script lang="ts">
  import { sessionState, projectState, getActiveSession, closeTab, setActiveSession, showNotification, loadSessionMessages, addUserMessage } from "../../state.svelte";
  import * as api from "../../api";
  import TabBar from "../layout/TabBar.svelte";
  import RightPanel from "../layout/RightPanel.svelte";
  import MessageList from "./MessageList.svelte";
  import ChatInput from "./ChatInput.svelte";
  import FilePreview from "../editor/FilePreview.svelte";
  import FileEditor from "../editor/FileEditor.svelte";
  import InfoBar from "./InfoBar.svelte";
  import PermissionBar from "./PermissionBar.svelte";
  import AskUserBar from "./AskUserBar.svelte";
  import QueuedInputBar from "./QueuedInputBar.svelte";
  import { FolderOpen, Shield, AlertTriangle, Skull, ArrowDown, ChevronDown, Send } from "lucide-svelte";
  import { open } from "@tauri-apps/plugin-dialog";

  const activeSession = $derived(getActiveSession());

  const hasNonChatTabs = $derived(activeSession?.tabs.some(t => t.type !== "chat") ?? false);

  let selectedProjectId = $state<string | "new">("");
  let newProjectPath = $state("");
  let newProjectName = $state("");
  let permissionLevel = $state("safe");
  let listRef: any = $state(null);
  let isNearBottom = $state(true);
  let chatInputRef: any = $state(null);
  let projectDropdownOpen = $state(false);
  let projectDropdownRef = $state<HTMLDivElement | null>(null);
  let homeInput = $state("");
  let submitting = $state(false);

  const selectedProject = $derived(
    selectedProjectId && selectedProjectId !== "new"
      ? projectState.projects.find((p) => p.id === selectedProjectId)
      : undefined
  );

  function onNearBottomChange(near: boolean) {
    isNearBottom = near;
  }

  function scrollToBottom() {
    listRef?.scrollToBottom?.();
    isNearBottom = true;
  }

  // Pick the first project by default when projects load and nothing selected
  $effect(() => {
    if (!sessionState.activeSessionId && selectedProjectId === "" && projectState.projects.length > 0) {
      selectedProjectId = projectState.projects[0].id;
    }
  });

  // Refresh project list on mount
  $effect(() => {
    api.listProjects().then(list => {
      projectState.projects = list.map(p => ({
        id: p.id,
        name: p.name,
        dir: p.dir,
        createdAt: p.createdAt,
        updatedAt: p.updatedAt,
      }));
    }).catch(() => {});
  });

  async function handleHomeSubmit() {
    if (submitting || !homeInput.trim()) return;

    let projectId: string | undefined;
    let workingDir: string;

    if (selectedProjectId === "") {
      showNotification("Please select a project", "error", 3000);
      return;
    }

    if (selectedProjectId === "new") {
      const dir = newProjectPath.trim();
      if (!dir) {
        showNotification("Project path is required", "error", 3000);
        return;
      }
      submitting = true;
      try {
        const project = await api.createProject(dir, newProjectName.trim() || undefined);
        projectId = project.id;
        workingDir = project.dir;
        projectState.projects.push({
          id: project.id,
          name: project.name,
          dir: project.dir,
          createdAt: project.createdAt,
          updatedAt: project.updatedAt,
        });
      } catch (e: any) {
        console.error("Failed to create project:", e?.message ?? e);
        showNotification("Failed to create project: " + (e?.message ?? ""), "error", 5000);
        submitting = false;
        return;
      }
    } else {
      const project = projectState.projects.find((p) => p.id === selectedProjectId);
      if (!project) {
        showNotification("Please select a project", "error", 3000);
        return;
      }
      projectId = project.id;
      workingDir = project.dir;
      submitting = true;
    }

    try {
      const id = await api.createSession(workingDir, permissionLevel, projectId);
      const result = await api.listSessions(projectId, undefined, 20);
      for (const s of result.sessions) {
        if (!sessionState.sessions.find(sess => sess.id === s.id)) {
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
          });
        }
      }
      setActiveSession(id);
      await api.subscribe(id);
      const msgs = await api.getMessages(id);
      const session = sessionState.sessions.find(s => s.id === id);
      if (session) {
        loadSessionMessages(id, msgs);
      }
      // Send the home input
      const text = homeInput.trim();
      homeInput = "";
      addUserMessage(id, text);
      await api.sendMessage(id, text);
    } catch (e: any) {
      console.error("Failed to create session:", e?.message ?? e);
      showNotification("Failed to create session: " + (e?.message ?? "Unknown error"), "error", 5000);
    } finally {
      submitting = false;
    }
  }

  async function browseProjectDir() {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
      });
      if (selected) {
        newProjectPath = selected as string;
      }
    } catch (e) {
      console.error("Failed to open directory picker:", e);
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

  function closeProjectDropdown(e: MouseEvent) {
    if (projectDropdownRef && !projectDropdownRef.contains(e.target as Node)) {
      projectDropdownOpen = false;
    }
  }

  $effect(() => {
    if (projectDropdownOpen) {
      window.addEventListener("click", closeProjectDropdown);
      return () => window.removeEventListener("click", closeProjectDropdown);
    }
  });

  function handleHomeKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      handleHomeSubmit();
    }
  }
</script>

<div class="flex flex-col h-full">
  {#if activeSession}
  <!-- Header -->
  <div class="flex items-center justify-between px-4 py-2 border-b border-border">
    <div class="flex items-center gap-2">
      <span class="font-medium truncate">{activeSession.alias ?? activeSession.id.slice(0, 8)}</span>
      <span class="text-xs text-muted-foreground">{activeSession.projectPath}</span>
    </div>
  </div>
  {/if}

  <!-- InfoBar removed from here — moved into chat area above ChatInput -->

  <!-- Tabs — only show non-chat tabs (e.g. preview/edit) -->
  {#if activeSession && hasNonChatTabs}
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
      <!-- Clean home screen with direct message input -->
      <div class="flex flex-col items-center justify-center h-full px-6">
        <div class="w-full max-w-2xl">
          <!-- Title -->
          <div class="text-center mb-8">
            <h1 class="text-4xl font-bold tracking-tight mb-2">Yomi</h1>
            <p class="text-muted-foreground text-lg">What can I help you with today?</p>
          </div>

          <!-- Input card -->
          <div class="rounded-2xl border border-border bg-card shadow-sm focus-within:shadow-md focus-within:ring-1 focus-within:ring-ring transition-all">
            <div class="p-4">
              <textarea
                bind:value={homeInput}
                onkeydown={handleHomeKeydown}
                placeholder="Ask anything..."
                rows={3}
                disabled={submitting}
                class="w-full resize-none bg-transparent text-base placeholder:text-muted-foreground focus:outline-none disabled:opacity-50"
              ></textarea>
            </div>
            <div class="px-4 py-3 border-t border-border flex items-center justify-between gap-3">
              <div class="flex items-center gap-3">
                <!-- Project selector -->
                <div class="relative" bind:this={projectDropdownRef}>
                  <button
                    type="button"
                    onclick={() => projectDropdownOpen = !projectDropdownOpen}
                    class="flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors"
                  >
                    <FolderOpen class="w-4 h-4" />
                    <span class="max-w-[140px] truncate">
                      {#if selectedProjectId === ""}
                        Select project
                      {:else if selectedProjectId === "new"}
                        + New Project
                      {:else}
                        {projectState.projects.find(p => p.id === selectedProjectId)?.name ?? "Unknown"}
                      {/if}
                    </span>
                    <ChevronDown class="w-3 h-3" />
                  </button>
                  {#if projectDropdownOpen}
                    <div class="absolute bottom-full left-0 mb-1 z-50 w-56 rounded-lg border border-border bg-popover shadow-lg overflow-hidden max-h-60 overflow-y-auto">
                      {#each projectState.projects as project (project.id)}
                        <button
                          type="button"
                          onclick={() => { selectedProjectId = project.id; projectDropdownOpen = false; }}
                          class="w-full text-left px-3 py-2 text-sm hover:bg-accent transition-colors {selectedProjectId === project.id ? 'bg-accent/50' : ''}"
                        >
                          <div class="font-medium">{project.name}</div>
                          <div class="text-xs text-muted-foreground truncate">{project.dir}</div>
                        </button>
                      {/each}
                      <div class="border-t border-border"></div>
                      <button
                        type="button"
                        onclick={() => { selectedProjectId = "new"; projectDropdownOpen = false; }}
                        class="w-full text-left px-3 py-2 text-sm hover:bg-accent transition-colors text-primary font-medium {selectedProjectId === 'new' ? 'bg-accent/50' : ''}"
                      >
                        + New Project...
                      </button>
                    </div>
                  {/if}
                </div>

                <!-- Permission level -->
                <div class="flex items-center gap-1">
                  {#each ["safe", "caution", "dangerous"] as level (level)}
                    {@const Icon = levelIcon(level)}
                    <button
                      type="button"
                      onclick={() => permissionLevel = level}
                      class="p-1 rounded transition-colors {permissionLevel === level ? levelColor(level) : 'text-muted-foreground hover:text-foreground'}"
                      title={levelDescription(level)}
                    >
                      <Icon class="w-4 h-4" />
                    </button>
                  {/each}
                </div>
              </div>

              <button
                type="button"
                onclick={handleHomeSubmit}
                disabled={submitting || !homeInput.trim() || selectedProjectId === ""}
                class="inline-flex items-center justify-center rounded-lg bg-primary text-primary-foreground h-8 w-8 hover:bg-primary/90 disabled:opacity-50 shrink-0 transition-colors"
              >
                {#if submitting}
                  <span class="w-3.5 h-3.5 border-2 border-primary-foreground/30 border-t-primary-foreground rounded-full animate-spin"></span>
                {:else}
                  <Send class="w-4 h-4" />
                {/if}
              </button>
            </div>
          </div>

          <!-- New project inputs -->
          {#if selectedProjectId === "new"}
            <div class="mt-3 space-y-2 rounded-xl border border-border bg-card p-3 shadow-sm">
              <div class="flex gap-2">
                <div class="relative flex-1">
                  <FolderOpen class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground" />
                  <input
                    type="text"
                    bind:value={newProjectPath}
                    placeholder="Project directory path..."
                    class="w-full pl-9 pr-3 py-2 rounded-lg border border-border bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring focus:border-ring"
                  />
                </div>
                <button
                  type="button"
                  onclick={browseProjectDir}
                  class="shrink-0 px-3 py-2 rounded-lg border border-border bg-background text-sm text-muted-foreground hover:bg-secondary hover:text-foreground transition-colors"
                  title="Browse directory"
                >
                  Browse
                </button>
              </div>
              <input
                type="text"
                bind:value={newProjectName}
                placeholder="Project name (optional)..."
                class="w-full px-3 py-2 rounded-lg border border-border bg-background text-sm focus:outline-none focus:ring-2 focus:ring-ring focus:border-ring"
              />
            </div>
          {/if}
        </div>
      </div>
    {:else if activeSession?.activeTabId === "chat"}
      <div class="flex h-full relative">
        <!-- Main chat area -->
        <div class="flex-1 flex flex-col h-full min-w-0 relative">
          <MessageList bind:this={listRef} onNearBottomChange={onNearBottomChange} />
          <QueuedInputBar session={activeSession} onEdit={(text) => chatInputRef?.setContent(text)} />
          <InfoBar session={activeSession} />
          <PermissionBar />
          <AskUserBar />
          <ChatInput bind:this={chatInputRef} />
          {#if !isNearBottom}
            <button
              type="button"
              onclick={scrollToBottom}
              class="absolute bottom-16 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1 px-3 py-1.5 rounded-full bg-primary text-primary-foreground text-xs shadow-lg hover:bg-primary/90 transition-colors"
            >
              <ArrowDown class="w-3 h-3" />
              Bottom
            </button>
          {/if}
        </div>
        <!-- Right side panel -->
        <RightPanel session={activeSession} />
      </div>
    {:else if activeSession}
      {@const activeTab = activeSession.tabs.find(t => t.id === activeSession.activeTabId)}
      {#if activeTab?.type === "preview" && activeTab.entry}
        <div class="flex h-full relative">
          <div class="flex-1 min-w-0">
            <FilePreview
              entry={activeTab.entry}
              onEdit={(_e) => { /* TODO: open edit tab */ }}
              onAskAI={(_path) => { /* TODO: send to chat */ }}
            />
          </div>
          <RightPanel session={activeSession} />
        </div>
      {:else if activeTab?.type === "edit" && activeTab.entry}
        <div class="flex h-full relative">
          <div class="flex-1 min-w-0">
            <FileEditor entry={activeTab.entry} />
          </div>
          <RightPanel session={activeSession} />
        </div>
      {/if}
    {:else}
      <div class="flex items-center justify-center h-full text-muted-foreground">
        Loading session...
      </div>
    {/if}
  </div>
</div>
