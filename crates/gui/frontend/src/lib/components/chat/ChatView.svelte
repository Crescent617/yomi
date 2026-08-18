<script lang="ts">
  import {
    sessionState,
    projectState,
    getSession,
    getActiveSession,
    showNotification,
    streamingMessages,
  } from "../../state.svelte";
  import {
    activateSession,
    closeTab,
    loadSessionMessages,
    syncSessionStatus,
    refreshCheckpoints,
    appendSessionMessages,
    createSessionState,
  } from "../../session";
  import * as api from "../../api";
  import TabBar from "../layout/TabBar.svelte";
  import MessageList from "./MessageList.svelte";
  import ChatInput from "./ChatInput.svelte";
  import LoadingPlaceholder from "../ui/LoadingPlaceholder.svelte";
  import PopoverPanel from "../ui/PopoverPanel.svelte";
  import FilePreview from "../editor/FilePreview.svelte";
  import FileEditor from "../editor/FileEditor.svelte";
  import HeaderBreadcrumb from "./HeaderBreadcrumb.svelte";
  import SidebarToggle from "../layout/SidebarToggle.svelte";
  import PermissionBar from "./PermissionBar.svelte";
  import AskUserBar from "./AskUserBar.svelte";
  import PendingBar from "./PendingBar.svelte";
  import ModelSelector from "./ModelSelector.svelte";
  import {
    ChevronDown,
    ArrowUp,
    Check,
    ExternalLink,
    Paperclip,
    Plus,
    Search,
    X,
    Code,
    Zap,
    GitBranch,
    FileDiff,
    Command,
    Copy,
    FolderOpen,
    Info,
    Loader2,
    AlertCircle,
  } from "lucide-svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import type { PermissionLevel } from "../../permission";
  import { pickGreeting } from "../home/greeting";
  import TodayUsageCard from "../home/TodayUsageCard.svelte";
  import RecentSessions from "../home/RecentSessions.svelte";
  import { formatTimeAgo } from "../../utils";
  import ProjectDot from "../ui/ProjectDot.svelte";
  import { clock } from "../../clock.svelte";
  import ChangesWorkspace from "../layout/ChangesWorkspace.svelte";
  import PermissionSelector from "./PermissionSelector.svelte";

  import { buildContentBlocks } from "../../types";
  import {
    guiPreferences,
    scheduleGuiPreferencesSave,
  } from "../../settings.svelte";

  let {
    onToggleLeftPanel,
    leftPanelCollapsed,
    leftPanelAttention,
  }: {
    onToggleLeftPanel?: () => void;
    leftPanelCollapsed?: boolean;
    /** Tint the toggle to signal the desktop sidebar is hidden. */
    leftPanelAttention?: boolean;
  } = $props();

  const activeSession = $derived(getActiveSession());

  let showingChanges = $state(false);
  let changesCount = $state(0);
  let changesLoading = $state(false);
  let changesError = $state(false);
  let changesLoadVersion = 0;

  async function loadChangesCount(path: string) {
    const version = ++changesLoadVersion;
    changesLoading = true;
    changesError = false;
    try {
      const [working, staged] = await Promise.all([
        api.getGitDiffSummary(path, false),
        api.getGitDiffSummary(path, true),
      ]);
      if (version !== changesLoadVersion) return;
      changesCount = new Set([
        ...(working ?? []).map((file) => file.path),
        ...(staged ?? []).map((file) => file.path),
      ]).size;
    } catch (error) {
      if (version !== changesLoadVersion) return;
      console.error("Failed to load changes summary:", error);
      changesError = true;
    } finally {
      if (version === changesLoadVersion) changesLoading = false;
    }
  }

  $effect(() => {
    const sessionId = activeSession?.id;
    const path = activeSession?.project_path;
    showingChanges = false;
    if (sessionId && path) {
      void loadChangesCount(path);
    } else {
      changesCount = 0;
      changesError = false;
    }
  });

  $effect(() => {
    const revision = activeSession?.git_refresh_revision;
    const path = activeSession?.project_path;
    if (revision && path) void loadChangesCount(path);
  });

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
  let projectSearch = $state("");
  let projectSearchRef = $state<HTMLInputElement | null>(null);
  let greeting = $state(pickGreeting());
  // Re-roll greeting each time the user returns to the home screen
  $effect(() => {
    if (!sessionState.activeSessionId) {
      greeting = pickGreeting();
    }
  });
  let openDropdownOpen = $state(false);
  let projectDropdownRef = $state<HTMLDivElement | null>(null);
  let homeComposing = $state(false);
  let homeIgnoreNextEnter = $state(false);
  let homeInput = $state("");
  let submitting = $state(false);
  let homeFileAttachments = $state<string[]>([]);

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
  let modelSelectorRef: ReturnType<typeof ModelSelector> | undefined = $state();

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

  // Pick the most recently used project by default when projects load
  $effect(() => {
    if (
      !sessionState.activeSessionId &&
      selectedProjectId === "" &&
      projectState.projects.length > 0
    ) {
      selectedProjectId = sortedProjects[0].id;
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
        permission_level =
          guiPreferences.chat.auto_approve_level ??
          c?.auto_approve ??
          "caution";
      })
      .catch(() => {
        if (cancelled) return;
        permission_level = guiPreferences.chat.auto_approve_level ?? "caution";
      });
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
      showNotification("Please select a project", "error");
      return;
    }

    if (selectedProjectId === "new") {
      const dir = newProjectPath.trim();
      if (!dir) {
        showNotification("Project path is required", "error");
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
          "Failed to create project: " + api.errorMessage(e),
          "error",
        );
        submitting = false;
        return;
      }
    } else {
      const project = projectState.projects.find(
        (p) => p.id === selectedProjectId,
      );
      if (!project) {
        showNotification("Please select a project", "error");
        return;
      }
      project_id = project.id;
      working_dir = project.dir;
      submitting = true;
    }

    try {
      const id = await api.createSession(
        working_dir,
        level,
        project_id,
        // getActiveModel() returns "" until models load — don't persist that
        modelSelectorRef?.getActiveModel() || undefined,
      );
      const result = await api.listSessions(project_id, "all", undefined, 20);
      for (const s of result.sessions) {
        if (!sessionState.sessions.find((sess) => sess.id === s.id)) {
          sessionState.sessions.push(
            createSessionState({
              id: s.id,
              project_path: s.project_path ?? "",
              project_id: s.project_id,
              alias: s.title ?? "Untitled",
              updated_at: s.updated_at ?? s.created_at,
              permission_level: s.auto_approve_level ?? level ?? "caution",
              model_key: s.model_key,
            }),
          );
        }
      }
      const phaseRevisionAtRequest = sessionState.sessions.find(
        (session) => session.id === id,
      )?.phase_revision;
      const sessionInfo = await api.getSession(id);
      const msgs = await api.getMessages(id);
      const session = sessionState.sessions.find((s) => s.id === id);
      if (session && phaseRevisionAtRequest !== undefined) {
        syncSessionStatus(id, sessionInfo, phaseRevisionAtRequest);
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
          appendSessionMessages(session, buf);
          streamingMessages[id] = [];
        }
        await activateSession(id);
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
        const blocks = buildContentBlocks(text, homeInlineImages);
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

  let showSessionInfo = $state(false);
  let infoButtonRef = $state<HTMLButtonElement | null>(null);
  let infoTooltipRef = $state<HTMLDivElement | null>(null);

  // ── Session detail fetch for the info popover ──
  // Fresh from the DB on every open (works for sub-agent sessions too, which
  // never appear in session lists, so their template wouldn't otherwise
  // reach the client).
  type SessionDetail = {
    created_at?: string;
    template?: string | null;
  };
  let sessionDetail = $state<SessionDetail>({});

  $effect(() => {
    if (!showSessionInfo || !activeSession) return;
    const id = activeSession.id;
    sessionDetail = {};
    void api
      .getSession(id)
      .then((info) => {
        if (showSessionInfo && activeSession?.id === id)
          sessionDetail = {
            created_at: info.created_at,
            template: info.template ?? null,
          };
      })
      .catch(() => {});
  });

  // ── Copy session IDs from the info popover (local check-swap feedback) ──
  let copiedIdKey = $state<string | null>(null);
  let copyIdTimer: ReturnType<typeof setTimeout> | undefined;

  async function copyIdToClipboard(id: string, key: string) {
    try {
      await navigator.clipboard.writeText(id);
      clearTimeout(copyIdTimer);
      copiedIdKey = key;
      copyIdTimer = setTimeout(() => (copiedIdKey = null), 1500);
    } catch (e) {
      console.error("Failed to copy:", e);
      showNotification("Failed to copy", "error");
    }
  }

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

  // Projects sorted by most recently updated, filtered by search
  const sortedProjects = $derived.by(() =>
    [...projectState.projects].sort((a, b) =>
      (b.updated_at ?? "").localeCompare(a.updated_at ?? ""),
    ),
  );
  const filteredProjects = $derived.by(() => {
    const q = projectSearch.trim().toLowerCase();
    if (!q) return sortedProjects;
    return sortedProjects.filter(
      (p) =>
        p.name.toLowerCase().includes(q) || p.dir.toLowerCase().includes(q),
    );
  });

  $effect(() => {
    if (projectDropdownOpen) {
      projectSearch = "";
      // focus search box after render
      setTimeout(() => projectSearchRef?.focus(), 0);
    }
  });

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

    // ── command: starts with / ──
    if (homeInput.startsWith("/")) {
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
  }

  function acceptHomeCommand(cmd: string) {
    homeInput = cmd + " ";
    showCommands = false;
    homeTextareaRef?.focus();
  }

  function handleHomeFocusOut(e: FocusEvent) {
    const container = e.currentTarget as HTMLElement;
    if (!container.contains(e.relatedTarget as Node)) {
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
          if (activeSession) {
            void api.openDefault(href).catch((error) => {
              showNotification(
                `Failed to open link: ${api.errorMessage(error)}`,
                "error",
              );
            });
          }
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
    <div
      class="flex h-11 shrink-0 items-center justify-between border-b border-border/70 bg-background/95 px-2"
    >
      <div class="flex h-full min-w-0 items-center gap-2">
        <!-- Left panel toggle -->
        {#if onToggleLeftPanel}
          <SidebarToggle
            open={!leftPanelCollapsed}
            attention={leftPanelAttention ?? false}
            onclick={() => onToggleLeftPanel()}
          />
        {/if}
        <div class="flex min-w-0 flex-1 items-center gap-1.5">
          <HeaderBreadcrumb session={activeSession} />
          <div class="relative flex shrink-0 items-center">
            <button
              type="button"
              bind:this={infoButtonRef}
              class="inline-flex h-5 w-5 items-center justify-center rounded-sm text-muted-foreground transition-colors hover:bg-secondary/80 hover:text-foreground {showSessionInfo
                ? 'bg-secondary/80 text-foreground'
                : ''}"
              onclick={(e) => {
                e.stopPropagation();
                showSessionInfo = !showSessionInfo;
              }}
              aria-label="Session information"
              aria-expanded={showSessionInfo}
            >
              <Info size={13} />
            </button>
            {#if showSessionInfo}
              <PopoverPanel
                bind:ref={infoTooltipRef}
                title="Session information"
                padded
                bodyClass="space-y-3"
                class="absolute left-0 top-full z-50 mt-1.5 w-80"
              >
                {#snippet headerActions()}
                  <span
                    class="inline-flex items-center gap-1 text-[10px] font-medium capitalize text-muted-foreground"
                  >
                    <span
                      class="h-1.5 w-1.5 rounded-full {activeSession.phase ===
                      'idle'
                        ? 'bg-success'
                        : activeSession.phase === 'error'
                          ? 'bg-error'
                          : 'bg-warning'}"
                    ></span>
                    {activeSession.phase}
                  </span>
                {/snippet}
                <div class="space-y-1.5">
                  <div class="flex items-center gap-1.5">
                    <span
                      class="micro-label w-24 shrink-0 text-muted-foreground"
                      >Permission</span
                    >
                    <span
                      class="capitalize {activeSession.permission_level ===
                      'safe'
                        ? 'text-success'
                        : activeSession.permission_level === 'dangerous'
                          ? 'text-error'
                          : 'text-warning'}"
                    >
                      {activeSession.permission_level || "Caution"}
                    </span>
                  </div>
                  <div class="flex items-center gap-1.5">
                    <span
                      class="micro-label w-24 shrink-0 text-muted-foreground"
                      >Directory</span
                    >
                    <span
                      class="min-w-0 truncate rounded-sm bg-code-bg px-1.5 py-0.5 font-mono text-foreground"
                      title={activeSession.project_path}
                    >
                      {activeSession.project_path || "N/A"}
                    </span>
                  </div>
                  <div class="flex items-center gap-1.5">
                    <span
                      class="micro-label w-24 shrink-0 text-muted-foreground"
                      >ID</span
                    >
                    <button
                      type="button"
                      class="flex min-w-0 items-center gap-1.5 rounded-sm bg-code-bg px-1.5 py-0.5 font-mono text-foreground transition-colors hover:bg-secondary/70"
                      title={activeSession.id}
                      aria-label="Copy session ID"
                      onclick={() =>
                        copyIdToClipboard(activeSession.id, "session")}
                    >
                      <span class="truncate">{activeSession.id}</span>
                      {#if copiedIdKey === "session"}
                        <Check size={11} class="shrink-0 text-success" />
                      {:else}
                        <Copy
                          size={11}
                          class="shrink-0 text-muted-foreground"
                        />
                      {/if}
                    </button>
                  </div>
                  {#if activeSession.parent_session_id}
                    <div class="flex items-center gap-1.5">
                      <span
                        class="micro-label w-24 shrink-0 text-muted-foreground"
                        >Parent</span
                      >
                      <button
                        type="button"
                        class="flex min-w-0 items-center gap-1.5 rounded-sm bg-code-bg px-1.5 py-0.5 font-mono text-foreground transition-colors hover:bg-secondary/70"
                        title={activeSession.parent_session_id}
                        aria-label="Copy parent session ID"
                        onclick={() =>
                          copyIdToClipboard(
                            activeSession.parent_session_id!,
                            "parent",
                          )}
                      >
                        <span class="truncate"
                          >{activeSession.parent_session_id}</span
                        >
                        {#if copiedIdKey === "parent"}
                          <Check size={11} class="shrink-0 text-success" />
                        {:else}
                          <Copy
                            size={11}
                            class="shrink-0 text-muted-foreground"
                          />
                        {/if}
                      </button>
                    </div>
                  {/if}
                  {#if activeSession.model_key}
                    <div class="flex items-center gap-1.5">
                      <span
                        class="micro-label w-24 shrink-0 text-muted-foreground"
                        >Model</span
                      >
                      <span
                        class="min-w-0 truncate rounded-sm bg-code-bg px-1.5 py-0.5 font-mono text-foreground"
                        title={activeSession.model_key}
                      >
                        {activeSession.model_key}
                      </span>
                    </div>
                  {/if}
                  {#if sessionDetail.template}
                    <div class="flex items-center gap-1.5">
                      <span
                        class="micro-label w-24 shrink-0 text-muted-foreground"
                        >Template</span
                      >
                      <span
                        class="min-w-0 truncate rounded-sm bg-code-bg px-1.5 py-0.5 font-mono text-foreground"
                        title={sessionDetail.template}
                      >
                        {sessionDetail.template}
                      </span>
                    </div>
                  {/if}
                  <div class="flex items-center gap-1.5">
                    <span
                      class="micro-label w-24 shrink-0 text-muted-foreground"
                      >Updated</span
                    >
                    <span class="min-w-0 text-foreground">
                      {new Date(activeSession.updated_at).toLocaleString()}
                    </span>
                  </div>
                  {#if activeSession.created_at ?? sessionDetail.created_at}
                    <div class="flex items-center gap-1.5">
                      <span
                        class="micro-label w-24 shrink-0 text-muted-foreground"
                        >Created</span
                      >
                      <span class="min-w-0 text-foreground">
                        {new Date(
                          sessionDetail.created_at ?? activeSession.created_at!,
                        ).toLocaleString()}
                      </span>
                    </div>
                  {/if}
                </div>
              </PopoverPanel>
            {/if}
          </div>
          {#if activeSession.git_info?.branch}
            <span class="text-border">·</span>
            <span
              class="inline-flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground"
            >
              <GitBranch size={10} />
              {activeSession.git_info.branch}
            </span>
          {/if}
        </div>
      </div>
      <div class="h-full flex items-center gap-0.5 shrink-0">
        {#if activeSession.project_path}
          {@const gitInfo = activeSession.git_info}
          <button
            type="button"
            onclick={() => (showingChanges = true)}
            class="relative inline-flex h-7 w-7 items-center justify-center rounded-md transition-colors {showingChanges
              ? 'bg-secondary text-foreground'
              : 'text-muted-foreground hover:bg-secondary/80 hover:text-foreground'}"
            title={changesError
              ? "Couldn’t load changes — open to retry"
              : changesLoading
                ? "Loading changes"
                : changesCount > 0
                  ? `${changesCount} changed file${changesCount === 1 ? "" : "s"}${gitInfo ? ` · +${gitInfo.added_lines} −${gitInfo.deleted_lines}` : ""}`
                  : "Working tree clean"}
            aria-label={changesError
              ? "Review changes; summary unavailable"
              : changesCount > 0
                ? `Review changes: ${changesCount} files`
                : "Review changes: working tree clean"}
            aria-pressed={showingChanges}
          >
            {#if changesLoading}
              <Loader2 size={14} class="animate-spin" />
            {:else if changesError}
              <AlertCircle size={14} class="text-error" />
            {:else if changesCount > 0}
              <FileDiff size={15} />
              <span
                class="absolute -right-1 -top-1 inline-flex min-w-4 items-center justify-center rounded-full bg-primary px-1 text-[9px] leading-4 text-primary-foreground"
                >{changesCount}</span
              >
            {:else}
              <Check size={15} class="text-success" />
            {/if}
          </button>
        {/if}
        {#if activeSession.project_path}
          <div class="relative flex h-full items-center">
            <button
              type="button"
              onclick={() => (openDropdownOpen = !openDropdownOpen)}
              class="inline-flex h-7 w-7 items-center justify-center rounded-md hover:bg-secondary/80 transition-colors text-muted-foreground hover:text-foreground"
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
                    void api
                      .openDefault(activeSession.project_path)
                      .catch((error) => {
                        showNotification(
                          `Failed to open project: ${api.errorMessage(error)}`,
                          "error",
                        );
                      });
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
              <button
                type="button"
                aria-label="Close open menu"
                class="fixed inset-0 z-10"
                onclick={() => (openDropdownOpen = false)}
              ></button>
            {/if}
          </div>
        {/if}
      </div>
    </div>
  {/if}

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
      <div
        class="relative flex flex-col items-center justify-center h-full px-6"
      >
        {#if onToggleLeftPanel}
          <!-- Sidebar toggle — aligned with the session header position -->
          <div class="absolute left-2 top-2 z-10">
            <SidebarToggle
              open={!leftPanelCollapsed}
              attention={leftPanelAttention ?? false}
              onclick={() => onToggleLeftPanel()}
            />
          </div>
        {/if}
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
              {greeting}
            </p>
          </div>

          <!-- Input card -->
          <div
            class="relative rounded-lg border border-border bg-card/70 shadow-sm focus-within:shadow-md focus-within:ring-1 focus-within:ring-ring transition-all"
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
                placeholder="Ask anything... (/ for commands)"
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
            <div
              class="px-4 py-3 border-t border-border flex items-center justify-between gap-3"
            >
              <div class="flex items-center gap-3">
                <!-- Project selector -->
                <div class="relative" bind:this={projectDropdownRef}>
                  {#if selectedProjectId !== "" && selectedProjectId !== "new"}
                    {@const sel = projectState.projects.find(
                      (p) => p.id === selectedProjectId,
                    )}
                    <button
                      type="button"
                      onclick={() =>
                        (projectDropdownOpen = !projectDropdownOpen)}
                      class="flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors"
                    >
                      {#if sel}
                        <ProjectDot
                          name={sel.name}
                          dir={sel.dir}
                          class="size-2"
                        />
                      {:else}
                        <FolderOpen class="w-4 h-4" />
                      {/if}
                      <span class="max-w-[140px] truncate"
                        >{sel?.name ?? "Unknown"}</span
                      >
                      <ChevronDown class="w-3 h-3" />
                    </button>
                  {:else}
                    <button
                      type="button"
                      onclick={() =>
                        (projectDropdownOpen = !projectDropdownOpen)}
                      class="flex items-center gap-1.5 text-sm text-muted-foreground hover:text-foreground transition-colors"
                    >
                      <FolderOpen class="w-4 h-4" />
                      <span class="max-w-[140px] truncate">
                        {selectedProjectId === "new"
                          ? "+ New Project"
                          : "Select project"}
                      </span>
                      <ChevronDown class="w-3 h-3" />
                    </button>
                  {/if}
                  {#if projectDropdownOpen}
                    <div
                      class="absolute bottom-full left-0 mb-1 z-50 w-72 rounded-xl border border-border bg-popover shadow-xl overflow-hidden flex flex-col"
                    >
                      {#if projectState.projects.length > 5}
                        <div
                          class="flex items-center gap-2 px-3 py-2 border-b border-border shrink-0"
                        >
                          <Search
                            class="w-3.5 h-3.5 text-muted-foreground shrink-0"
                          />
                          <input
                            bind:this={projectSearchRef}
                            bind:value={projectSearch}
                            type="text"
                            placeholder="Search projects..."
                            class="w-full bg-transparent text-sm focus:outline-none placeholder:text-muted-foreground"
                            onkeydown={(e: KeyboardEvent) => {
                              if (e.key === "Escape") {
                                projectDropdownOpen = false;
                              }
                              if (
                                e.key === "Enter" &&
                                filteredProjects.length > 0
                              ) {
                                selectedProjectId = filteredProjects[0].id;
                                projectDropdownOpen = false;
                              }
                            }}
                          />
                        </div>
                      {/if}
                      <div class="max-h-64 overflow-y-auto">
                        {#each filteredProjects as project (project.id)}
                          <button
                            type="button"
                            onclick={() => {
                              selectedProjectId = project.id;
                              projectDropdownOpen = false;
                            }}
                            class="w-full flex items-center gap-2.5 text-left px-3 py-2 text-sm hover:bg-accent transition-colors {selectedProjectId ===
                            project.id
                              ? 'bg-accent/50'
                              : ''}"
                          >
                            <ProjectDot
                              name={project.name}
                              dir={project.dir}
                              class="size-2"
                            />
                            <span class="flex-1 min-w-0">
                              <span class="flex items-center gap-2 min-w-0">
                                <span class="font-medium truncate"
                                  >{project.name}</span
                                >
                                {#if project.updated_at}
                                  <span
                                    class="ml-auto text-[10px] text-muted-foreground shrink-0"
                                  >
                                    {formatTimeAgo(
                                      project.updated_at,
                                      clock.now,
                                    )}
                                  </span>
                                {/if}
                              </span>
                              <span
                                class="block text-xs text-muted-foreground truncate"
                              >
                                {project.dir}
                              </span>
                            </span>
                            {#if selectedProjectId === project.id}
                              <Check
                                class="w-3.5 h-3.5 text-primary shrink-0"
                              />
                            {/if}
                          </button>
                        {:else}
                          <div
                            class="px-3 py-4 text-center text-xs text-muted-foreground"
                          >
                            No matching projects
                          </div>
                        {/each}
                      </div>
                      <div class="border-t border-border shrink-0"></div>
                      <button
                        type="button"
                        onclick={() => {
                          selectedProjectId = "new";
                          projectDropdownOpen = false;
                        }}
                        class="w-full flex items-center gap-2 text-left px-3 py-2 text-sm hover:bg-accent transition-colors text-primary font-medium shrink-0 {selectedProjectId ===
                        'new'
                          ? 'bg-accent/50'
                          : ''}"
                      >
                        <Plus class="w-3.5 h-3.5" />
                        New Project...
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
                <PermissionSelector
                  value={(permission_level as PermissionLevel) || "caution"}
                  onSelect={(level) => {
                    permission_level = level;
                    guiPreferences.chat.auto_approve_level = level;
                    scheduleGuiPreferencesSave();
                  }}
                />
                <ModelSelector bind:this={modelSelectorRef} />
              </div>

              <button
                type="button"
                onclick={handleHomeSubmit}
                disabled={submitting ||
                  !homeInput.trim() ||
                  selectedProjectId === ""}
                class="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background enabled:bg-primary enabled:text-primary-foreground enabled:hover:bg-primary/90 enabled:active:scale-95 disabled:cursor-not-allowed disabled:bg-secondary disabled:text-muted-foreground"
                aria-label="Send message"
                title="Send message"
              >
                {#if submitting}
                  <span
                    class="w-3.5 h-3.5 border-2 border-primary-foreground/30 border-t-primary-foreground rounded-full animate-spin"
                  ></span>
                {:else}
                  <ArrowUp size={17} strokeWidth={2.25} />
                {/if}
              </button>
            </div>
          </div>

          <!-- New project inputs -->
          {#if selectedProjectId === "new"}
            <div
              class="mt-3 space-y-2 rounded-lg border border-border bg-card p-3 shadow-sm"
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

          <!-- Recent sessions quick-resume -->
          <div class="mt-6">
            <RecentSessions />
          </div>

          <!-- Today usage hero card -->
          <div class="mt-3">
            <TodayUsageCard />
          </div>
        </div>
      </div>
    {:else if showingChanges && activeSession}
      <ChangesWorkspace
        session={activeSession}
        onClose={() => {
          showingChanges = false;
          void loadChangesCount(activeSession.project_path);
        }}
      />
    {:else if activeSession?.active_tab_id === "chat"}
      <div class="flex h-full relative">
        <!-- Main chat area -->
        <div class="@container flex-1 flex flex-col h-full min-w-0 relative">
          <div
            class="flex-1 relative min-h-0"
            onclick={handleChatClick}
            role="presentation"
          >
            <MessageList />
          </div>
          <div class="shrink-0 w-full">
            <div class="mx-auto w-full max-w-4xl px-4 lg:px-6">
              <PendingBar
                session={activeSession}
                onEdit={(text) => chatInputRef?.setContent?.(text)}
              />
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
      <LoadingPlaceholder
        label="Loading session"
        description="Preparing messages and activity."
        class="h-full"
      />
    {/if}
  </div>
</div>
