<script lang="ts">
  import {
    sessionState,
    projectState,
    getSession,
    getActiveSession,
    openBrowser,
    closeBrowser,
    closeTab,
    setActiveSession,
    showNotification,
    loadSessionMessages,
    streamingMessages,
    syncSessionStatus,
    refreshCheckpoints,
  } from "../../state.svelte";
  import * as api from "../../api";
  import { collapseHome } from "../../utils";
  import TabBar from "../layout/TabBar.svelte";
  import MessageList from "./MessageList.svelte";
  import ChatInput from "./ChatInput.svelte";
  import FilePreview from "../editor/FilePreview.svelte";
  import FileEditor from "../editor/FileEditor.svelte";
  import InfoBar from "./InfoBar.svelte";
  import BreadcrumbBar from "./BreadcrumbBar.svelte";
  import PermissionBar from "./PermissionBar.svelte";
  import AskUserBar from "./AskUserBar.svelte";
  import QueuedInputBar from "./QueuedInputBar.svelte";
  import {
    ArrowLeft,
    ChevronDown,
    Send,
    PanelRightOpen,
    PanelRightClose,
    PanelLeftOpen,
    PanelLeftClose,
    ExternalLink,
    Paperclip,
    X,
    Code,
    Zap,
    GitBranch,
    FileDiff,
    Command,
    Globe,
    FolderOpen,
    Info,
  } from "lucide-svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { homeDir } from "@tauri-apps/api/path";
  import {
    levelDescription,
    levelIcon,
    levelColor,
    type PermissionLevel,
  } from "../../permission";
  import { createFilePicker } from "$lib/filePicker.svelte";

  import type { FileEntry } from "../../fs/provider";
  import FilePicker from "../filePicker/FilePicker.svelte";
  import type { TaggedContentBlock } from "../../types";

  let {
    rightPanelCollapsed,
    onToggleRightPanel,
    onToggleLeftPanel,
    leftPanelCollapsed,
  }: {
    rightPanelCollapsed?: boolean;
    onToggleRightPanel?: () => void;
    onToggleLeftPanel?: () => void;
    leftPanelCollapsed?: boolean;
  } = $props();

  const activeSession = $derived(getActiveSession());

  const hasNonChatTabs = $derived(
    activeSession?.tabs.some((t: { type: string }) => t.type !== "chat") ??
      false,
  );

  let selectedProjectId = $state<string | "new">("");
  let newProjectPath = $state("");
  let newProjectName = $state("");
  let permission_level = $state("");
  let chatInputRef: {
    setContent?: (text: string) => void;
    focus?: () => void;
  } | null = $state(null);
  let projectDropdownOpen = $state(false);
  let openDropdownOpen = $state(false);
  let projectDropdownRef = $state<HTMLDivElement | null>(null);
  let homeComposing = $state(false);
  let homeIgnoreNextEnter = $state(false);
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
    HOME_COMMANDS.filter(([cmd]) =>
      cmd.toLowerCase().includes(commandFilter.toLowerCase()),
    ),
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
    homeInlineImages = [
      ...homeInlineImages,
      { id: homeInlineImageCounter, url: base64Url },
    ];
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
    if (
      !sessionState.activeSessionId &&
      selectedProjectId === "" &&
      projectState.projects.length > 0
    ) {
      selectedProjectId = projectState.projects[0].id;
    }
  });

  // Refresh project list on mount
  $effect(() => {
    let cancelled = false;
    api
      .listProjects()
      .then((list) => {
        if (cancelled) return;
        projectState.projects = list.map((p) => ({
          id: p.id,
          name: p.name,
          dir: p.dir,
          created_at: p.created_at,
          updated_at: p.updated_at,
        }));
      })
      .catch(() => {});
    api
      .getConfig()
      .then((c) => {
        if (cancelled) return;
        if (c?.auto_approve) {
          permission_level = c.auto_approve;
        } else {
          permission_level = "caution";
        }
      })
      .catch(() => {
        if (cancelled) return;
        permission_level = "caution";
      });
    homeDir()
      .then((h) => {
        if (!cancelled) homeDirPath = h;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  });

  async function handleHomeSubmit() {
    if (submitting || !homeInput.trim()) return;

    // Validate /goal before clearing any state
    const baseText = homeInput.trim();
    const isGoal =
      baseText.toLowerCase() === "/goal" ||
      baseText.toLowerCase().startsWith("/goal ");
    if (isGoal) {
      const description = baseText.slice(5).trim();
      if (!description) {
        showNotification(
          "Please provide a goal description: /goal <description>",
          "error",
          5000,
        );
        return;
      }
    }

    let level = permission_level;
    if (!level) {
      try {
        const c = await api.getConfig();
        level = c.auto_approve || "caution";
      } catch {
        level = "caution";
      }
    }

    let project_id: string | undefined;
    let working_dir: string;

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
        const project = await api.createProject(
          dir,
          newProjectName.trim() || undefined,
        );
        project_id = project.id;
        working_dir = project.dir;
        projectState.projects.push({
          id: project.id,
          name: project.name,
          dir: project.dir,
          created_at: project.created_at,
          updated_at: project.updated_at,
        });
      } catch (e: unknown) {
        console.error(
          "Failed to create project:",
          e instanceof Error ? e.message : e,
        );
        showNotification(
          "Failed to create project: " + (e instanceof Error ? e.message : ""),
          "error",
          5000,
        );
        submitting = false;
        return;
      }
    } else {
      const project = projectState.projects.find(
        (p) => p.id === selectedProjectId,
      );
      if (!project) {
        showNotification("Please select a project", "error", 3000);
        return;
      }
      project_id = project.id;
      working_dir = project.dir;
      submitting = true;
    }

    try {
      const id = await api.createSession(working_dir, level, project_id);
      const result = await api.listSessions(project_id, undefined, 20);
      for (const s of result.sessions) {
        if (!sessionState.sessions.find((sess) => sess.id === s.id)) {
          sessionState.sessions.push({
            id: s.id,
            project_path: s.project_path ?? "",
            project_id: s.project_id,
            alias: s.title ?? "Untitled",
            messages: [],
            phase: "idle",
            is_running: false,
            unread: 0,
            checkpoints: [],
            tabs: [{ id: "chat", type: "chat", label: "Chat", pinned: true }],
            active_tab_id: "chat",
            pending_permissions: [],
            pending_ask_user: null,
            queued_input: null,
            updated_at: s.updated_at ?? s.created_at,
            permission_level: s.auto_approve_level ?? level ?? "caution",
            goal: null,
          });
        }
      }
      setActiveSession(id);
      await api.subscribe(id);
      const sessionInfo = await api.getSession(id);
      const msgs = await api.getMessages(id);
      const session = sessionState.sessions.find((s) => s.id === id);
      if (session) {
        syncSessionStatus(id, sessionInfo);
        loadSessionMessages(id, msgs);
        refreshCheckpoints(id);
        api
          .getGoal(id)
          .then((g) => {
            session.goal = g;
          })
          .catch(() => {
            session.goal = null;
          });
        const buf = streamingMessages[id] ?? [];
        if (buf.length > 0) {
          session.messages = [...session.messages, ...buf];
          streamingMessages[id] = [];
        }
      }
      // Send the home input
      homeInput = "";
      const fileSuffix =
        homeFileAttachments.length > 0
          ? "\n" + homeFileAttachments.map((p) => `[File: ${p}]`).join("\n")
          : "";
      const text = baseText + fileSuffix;
      homeFileAttachments = [];
      const hasImages = homeInlineImages.length > 0;
      if (isGoal) {
        const description = baseText.slice(5).trim();
        await api.startGoal(id, description);
        {
          const session = sessionState.sessions.find((s) => s.id === id);
          if (session) {
            api
              .getGoal(id)
              .then((g) => {
                session.goal = g;
              })
              .catch(() => {});
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
      console.error(
        "Failed to create session:",
        e instanceof Error ? e.message : e,
      );
      showNotification(
        "Failed to create session: " +
          (e instanceof Error ? e.message : "Unknown error"),
        "error",
        5000,
      );
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
    activeSession.active_tab_id = id;
  }

  function handleCloseTab(id: string) {
    if (!activeSession) return;
    closeTab(activeSession, id);
  }

  let editingTitle = $state(false);
  let titleValue = $state("");
  let showSessionInfo = $state(false);
  let infoButtonRef = $state<HTMLButtonElement | null>(null);
  let infoTooltipRef = $state<HTMLDivElement | null>(null);

  // Close session info tooltip when clicking outside
  $effect(() => {
    if (!showSessionInfo) return;
    function onDocClick(e: MouseEvent) {
      const target = e.target as Node;
      if (
        infoButtonRef &&
        !infoButtonRef.contains(target) &&
        infoTooltipRef &&
        !infoTooltipRef.contains(target)
      ) {
        showSessionInfo = false;
      }
    }
    document.addEventListener("click", onDocClick);
    return () => document.removeEventListener("click", onDocClick);
  });

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
      console.error(
        "Failed to rename session:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to rename session", "error", 3000);
    } finally {
      editingTitle = false;
    }
  }

  // ── Git info sync ──
  async function syncGitInfo(session_id: string, project_path: string) {
    try {
      const info = await api.getGitInfo(project_path);
      const session = getSession(session_id);
      if (session && session.id === activeSession?.id) {
        session.git_info = info;
      }
    } catch {
      const session = getSession(session_id);
      if (session && session.id === activeSession?.id) {
        session.git_info = null;
      }
    }
  }

  // Refresh immediately when active session changes
  $effect(() => {
    const session = activeSession;
    if (!session?.project_path) return;
    syncGitInfo(session.id, session.project_path);
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
        const root =
          projectState.projects.find((p) => p.id === selectedProjectId)?.dir ||
          "";
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
    const after = homeInput.slice(
      homeTextareaRef?.selectionStart ?? homeInput.length,
    );
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

  function handleChatClick(e: MouseEvent) {
    let node: Node | null = e.target as Node;
    const container = e.currentTarget as HTMLElement;
    while (node && node !== container) {
      if (node.nodeName === "A") {
        const anchor = node as HTMLAnchorElement;
        const href = anchor.getAttribute("href");
        if (
          href &&
          (href.startsWith("http://") ||
            href.startsWith("https://") ||
            href.startsWith("mailto:"))
        ) {
          e.preventDefault();
          e.stopPropagation();
          if (activeSession) openBrowser(activeSession.id, href);
        }
        return;
      }
      node = node.parentNode;
    }
  }

  function handleHomeKeydown(e: KeyboardEvent) {
    // Ignore key events while IME is composing or right after composition ends
    if (e.isComposing || homeComposing) {
      return;
    }

    // Command picker navigation
    if (showCommands) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        if (filteredHomeCommands.length === 0) return;
        selectedCommandIdx =
          (selectedCommandIdx + 1) % filteredHomeCommands.length;
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (filteredHomeCommands.length === 0) return;
        selectedCommandIdx =
          (selectedCommandIdx - 1 + filteredHomeCommands.length) %
          filteredHomeCommands.length;
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
        if (
          entry &&
          !entry.isDirectory &&
          (e.key === "Enter" || e.key === "Tab")
        ) {
          onAcceptHomeFile(entry);
        }
        return;
      }
    }
    if (e.key === "Enter" && !e.shiftKey) {
      // If this Enter is right after IME composition ends, ignore it
      if (homeIgnoreNextEnter) {
        homeIgnoreNextEnter = false;
        e.preventDefault();
        return;
      }
      e.preventDefault();
      handleHomeSubmit();
    }
  }
</script>

<div class="flex-1 flex flex-col min-w-0 overflow-hidden">
  {#if activeSession}
    <!-- Header -->
    <div class="flex items-center justify-between p-2 border-b border-border">
      <div class="flex items-center gap-1 min-w-0">
        <!-- Left panel toggle -->
        {#if onToggleLeftPanel}
          <button
            type="button"
            onclick={() => onToggleLeftPanel()}
            class="p-1.5 rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground"
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
            onkeydown={(e: KeyboardEvent) => {
              if (e.key === "Enter") confirmRenameTitle();
              if (e.key === "Escape") editingTitle = false;
            }}
            onblur={() => confirmRenameTitle()}
            class="flex-1 min-w-0 bg-background border border-border rounded px-2 py-0.5 text-sm font-medium focus:outline-none focus:ring-1 focus:ring-ring"
            autofocus
          />
        {:else}
          <span
            class="font-medium truncate cursor-pointer hover:text-primary transition-colors"
            title={activeSession.alias ?? activeSession.id.slice(-8)}
            ondblclick={() => {
              editingTitle = true;
              titleValue = activeSession.alias ?? activeSession.id.slice(-8);
            }}
            role="button"
            tabindex="0"
          >
            {activeSession.alias ?? activeSession.id.slice(-8)}
          </span>
          <div class="group relative">
            <button
              type="button"
              bind:this={infoButtonRef}
              class="inline-flex items-center justify-center rounded p-0.5 hover:bg-secondary/80 transition-colors {showSessionInfo ? 'bg-secondary/80' : ''}"
              onclick={(e) => {
                e.stopPropagation();
                showSessionInfo = !showSessionInfo;
              }}
            >
              <Info class="w-3.5 h-3.5 text-muted-foreground opacity-60" />
            </button>
            {#if showSessionInfo}
              <div
                bind:this={infoTooltipRef}
                class="absolute left-full top-0 ml-2 z-50"
              >
                <div
                  class="absolute left-0 top-3 w-2 h-2 bg-card rotate-45 border-l border-b border-border/20 -translate-x-[3px]"
                ></div>
                <div
                  class="relative p-3 w-80 bg-card rounded-xl border border-border/20 shadow-xl overflow-visible"
                >
                  <div
                    class="text-[11px] font-medium text-foreground mb-2 pb-1.5 border-b border-border/20"
                  >
                    Session Info
                  </div>
                  <div class="grid grid-cols-[3.5rem_1fr] gap-x-3 gap-y-1 text-[11px]">
                    <span class="text-muted-foreground text-left">ID</span>
                    <span class="text-foreground font-mono text-left break-all">
                      {activeSession.id}
                    </span>
                    <span class="text-muted-foreground text-left">Title</span>
                    <span class="text-foreground text-left break-words">
                      {activeSession.alias || "Untitled"}
                    </span>
                    <span class="text-muted-foreground text-left">Phase</span>
                    <span class="text-foreground text-left">{activeSession.phase}</span>
                    {#if activeSession.parent_session_id}
                      <span class="text-muted-foreground text-left">Parent</span>
                      <span class="text-foreground font-mono text-left break-all">
                        {activeSession.parent_session_id}
                      </span>
                    {/if}
                    <span class="text-muted-foreground text-left">Messages</span>
                    <span class="text-foreground text-left">{activeSession.messages.length}</span>
                    <span class="text-muted-foreground text-left">Working Dir</span>
                    <span class="text-foreground text-left break-all">
                      {activeSession.project_path || "N/A"}
                    </span>
                    <span class="text-muted-foreground text-left">Updated</span>
                    <span class="text-foreground text-left">
                      {new Date(activeSession.updated_at).toLocaleString()}
                    </span>
                    {#if activeSession.permission_level}
                      <span class="text-muted-foreground text-left">Permission</span>
                      <span class="text-foreground text-left">{activeSession.permission_level}</span>
                    {/if}
                  </div>
                </div>
              </div>
            {/if}
          </div>
        {/if}
        {#if activeSession.project_path}
          {@const displayPath = collapseHome(
            activeSession.project_path,
            homeDirPath,
          )}
          <span
            class="text-xs text-muted-foreground truncate"
            title={activeSession.project_path}>{displayPath}</span
          >
        {/if}
        {#if activeSession.git_info?.branch}
          <span
            class="inline-flex items-center gap-1 text-xs text-muted-foreground/80 bg-muted rounded px-1.5 py-0.5 ml-1"
          >
            <GitBranch size={10} />
            {activeSession.git_info.branch}
          </span>
          {@const g = activeSession.git_info}
          {#if g.added_lines > 0 || g.deleted_lines > 0 || g.untracked > 0}
            <span
              class="inline-flex items-center gap-1 text-xs text-muted-foreground/70 font-mono bg-muted rounded px-1.5 py-0.5 ml-1"
            >
              <FileDiff size={10} class="text-muted-foreground/50 shrink-0" />
              {#if g.added_lines > 0}{#key g.added_lines}<span
                    class="roll-num text-green-700/80 dark:text-green-400/80"
                    >+{g.added_lines}</span
                  >{/key}{/if}
              {#if g.deleted_lines > 0}{#key g.deleted_lines}<span
                    class="roll-num text-red-700/80 dark:text-red-400/80"
                    >-{g.deleted_lines}</span
                  >{/key}{/if}
              {#if g.untracked > 0}{#key g.untracked}<span
                    class="roll-num text-slate-500 dark:text-slate-400"
                    >?{g.untracked}</span
                  >{/key}{/if}
            </span>
          {/if}
        {/if}
      </div>
      <div class="flex items-center gap-0.5">
        {#if activeSession.project_path}
          <div class="relative">
            <button
              type="button"
              onclick={() => (openDropdownOpen = !openDropdownOpen)}
              class="p-1.5 rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground"
              title="Open project"
            >
              <ExternalLink size={16} />
            </button>
            {#if openDropdownOpen}
              <div
                class="absolute right-0 top-full mt-1 z-20 w-40 rounded-md border border-border bg-popover shadow-md py-1"
              >
                <button
                  class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                  onclick={() => {
                    api.openInExplorer(activeSession.project_path);
                    openDropdownOpen = false;
                  }}
                >
                  <FolderOpen size={12} /> Open in Explorer
                </button>
                <button
                  class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                  onclick={() => {
                    api.openInVscode(activeSession.project_path);
                    openDropdownOpen = false;
                  }}
                >
                  <Code size={12} /> Open in VS Code
                </button>
                <button
                  class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-foreground hover:bg-secondary/50 text-left"
                  onclick={() => {
                    api.openInZed(activeSession.project_path);
                    openDropdownOpen = false;
                  }}
                >
                  <Zap size={12} /> Open in Zed
                </button>
              </div>
              <div
                class="fixed inset-0 z-10"
                onclick={() => (openDropdownOpen = false)}
              ></div>
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
      active_tab_id={activeSession.active_tab_id}
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
            <img
              src="/yomi-dark.png"
              alt="Yomi"
              class="w-24 h-24 mx-auto mb-3 object-contain hidden dark:block"
            />
            <img
              src="/yomi-light.png"
              alt="Yomi"
              class="w-24 h-24 mx-auto mb-3 object-contain dark:hidden"
            />
            <p class="text-muted-foreground text-lg">
              What can I help you with today?
            </p>
          </div>

          <!-- Input card -->
          <div
            class="relative rounded-2xl border border-border bg-card shadow-sm focus-within:shadow-md focus-within:ring-1 focus-within:ring-ring transition-all"
            onfocusout={handleHomeFocusOut}
          >
            <div class="p-4">
              <textarea
                bind:this={homeTextareaRef}
                bind:value={homeInput}
                onkeydown={handleHomeKeydown}
                oninput={detectHomeCompletion}
                onfocus={detectHomeCompletion}
                onpaste={handleHomePaste}
                oncompositionstart={() => (homeComposing = true)}
                oncompositionend={() => {
                  homeComposing = false;
                  homeIgnoreNextEnter = true;
                  setTimeout(() => (homeIgnoreNextEnter = false), 100);
                }}
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
                    <div
                      class="flex items-center gap-1.5 rounded-md border border-border bg-secondary px-2 py-0.5"
                    >
                      <span
                        class="text-xs text-muted-foreground truncate max-w-[200px]"
                        >{path.split("/").pop()}</span
                      >
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
              <div
                bind:this={homeCommandListRef}
                class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-48 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50"
              >
                {#each filteredHomeCommands as [cmd, desc], i (cmd)}
                  <button
                    type="button"
                    class="flex items-center gap-2 w-full px-3 py-2 text-left text-sm transition-colors {i ===
                    selectedCommandIdx
                      ? 'bg-secondary'
                      : 'hover:bg-secondary/50'}"
                    onclick={() => acceptHomeCommand(cmd)}
                  >
                    <Command size={14} class="text-muted-foreground shrink-0" />
                    <span class="font-mono text-primary shrink-0">{cmd}</span>
                    <span class="text-muted-foreground text-xs truncate"
                      >{desc}</span
                    >
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
            <div
              class="px-4 py-3 border-t border-border flex items-center justify-between gap-3"
            >
              <div class="flex items-center gap-3">
                <!-- Project selector -->
                <div class="relative" bind:this={projectDropdownRef}>
                  <button
                    type="button"
                    onclick={() => (projectDropdownOpen = !projectDropdownOpen)}
                    class="flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors"
                  >
                    <FolderOpen class="w-4 h-4" />
                    <span class="max-w-[140px] truncate">
                      {#if selectedProjectId === ""}
                        Select project
                      {:else if selectedProjectId === "new"}
                        + New Project
                      {:else}
                        {projectState.projects.find(
                          (p) => p.id === selectedProjectId,
                        )?.name ?? "Unknown"}
                      {/if}
                    </span>
                    <ChevronDown class="w-3 h-3" />
                  </button>
                  {#if projectDropdownOpen}
                    <div
                      class="absolute bottom-full left-0 mb-1 z-50 w-56 rounded-lg border border-border bg-popover shadow-lg overflow-hidden max-h-60 overflow-y-auto"
                    >
                      {#each projectState.projects as project (project.id)}
                        <button
                          type="button"
                          onclick={() => {
                            selectedProjectId = project.id;
                            projectDropdownOpen = false;
                          }}
                          class="w-full text-left px-3 py-2 text-sm hover:bg-accent transition-colors {selectedProjectId ===
                          project.id
                            ? 'bg-accent/50'
                            : ''}"
                        >
                          <div class="font-medium">{project.name}</div>
                          <div class="text-xs text-muted-foreground truncate">
                            {project.dir}
                          </div>
                        </button>
                      {/each}
                      <div class="border-t border-border"></div>
                      <button
                        type="button"
                        onclick={() => {
                          selectedProjectId = "new";
                          projectDropdownOpen = false;
                        }}
                        class="w-full text-left px-3 py-2 text-sm hover:bg-accent transition-colors text-primary font-medium {selectedProjectId ===
                        'new'
                          ? 'bg-accent/50'
                          : ''}"
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
                  {#each ["safe", "caution", "dangerous"] as PermissionLevel[] as level (level)}
                    {@const Icon = levelIcon(level)}
                    <button
                      type="button"
                      onclick={() => (permission_level = level)}
                      class="p-1 rounded transition-colors {permission_level ===
                      level
                        ? levelColor(level)
                        : 'text-muted-foreground hover:text-foreground'}"
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
                disabled={submitting ||
                  !homeInput.trim() ||
                  selectedProjectId === ""}
                class="inline-flex items-center justify-center rounded-lg bg-primary text-primary-foreground h-8 w-8 hover:bg-primary/90 disabled:opacity-50 shrink-0 transition-colors"
              >
                {#if submitting}
                  <span
                    class="w-3.5 h-3.5 border-2 border-primary-foreground/30 border-t-primary-foreground rounded-full animate-spin"
                  ></span>
                {:else}
                  <Send class="w-4 h-4" />
                {/if}
              </button>
            </div>
          </div>

          <!-- New project inputs -->
          {#if selectedProjectId === "new"}
            <div
              class="mt-3 space-y-2 rounded-xl border border-border bg-card p-3 shadow-sm"
            >
              <div class="flex gap-2">
                <div class="relative flex-1">
                  <FolderOpen
                    class="absolute left-3 top-1/2 -translate-y-1/2 w-4 h-4 text-muted-foreground"
                  />
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
    {:else if activeSession?.active_tab_id === "chat"}
      <div class="flex h-full relative">
        <!-- Main chat area -->
        <div class="flex-1 flex flex-col h-full min-w-0 relative">
          <!-- Browser overlay -->
          {#if activeSession?.browserUrl}
            <div class="absolute inset-0 z-50 flex flex-col bg-background">
              <div
                class="flex items-center gap-2 px-3 py-2 border-b border-border bg-muted/30 shrink-0"
              >
                <button
                  type="button"
                  onclick={() => {
                    if (activeSession) closeBrowser(activeSession.id);
                  }}
                  class="flex items-center gap-1 px-2 py-1 rounded-md text-sm font-medium text-foreground hover:bg-secondary/80 transition-colors"
                >
                  <ArrowLeft class="w-4 h-4" />
                  Back
                </button>
                <div class="flex items-center gap-1 flex-1 min-w-0">
                  <Globe class="w-3.5 h-3.5 text-muted-foreground shrink-0" />
                  <span class="truncate text-sm text-muted-foreground"
                    >{activeSession.browserUrl}</span
                  >
                </div>
              </div>
              <iframe
                src={activeSession.browserUrl}
                class="flex-1 w-full border-0"
                title="Browser"
                sandbox="allow-scripts allow-same-origin allow-popups"
              ></iframe>
            </div>
          {/if}
          <div class="flex-1 relative min-h-0" onclick={handleChatClick}>
            {#if activeSession}
              <BreadcrumbBar session={activeSession} />
            {/if}
            <MessageList />
          </div>
          <div class="shrink-0 w-full">
            <div class="container mx-auto px-4 lg:px-6">
              <QueuedInputBar
                session={activeSession}
                onEdit={(text) => chatInputRef?.setContent?.(text)}
                onSteer={(blocks) => {
                  if (!activeSession) return;
                  api
                    .sendSteer(activeSession.id, blocks)
                    .then(() => {
                      showNotification(
                        "Steer message queued for next step",
                        "info",
                        3000,
                      );
                    })
                    .catch((e: unknown) => {
                      console.error(
                        "Failed to send steer:",
                        e instanceof Error ? e.message : e,
                      );
                      showNotification("Failed to send steer", "error", 3000);
                    });
                }}
              />
              <InfoBar session={activeSession} />
              <PermissionBar />
              <AskUserBar />
              <ChatInput bind:this={chatInputRef} />
            </div>
          </div>
        </div>
      </div>
    {:else if activeSession}
      {@const activeTab = activeSession.tabs.find(
        (t: { id: string }) => t.id === activeSession.active_tab_id,
      )}
      {@const fileEntry = activeTab?.entry
        ? {
            name: activeTab.entry.name,
            path: activeTab.entry.path,
            isDirectory: activeTab.entry.is_directory,
            isFile: !activeTab.entry.is_directory,
          }
        : null}
      {#if activeTab?.type === "preview" && fileEntry}
        <div class="flex h-full relative container mx-auto">
          <div class="flex-1 min-w-0">
            <FilePreview
              entry={fileEntry}
              onEdit={(_e) => {
                /* TODO: open edit tab */
              }}
              onAskAI={(_path) => {
                /* TODO: send to chat */
              }}
            />
          </div>
        </div>
      {:else if activeTab?.type === "edit" && fileEntry}
        <div class="flex h-full relative container mx-auto">
          <div class="flex-1 min-w-0">
            <FileEditor entry={fileEntry} />
          </div>
        </div>
      {/if}
    {:else}
      <div
        class="flex items-center justify-center h-full text-muted-foreground"
      >
        Loading session...
      </div>
    {/if}
  </div>
</div>

<style>
  @keyframes roll-in {
    0% {
      transform: translateY(60%);
      opacity: 0;
    }
    100% {
      transform: translateY(0);
      opacity: 1;
    }
  }
  .roll-num {
    animation: roll-in 0.25s ease-out;
  }
</style>
