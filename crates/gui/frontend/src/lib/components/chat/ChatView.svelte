<script lang="ts">
  import { sessionState, projectState, getActiveSession, closeTab, setActiveSession, showNotification, loadSessionMessages, streamingMessages } from "../../state.svelte";
  import * as api from "../../api";
  import TabBar from "../layout/TabBar.svelte";
  import MessageList from "./MessageList.svelte";
  import ChatInput from "./ChatInput.svelte";
  import FilePreview from "../editor/FilePreview.svelte";
  import FileEditor from "../editor/FileEditor.svelte";
  import InfoBar from "./InfoBar.svelte";
  import PermissionBar from "./PermissionBar.svelte";
  import AskUserBar from "./AskUserBar.svelte";
  import QueuedInputBar from "./QueuedInputBar.svelte";
  import { FolderOpen, ArrowDown, ChevronDown, Send, PanelRightOpen, PanelRightClose, PanelLeftOpen, ExternalLink, Paperclip, X } from "lucide-svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { levelLabel, levelDescription, levelIcon, levelColor, type PermissionLevel } from "../../permission";

  let { rightPanelCollapsed, onToggleRightPanel, onToggleLeftPanel }: { rightPanelCollapsed?: boolean; onToggleRightPanel?: () => void; onToggleLeftPanel?: () => void } = $props();

  const activeSession = $derived(getActiveSession());

  const hasNonChatTabs = $derived(activeSession?.tabs.some((t: any) => t.type !== "chat") ?? false);

  let selectedProjectId = $state<string | "new">("");
  let newProjectPath = $state("");
  let newProjectName = $state("");
  let permissionLevel = $state("");
  let permissionLevelReady = $state(false);
  let listRef: any = $state(null);
  let isNearBottom = $state(true);
  let chatInputRef: any = $state(null);
  let projectDropdownOpen = $state(false);
  let openDropdownOpen = $state(false);
  let projectDropdownRef = $state<HTMLDivElement | null>(null);
  let homeInput = $state("");
  let submitting = $state(false);
  let homeFileAttachments = $state<string[]>([]);

  // ── home inline images (clipboard paste) ──
  interface HomeInlineImage {
    id: number;
    url: string;
  }
  let homeInlineImages = $state<HomeInlineImage[]>([]);
  let homeInlineImageCounter = $state(0);

  function addHomeInlineImage(base64Url: string) {
    homeInlineImageCounter += 1;
    homeInlineImages = [...homeInlineImages, { id: homeInlineImageCounter, url: base64Url }];
  }

  function removeHomeInlineImage(id: number) {
    homeInlineImages = homeInlineImages.filter((img) => img.id !== id);
  }

  function clearHomeInlineImages() {
    homeInlineImages = [];
    homeInlineImageCounter = 0;
  }

  async function readHomeFileAsBase64(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(reader.result as string);
      reader.onerror = reject;
      reader.readAsDataURL(file);
    });
  }

  async function handleHomePaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;
    for (const item of items) {
      if (item.type.startsWith("image/")) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) {
          try {
            const base64Url = await readHomeFileAsBase64(file);
            addHomeInlineImage(base64Url);
          } catch (err) {
            console.error("Failed to read clipboard image:", err);
          }
        }
      }
    }
  }

  function buildHomeContentBlocks(text: string): unknown[] {
    const blocks: unknown[] = [];
    for (const img of homeInlineImages) {
      blocks.push({
        type: "image_url",
        image_url: { url: img.url, detail: "auto" },
      });
    }
    const trimmed = text.trim();
    if (trimmed) {
      blocks.push({ type: "text", text: trimmed });
    }
    if (blocks.length === 0) {
      blocks.push({ type: "text", text: "" });
    }
    return blocks;
  }

  async function attachHomeFiles() {
    try {
      const selected = await open({ multiple: true });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      const newPaths = paths.filter((p) => !homeFileAttachments.includes(p));
      if (newPaths.length === 0) return;
      homeFileAttachments = [...homeFileAttachments, ...newPaths];
      const sep = homeInput.length > 0 && !homeInput.endsWith("\n") ? "\n" : "";
      const additions = newPaths.map((p) => `[File: ${p}]`).join("\n");
      homeInput += `${sep}${additions}\n`;
    } catch (e) {
      console.error("Failed to attach files:", e);
    }
  }

  function removeHomeFileAttachment(path: string) {
    homeFileAttachments = homeFileAttachments.filter((p) => p !== path);
    const marker = `[File: ${path}]`;
    const lines = homeInput.split("\n");
    const filtered = lines.filter((line) => line.trim() !== marker);
    homeInput = filtered.join("\n");
  }

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
    let cancelled = false;
    api.listProjects().then(list => {
      if (cancelled) return;
      projectState.projects = list.map(p => ({
        id: p.id,
        name: p.name,
        dir: p.dir,
        createdAt: p.createdAt,
        updatedAt: p.updatedAt,
      }));
    }).catch(() => {});
    api.getConfig().then(c => {
      if (cancelled) return;
      if (c?.auto_approve) {
        permissionLevel = c.auto_approve;
      } else {
        permissionLevel = "caution";
      }
      permissionLevelReady = true;
    }).catch(() => {
      if (cancelled) return;
      permissionLevel = "caution";
      permissionLevelReady = true;
    });
    return () => { cancelled = true; };
  });

  async function handleHomeSubmit() {
    if (submitting || !homeInput.trim()) return;

    let level = permissionLevel;
    if (!level) {
      try {
        const c = await api.getConfig();
        level = c.auto_approve || "caution";
      } catch {
        level = "caution";
      }
    }

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
      const id = await api.createSession(workingDir, level, projectId);
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
            updatedAt: s.endedAt ?? s.createdAt,
            permissionLevel: s.autoApproveLevel ?? level ?? "caution",
          });
        }
      }
      setActiveSession(id);
      await api.subscribe(id);
      const msgs = await api.getMessages(id);
      const session = sessionState.sessions.find(s => s.id === id);
      if (session) {
        loadSessionMessages(id, msgs);
        // Merge any stale streaming buffer (defensive)
        const buf = streamingMessages[id] ?? [];
        if (buf.length > 0) {
          session.messages = [...session.messages, ...buf];
          streamingMessages[id] = [];
        }
      }
      // Send the home input
      const text = homeInput.trim();
      homeInput = "";
      homeFileAttachments = [];
      const hasImages = homeInlineImages.length > 0;
      if (hasImages) {
        const blocks = buildHomeContentBlocks(text);
        clearHomeInlineImages();
        await api.sendMessageBlocks(id, blocks);
      } else {
        clearHomeInlineImages();
        await api.sendMessage(id, text);
      }
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

  let editingTitle = $state(false);
  let titleValue = $state("");

  async function confirmRenameTitle() {
    if (!activeSession) return;
    const name = titleValue.trim();
    if (!name || name === (activeSession.alias ?? activeSession.id.slice(-8))) {
      editingTitle = false;
      return;
    }
    try {
      await api.renameSession(activeSession.id, name);
      activeSession.alias = name;
      showNotification("Session renamed", "success", 2000);
    } catch (e: any) {
      console.error("Failed to rename session:", e?.message ?? e);
      showNotification("Failed to rename session", "error", 3000);
    } finally {
      editingTitle = false;
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

<div class="flex-1 flex flex-col min-w-0 overflow-hidden">
  {#if activeSession}
  <!-- Header -->
  <div class="flex items-center justify-between px-4 py-2 border-b border-border">
    <div class="flex items-center gap-2 min-w-0">
      <!-- Mobile sidebar toggle -->
      {#if onToggleLeftPanel}
        <button
          type="button"
          onclick={() => onToggleLeftPanel()}
          class="lg:hidden p-1.5 rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground mr-1"
          title="Toggle sidebar"
        >
          <PanelLeftOpen size={16} />
        </button>
      {/if}
      {#if editingTitle}
        <!-- svelte-ignore a11y_autofocus -->
        <input
          type="text"
          bind:value={titleValue}
          onkeydown={(e: KeyboardEvent) => { if (e.key === 'Enter') confirmRenameTitle(); if (e.key === 'Escape') editingTitle = false; }}
          onblur={() => confirmRenameTitle()}
          class="flex-1 min-w-0 bg-background border border-border rounded px-2 py-0.5 text-sm font-medium focus:outline-none focus:ring-1 focus:ring-ring"
          autofocus
        />
      {:else}
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <span
          class="font-medium truncate cursor-pointer hover:text-primary transition-colors"
          title={activeSession.alias ?? activeSession.id.slice(-8)}
          ondblclick={() => { editingTitle = true; titleValue = activeSession.alias ?? activeSession.id.slice(-8); }}
          role="button"
          tabindex="0"
        >
          {activeSession.alias ?? activeSession.id.slice(-8)}
        </span>
      {/if}
      <span class="text-xs text-muted-foreground truncate">{activeSession.projectPath}</span>
    </div>
    <div class="flex items-center gap-2">
      {#if activeSession.projectPath}
        <div class="relative">
          <button
            type="button"
            onclick={() => openDropdownOpen = !openDropdownOpen}
            class="p-1.5 rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground"
            title="Open project"
          >
            <ExternalLink size={16} />
          </button>
          {#if openDropdownOpen}
            <div class="absolute right-0 top-full mt-1 z-20 w-40 rounded-md border border-border bg-popover shadow-md py-1">
              <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left" onclick={() => { api.openInExplorer(activeSession.projectPath); openDropdownOpen = false; }}>
                <ExternalLink size={12} /> Open in Explorer
              </button>
              <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left" onclick={() => { api.openInVscode(activeSession.projectPath); openDropdownOpen = false; }}>
                <ExternalLink size={12} /> Open in VS Code
              </button>
              <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left" onclick={() => { api.openInZed(activeSession.projectPath); openDropdownOpen = false; }}>
                <ExternalLink size={12} /> Open in Zed
              </button>
            </div>
            <div class="fixed inset-0 z-10" onclick={() => openDropdownOpen = false}></div>
          {/if}
        </div>
      {/if}
      <button
        type="button"
        onclick={() => onToggleRightPanel?.()}
        class="p-1.5 rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground"
        title={rightPanelCollapsed ? "Open side panel" : "Close side panel"}
      >
        {#if rightPanelCollapsed}
          <PanelRightOpen size={16} />
        {:else}
          <PanelRightClose size={16} />
        {/if}
      </button>
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
            <img src="/yomi-dark.png" alt="Yomi" class="w-24 h-24 mx-auto mb-3 object-contain hidden dark:block" />
            <img src="/yomi-light.png" alt="Yomi" class="w-24 h-24 mx-auto mb-3 object-contain dark:hidden" />
            <p class="text-muted-foreground text-lg">What can I help you with today?</p>
          </div>

          <!-- Input card -->
          <div class="rounded-2xl border border-border bg-card shadow-sm focus-within:shadow-md focus-within:ring-1 focus-within:ring-ring transition-all">
            <div class="p-4">
              <textarea
                bind:value={homeInput}
                onkeydown={handleHomeKeydown}
                onpaste={handleHomePaste}
                placeholder="Ask anything..."
                rows={3}
                disabled={submitting}
                class="w-full resize-none bg-transparent text-base placeholder:text-muted-foreground focus:outline-none disabled:opacity-50"
              ></textarea>
              {#if homeInlineImages.length > 0}
                <div class="flex flex-wrap gap-2 mt-2">
                  {#each homeInlineImages as img (img.id)}
                    <div class="relative group shrink-0">
                      <img
                        src={img.url}
                        alt=""
                        class="h-16 w-16 object-cover rounded-lg border border-border"
                      />
                      <button
                        type="button"
                        onclick={() => removeHomeInlineImage(img.id)}
                        class="absolute -top-1.5 -right-1.5 w-5 h-5 rounded-full bg-destructive text-destructive-foreground flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity shadow-sm"
                        title="Remove"
                      >
                        <X size={12} />
                      </button>
                    </div>
                  {/each}
                </div>
              {/if}
              {#if homeFileAttachments.length > 0}
                <div class="flex items-center gap-2 mt-2 flex-wrap">
                  {#each homeFileAttachments as path (path)}
                    <div class="flex items-center gap-1.5 rounded-md border border-border bg-secondary px-2 py-0.5">
                      <span class="text-xs text-muted-foreground truncate max-w-[200px]">{path.split("/").pop()}</span>
                      <button
                        type="button"
                        onclick={() => removeHomeFileAttachment(path)}
                        class="text-muted-foreground hover:text-destructive transition-colors"
                        title="Remove"
                      >
                        <X size={12} />
                      </button>
                    </div>
                  {/each}
                </div>
              {/if}
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

                <!-- Attach button -->
                <button
                  type="button"
                  onclick={attachHomeFiles}
                  class="inline-flex items-center gap-1 px-2 py-1 rounded-md text-xs text-muted-foreground hover:text-foreground hover:bg-secondary/50 transition-colors"
                  title="Attach files"
                >
                  <Paperclip size={14} />
                </button>
                <!-- Permission level -->
                <div class="flex items-center gap-1">
                  {#each (["safe", "caution", "dangerous"] as PermissionLevel[]) as level (level)}
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
          <div class="shrink-0 w-full">
            <div class="container mx-auto px-4 lg:px-6">
              <QueuedInputBar session={activeSession} onEdit={(text) => chatInputRef?.setContent(text)} />
              <InfoBar session={activeSession} />
              <PermissionBar />
              <AskUserBar />
              <ChatInput bind:this={chatInputRef} />
            </div>
          </div>
          {#if !isNearBottom}
            <button
              type="button"
              onclick={scrollToBottom}
              class="absolute bottom-20 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1 px-3 py-1.5 rounded-full bg-primary text-primary-foreground text-xs shadow-lg hover:bg-primary/90 transition-colors"
            >
              <ArrowDown class="w-3 h-3" />
              Bottom
            </button>
          {/if}
        </div>
      </div>
    {:else if activeSession}
      {@const activeTab = activeSession.tabs.find((t: any) => t.id === activeSession.activeTabId)}
      {#if activeTab?.type === "preview" && activeTab.entry}
        <div class="flex h-full relative container mx-auto">
          <div class="flex-1 min-w-0">
            <FilePreview
              entry={activeTab.entry}
              onEdit={(_e) => { /* TODO: open edit tab */ }}
              onAskAI={(_path) => { /* TODO: send to chat */ }}
            />
          </div>
        </div>
      {:else if activeTab?.type === "edit" && activeTab.entry}
        <div class="flex h-full relative container mx-auto">
          <div class="flex-1 min-w-0">
            <FileEditor entry={activeTab.entry} />
          </div>
        </div>
      {/if}
    {:else}
      <div class="flex items-center justify-center h-full text-muted-foreground">
        Loading session...
      </div>
    {/if}
  </div>
</div>
