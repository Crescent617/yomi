<script lang="ts">
  import { sessionState, projectState, getSession, getActiveSession, closeTab, setActiveSession, showNotification, loadSessionMessages, streamingMessages, syncSessionStatus, refreshCheckpoints } from "../../state.svelte";
  import * as api from "../../api";
  import { collapseHome } from "../../utils";
  import TabBar from "../layout/TabBar.svelte";
  import MessageList from "./MessageList.svelte";
  import ChatInput from "./ChatInput.svelte";
  import FilePreview from "../editor/FilePreview.svelte";
  import FileEditor from "../editor/FileEditor.svelte";
  import InfoBar from "./InfoBar.svelte";
  import PermissionBar from "./PermissionBar.svelte";
  import AskUserBar from "./AskUserBar.svelte";
  import QueuedInputBar from "./QueuedInputBar.svelte";
  import { FolderOpen, ChevronDown, Send, PanelRightOpen, PanelRightClose, PanelLeftOpen, PanelLeftClose, ExternalLink, Paperclip, X, Code, Zap, GitBranch, FileDiff, Command } from "lucide-svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { homeDir } from "@tauri-apps/api/path";
  import { levelDescription, levelIcon, levelColor, type PermissionLevel } from "../../permission";
  import { createFilePicker } from "$lib/filePicker";
  import type { FileEntry } from "../../fs/provider";
  import FilePicker from "../filePicker/FilePicker.svelte";
  import type { TaggedContentBlock } from "../../types";

  let { rightPanelCollapsed, onToggleRightPanel, onToggleLeftPanel, leftPanelCollapsed }: { rightPanelCollapsed?: boolean; onToggleRightPanel?: () => void; onToggleLeftPanel?: () => void; leftPanelCollapsed?: boolean } = $props();

  const activeSession = $derived(getActiveSession());

  const hasNonChatTabs = $derived(activeSession?.tabs.some((t: { type: string }) => t.type !== "chat") ?? false);

  let selectedProjectId = $state<string | "new">("");
  let newProjectPath = $state("");
  let newProjectName = $state("");
  let permissionLevel = $state("");
  let chatInputRef: { setContent?: (text: string) => void; focus?: () => void } | null = $state(null);
  let projectDropdownOpen = $state(false);
  let openDropdownOpen = $state(false);
  let projectDropdownRef = $state<HTMLDivElement | null>(null);
  let homeInput = $state("");
  let submitting = $state(false);
  let homeFileAttachments = $state<string[]>([]);
  let homeDirPath = $state("");

  // ── home file picker (shared hook) ──
  const homeFilePicker = createFilePicker();
  let homeTextareaRef: HTMLTextAreaElement | null = $state(null);

  // ── home command picker (only /goal on home screen) ──
  let showCommands = $state(false);
  let commandFilter = $state("");
  let selectedCommandIdx = $state(0);
  let homeCommandListRef: HTMLDivElement | null = $state(null);
  const HOME_COMMANDS: readonly (readonly [string, string])[] = [
    ["/goal", "<description> Start goal mode with optional description"],
  ];
  const filteredHomeCommands = $derived(
    HOME_COMMANDS.filter(([cmd]) => cmd.toLowerCase().includes(commandFilter.toLowerCase()))
  );

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

  function buildHomeContentBlocks(text: string): TaggedContentBlock[] {
    const blocks: TaggedContentBlock[] = [];
    for (const img of homeInlineImages) {
      blocks.push({
        type: "imageUrl",
        imageUrl: { url: img.url, detail: "auto" },
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
    } catch (e) {
      console.error("Failed to attach files:", e);
    }
  }

  function removeHomeFileAttachment(path: string) {
    homeFileAttachments = homeFileAttachments.filter((p) => p !== path);
  }

  // Auto-focus chat input when switching to a new session (only once per session)
  let lastFocusedSessionId = $state("");
  $effect(() => {
    const id = activeSession?.id;
    if (id && id !== lastFocusedSessionId) {
      lastFocusedSessionId = id;
      chatInputRef?.focus?.();
    }
  });

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
      if (c?.autoApprove) {
        permissionLevel = c.autoApprove;
      } else {
        permissionLevel = "caution";
      }
    }).catch(() => {
      if (cancelled) return;
      permissionLevel = "caution";
    });
    homeDir().then(h => {
      if (!cancelled) homeDirPath = h;
    }).catch(() => {});
    return () => { cancelled = true; };
  });

  async function handleHomeSubmit() {
    if (submitting || !homeInput.trim()) return;

    // Validate /goal before clearing any state
    const baseText = homeInput.trim();
    const isGoal = baseText.toLowerCase() === "/goal" || baseText.toLowerCase().startsWith("/goal ");
    if (isGoal) {
      const description = baseText.slice(5).trim();
      if (!description) {
        showNotification("Please provide a goal description: /goal <description>", "error", 5000);
        return;
      }
    }

    let level = permissionLevel;
    if (!level) {
      try {
        const c = await api.getConfig();
        level = c.autoApprove || "caution";
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
      } catch (e: unknown) {
        console.error("Failed to create project:", e instanceof Error ? e.message : e);
        showNotification("Failed to create project: " + (e instanceof Error ? e.message : ""), "error", 5000);
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
            alias: s.title ?? "Untitled",
            messages: [],
            phase: "idle",
            unread: 0,
            checkpoints: [],
            tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
            activeTabId: "chat",
            pendingPermissions: [],
            pendingAskUser: null,
            queuedInput: null,
            updatedAt: s.endedAt ?? s.createdAt,
            permissionLevel: s.autoApproveLevel ?? level ?? "caution",
            goal: null,
          });
        }
      }
      setActiveSession(id);
      await api.subscribe(id);
      const status = await api.getSessionStatus(id);
      const msgs = await api.getMessages(id);
      const session = sessionState.sessions.find(s => s.id === id);
      if (session) {
        syncSessionStatus(id, status);
        loadSessionMessages(id, msgs);
        refreshCheckpoints(id);
        api.getGoal(id).then((g) => { session.goal = g; }).catch(() => { session.goal = null; });
        const buf = streamingMessages[id] ?? [];
        if (buf.length > 0) {
          session.messages = [...session.messages, ...buf];
          streamingMessages[id] = [];
        }
      }
      // Send the home input
      homeInput = "";
      const fileSuffix = homeFileAttachments.length > 0
        ? "\n" + homeFileAttachments.map((p) => `[File: ${p}]`).join("\n")
        : "";
      const text = baseText + fileSuffix;
      homeFileAttachments = [];
      const hasImages = homeInlineImages.length > 0;
      if (isGoal) {
        const description = baseText.slice(5).trim();
        await api.startGoal(id, description);
        {
          const session = sessionState.sessions.find(s => s.id === id);
          if (session) {
            api.getGoal(id).then((g) => { session.goal = g; }).catch(() => {});
          }
        }
        // rename_session will emit TitleUpdated event — alias is synced there
        try {
          await api.renameSession(id, description);
        } catch {
          // ignore rename failure
        }
        console.log("Goal mode activated — agent will work autonomously");
      } else if (hasImages) {
        const blocks = buildHomeContentBlocks(text);
        await api.sendMessageBlocks(id, blocks);
      } else {
        await api.sendMessage(id, text);
      }
      clearHomeInlineImages();
    } catch (e: unknown) {
      console.error("Failed to create session:", e instanceof Error ? e.message : e);
      showNotification("Failed to create session: " + (e instanceof Error ? e.message : "Unknown error"), "error", 5000);
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
    } catch (e: unknown) {
      console.error("Failed to rename session:", e instanceof Error ? e.message : e);
      showNotification("Failed to rename session", "error", 3000);
    } finally {
      editingTitle = false;
    }
  }

  // ── Git info sync ──
  async function syncGitInfo(sessionId: string, projectPath: string) {
    try {
      const info = await api.getGitInfo(projectPath);
      const session = getSession(sessionId);
      if (session && session.id === activeSession?.id) {
        session.gitInfo = info;
      }
    } catch {
      const session = getSession(sessionId);
      if (session && session.id === activeSession?.id) {
        session.gitInfo = null;
      }
    }
  }

  // Refresh immediately when active session changes
  $effect(() => {
    const session = activeSession;
    if (!session?.projectPath) return;
    syncGitInfo(session.id, session.projectPath);
  });

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

  $effect(() => {
    if (showCommands && homeCommandListRef) {
      const buttons = homeCommandListRef.querySelectorAll("button");
      const selected = buttons[selectedCommandIdx];
      if (selected) {
        selected.scrollIntoView({ block: "nearest", inline: "nearest" });
      }
    }
  });

  $effect(() => {
    if (selectedCommandIdx >= filteredHomeCommands.length) {
      selectedCommandIdx = Math.max(0, filteredHomeCommands.length - 1);
    }
  });

  function detectHomeCompletion() {
    if (!homeTextareaRef) return;
    const cursorPos = homeTextareaRef.selectionStart;
    const beforeCursor = homeInput.slice(0, cursorPos);

    // ── command: starts with / ──
    if (homeInput.startsWith("/")) {
      homeFilePicker.close();
      const query = homeInput.slice(1);
      const valid = /^[a-zA-Z0-9_\-:]*$/.test(query);
      if (valid) {
        showCommands = true;
        commandFilter = query;
        selectedCommandIdx = 0;
      } else {
        showCommands = false;
      }
      return;
    } else {
      showCommands = false;
    }

    // ── file: last @ before cursor ──
    const lastAt = beforeCursor.lastIndexOf("@");
    if (lastAt >= 0) {
      const afterAt = beforeCursor.slice(lastAt + 1);
      if (!afterAt.includes(" ")) {
        const root = projectState.projects.find(p => p.id === selectedProjectId)?.dir || "";
        homeFilePicker.open(lastAt, afterAt, root);
      } else {
        homeFilePicker.close();
      }
    } else {
      homeFilePicker.close();
    }
  }

  function acceptHomeCommand(cmd: string) {
    homeInput = cmd + " ";
    showCommands = false;
    homeTextareaRef?.focus();
  }

  function onEnterHomeDir(entry: FileEntry) {
    const newQuery = homeFilePicker.enterDir(entry);
    const before = homeInput.slice(0, homeFilePicker.anchor);
    const after = homeInput.slice(homeTextareaRef?.selectionStart ?? homeInput.length);
    homeInput = before + "@" + newQuery + after;
    homeTextareaRef?.focus();
  }

  function onAcceptHomeFile(entry: FileEntry) {
    const resultPath = homeFilePicker.acceptFile(entry);
    const cursorPos = homeTextareaRef?.selectionStart ?? homeInput.length;
    const before = homeInput.slice(0, homeFilePicker.anchor);
    const after = homeInput.slice(cursorPos);
    homeInput = before + "@" + resultPath + " " + after;
    homeFilePicker.close();
    homeTextareaRef?.focus();
  }

  function handleHomeFocusOut(e: FocusEvent) {
    const container = e.currentTarget as HTMLElement;
    if (!container.contains(e.relatedTarget as Node)) {
      homeFilePicker.close();
      showCommands = false;
    }
  }

  function handleHomeKeydown(e: KeyboardEvent) {
    // Command picker navigation
    if (showCommands) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        if (filteredHomeCommands.length === 0) return;
        selectedCommandIdx = (selectedCommandIdx + 1) % filteredHomeCommands.length;
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (filteredHomeCommands.length === 0) return;
        selectedCommandIdx = (selectedCommandIdx - 1 + filteredHomeCommands.length) % filteredHomeCommands.length;
        return;
      }
      if (e.key === "Tab" || e.key === "Enter") {
        e.preventDefault();
        if (filteredHomeCommands.length === 0) return;
        const cmd = filteredHomeCommands[selectedCommandIdx]?.[0];
        if (cmd) acceptHomeCommand(cmd);
        return;
      }
      if (e.key === "Escape") {
        showCommands = false;
        return;
      }
    }

    if (homeFilePicker.show) {
      const handled = homeFilePicker.handleKeydown(e);
      if (handled) {
        const entries = homeFilePicker.entries;
        const idx = homeFilePicker.selectedIdx;
        const entry = entries[idx];
        if (entry && !entry.isDirectory && (e.key === "Enter" || e.key === "Tab")) {
          onAcceptHomeFile(entry);
        }
        return;
      }
    }
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
      <!-- Left panel toggle -->
      {#if onToggleLeftPanel}
        <button
          type="button"
          onclick={() => onToggleLeftPanel()}
          class="p-1.5 rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground mr-1"
          title={leftPanelCollapsed ? "Expand sidebar" : "Collapse sidebar"}
        >
          {#if leftPanelCollapsed}
            <PanelLeftOpen size={16} />
          {:else}
            <PanelLeftClose size={16} />
          {/if}
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
      {#if activeSession.projectPath}
        {@const displayPath = collapseHome(activeSession.projectPath, homeDirPath)}
        <span class="text-xs text-muted-foreground truncate" title={activeSession.projectPath}>{displayPath}</span>
      {/if}
      {#if activeSession.gitInfo?.branch}
        <span class="inline-flex items-center gap-1 text-xs text-muted-foreground/80 bg-muted rounded px-1.5 py-0.5 ml-1">
          <GitBranch size={10} />
          {activeSession.gitInfo.branch}
        </span>
        {@const g = activeSession.gitInfo}
        {#if g.addedLines > 0 || g.deletedLines > 0 || g.untracked > 0}
          <span class="inline-flex items-center gap-1 text-xs text-muted-foreground/70 font-mono bg-muted rounded px-1.5 py-0.5 ml-1">
            <FileDiff size={10} class="text-muted-foreground/50 shrink-0" />
            {#if g.addedLines > 0}{#key g.addedLines}<span class="roll-num text-green-700/80 dark:text-green-400/80">+{g.addedLines}</span>{/key}{/if}
            {#if g.deletedLines > 0}{#key g.deletedLines}<span class="roll-num text-red-700/80 dark:text-red-400/80">-{g.deletedLines}</span>{/key}{/if}
            {#if g.untracked > 0}{#key g.untracked}<span class="roll-num text-slate-500 dark:text-slate-400">?{g.untracked}</span>{/key}{/if}
          </span>
        {/if}
      {/if}
    </div>
    <div class="flex items-center gap-0.5">
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
                <FolderOpen size={12} /> Open in Explorer
              </button>
              <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left" onclick={() => { api.openInVscode(activeSession.projectPath); openDropdownOpen = false; }}>
                <Code size={12} /> Open in VS Code
              </button>
              <button class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left" onclick={() => { api.openInZed(activeSession.projectPath); openDropdownOpen = false; }}>
                <Zap size={12} /> Open in Zed
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
          <div class="relative rounded-2xl border border-border bg-card shadow-sm focus-within:shadow-md focus-within:ring-1 focus-within:ring-ring transition-all" onfocusout={handleHomeFocusOut}>
            <div class="p-4">
              <textarea
                bind:this={homeTextareaRef}
                bind:value={homeInput}
                onkeydown={handleHomeKeydown}
                oninput={detectHomeCompletion}
                onfocus={detectHomeCompletion}
                onpaste={handleHomePaste}
                placeholder="Ask anything... (type @ to reference files, / for commands)"
                rows={3}
                disabled={submitting}
                autofocus
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
            <!-- Home command picker dropdown (floating above input) -->
            {#if showCommands && filteredHomeCommands.length > 0}
              <div bind:this={homeCommandListRef} class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-48 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50">
                {#each filteredHomeCommands as [cmd, desc], i (cmd)}
                  <button
                    type="button"
                    class="flex items-center gap-2 w-full px-3 py-2 text-left text-sm transition-colors {i === selectedCommandIdx ? 'bg-secondary' : 'hover:bg-secondary/50'}"
                    onclick={() => acceptHomeCommand(cmd)}
                  >
                    <Command size={14} class="text-muted-foreground shrink-0" />
                    <span class="font-mono text-primary shrink-0">{cmd}</span>
                    <span class="text-muted-foreground text-xs truncate">{desc}</span>
                  </button>
                {/each}
              </div>
            {/if}
            <!-- Home file picker dropdown (floating above input) -->
            <FilePicker
              show={homeFilePicker.show}
              entries={homeFilePicker.entries}
              selectedIdx={homeFilePicker.selectedIdx}
              query={homeFilePicker.query}
              root={homeFilePicker.root}
              onEnter={onEnterHomeDir}
              onAccept={onAcceptHomeFile}
            />
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
          <div class="flex-1 relative min-h-0">
            <MessageList />
          </div>
          <div class="shrink-0 w-full">
            <div class="container mx-auto px-4 lg:px-6">
              <QueuedInputBar session={activeSession} onEdit={(text) => chatInputRef?.setContent(text)} onSteer={(blocks) => {
                if (!activeSession) return;
                api.sendSteer(activeSession.id, blocks).then(() => {
                  showNotification("Steer message queued for next turn", "info", 3000);
                }).catch((e: unknown) => {
                  console.error("Failed to send steer:", e instanceof Error ? e.message : e);
                  showNotification("Failed to send steer", "error", 3000);
                });
              }} />
              <InfoBar session={activeSession} />
              <PermissionBar />
              <AskUserBar />
              <ChatInput bind:this={chatInputRef} />
            </div>
          </div>
        </div>
      </div>
    {:else if activeSession}
      {@const activeTab = activeSession.tabs.find((t: { id: string }) => t.id === activeSession.activeTabId)}
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

<style>
  @keyframes roll-in {
    0% { transform: translateY(60%); opacity: 0; }
    100% { transform: translateY(0); opacity: 1; }
  }
  .roll-num {
    animation: roll-in 0.25s ease-out;
  }
</style>
