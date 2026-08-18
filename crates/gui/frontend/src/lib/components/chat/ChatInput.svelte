<script lang="ts">
  import {
    ArrowUp,
    Command,
    Square,
    Clock,
    Paperclip,
    X,
    Wrench,
  } from "lucide-svelte";
  import type { PermissionLevel } from "../../permission";
  import * as api from "../../api";
  import { buildContentBlocks } from "../../types";
  import {
    sessionState,
    getActiveSession,
    showNotification,
    inputDrafts,
  } from "../../state.svelte";
  import { enqueue, queueHead, steerQueueHead } from "../../mailbox.svelte";
  import { forkSession, textFromBlocks } from "../../session";
  import { isActiveSessionPhase } from "../../session-phase";
  import { SLASH_COMMANDS } from "../../commands";
  import { blockPuaInput } from "../../utils";
  import { open } from "@tauri-apps/plugin-dialog";

  import ModelSelector from "./ModelSelector.svelte";
  import PermissionSelector from "./PermissionSelector.svelte";

  let content = $state("");
  let textareaRef: HTMLTextAreaElement | null = $state(null);
  let composing = $state(false);
  let ignoreNextEnter = $state(false);

  // ── command completion ──
  let showCommands = $state(false);
  let commandFilter = $state("");
  let selectedCommandIdx = $state(0);
  let commandListRef: HTMLDivElement | null = $state(null);

  // ── skill completion (/skill:) ──
  let showSkills = $state(false);
  let skillFilter = $state("");
  let selectedSkillIdx = $state(0);
  let skillListRef: HTMLDivElement | null = $state(null);
  let availableSkills = $state<api.SkillInfo[]>([]);
  let skillsLoadedForSessionId = $state<string | null>(null);

  // ── history picker ──
  let showHistory = $state(false);
  let selectedHistoryIdx = $state(0);
  let historyListRef: HTMLDivElement | null = $state(null);
  let prevSessionId = $state<string | null>(null);

  // ── inline image attachments (paste / drop) ──
  interface InlineImage {
    id: number;
    url: string; // base64 data URL
  }
  let inlineImages = $state<InlineImage[]>([]);
  let inlineImageCounter = $state(0);

  const activeSession = $derived(getActiveSession());
  const isStreaming = $derived(
    activeSession ? isActiveSessionPhase(activeSession.phase) : false,
  );

  let context_window = $state(0);
  const total_tokens = $derived(activeSession?.token_usage?.total_tokens ?? 0);
  const context_percent = $derived(
    context_window > 0 && total_tokens > 0
      ? (total_tokens / context_window) * 100
      : null,
  );
  const context_progress_percent = $derived(
    context_percent === null ? 0 : Math.min(context_percent, 100),
  );
  const context_segments = $derived(
    context_progress_percent === 0
      ? 0
      : Math.max(1, Math.ceil(context_progress_percent / 20)),
  );
  const context_title = $derived(
    context_percent !== null
      ? `${total_tokens.toLocaleString()} of ${context_window.toLocaleString()} tokens used (${context_percent.toFixed(1)}%) · ${Math.max(context_window - total_tokens, 0).toLocaleString()} remaining`
      : "",
  );

  // detect completion triggers
  function detectCompletion() {
    if (!textareaRef) return;
    if (showHistory) return; // Don't interfere with history search

    // ── skill: starts with /skill: ──
    if (content.startsWith("/skill:")) {
      showCommands = false;
      const query = content.slice(7);
      const valid = /^[a-zA-Z0-9_\-:]*$/.test(query);
      if (valid) {
        showSkills = true;
        skillFilter = query;
        selectedSkillIdx = 0;
        loadSkillsForActiveSession();
      } else {
        showSkills = false;
      }
      return;
    }
    showSkills = false;
    skillFilter = "";

    // ── command: starts with / ──
    if (content.startsWith("/")) {
      const query = content.slice(1);
      const valid = /^[a-zA-Z0-9_\-:]*$/.test(query);
      if (valid) {
        showCommands = true;
        commandFilter = query;
        selectedCommandIdx = 0;
      } else {
        showCommands = false;
      }
    } else {
      showCommands = false;
    }
  }

  async function loadSkillsForActiveSession() {
    const session_id = sessionState.activeSessionId;
    if (!session_id || skillsLoadedForSessionId === session_id) return;
    try {
      availableSkills = await api.listSessionSkills(session_id);
      skillsLoadedForSessionId = session_id;
    } catch (e) {
      console.error(
        "Failed to load session skills:",
        e instanceof Error ? e.message : e,
      );
      availableSkills = [];
    }
  }

  const filteredCommands = $derived.by(() => {
    const q = commandFilter.toLowerCase();
    return SLASH_COMMANDS.filter(([cmd]) => cmd.toLowerCase().includes(q));
  });

  const filteredSkills = $derived.by(() => {
    const q = skillFilter.toLowerCase();
    return availableSkills.filter(
      (s) =>
        s.name.toLowerCase().includes(q) ||
        s.description.toLowerCase().includes(q),
    );
  });

  const historyEntries = $derived.by(() => {
    if (!showHistory) return [];
    const session = activeSession;
    if (!session) return [];
    const userMsgs = session.messages
      .filter((m) => m.type === "user")
      .map((m) => textFromBlocks(m.content))
      .filter((c) => c.trim());
    // Deduplicate and reverse (newest first)
    const seen = new Set<string>();
    const unique: string[] = [];
    for (let i = userMsgs.length - 1; i >= 0; i--) {
      const c = userMsgs[i].trim();
      if (!seen.has(c)) {
        seen.add(c);
        unique.push(c);
      }
    }
    const q = content.toLowerCase();
    if (!q) return unique;
    return unique.filter((c) => c.toLowerCase().includes(q));
  });

  // ── actions ──
  export function focus() {
    requestAnimationFrame(() => {
      textareaRef?.focus();
    });
  }

  export function setContent(text: string) {
    content = text;
    clearInlineImages();
    fileAttachments = [];
    requestAnimationFrame(autoResize);
  }

  /** Steer the queued message into the current run. */
  async function steerQueued(sessionId: string) {
    try {
      const sent = await steerQueueHead(sessionId);
      if (sent) {
        showNotification("Steer message queued for next step", "info");
      }
    } catch {
      showNotification("Failed to send steer", "error");
    }
  }

  async function queueInput() {
    if (isSending) return;
    const session = activeSession;
    if (!session || !content.trim()) return;
    const text = content.trim();
    try {
      const queued = await enqueue(
        session.id,
        text,
        inlineImages.length > 0
          ? buildContentBlocks(text, inlineImages)
          : undefined,
      );
      if (!queued) {
        showNotification(
          "A message is already queued — edit or cancel it first",
          "warning",
        );
        return;
      }
      content = "";
      clearInlineImages();
      fileAttachments = [];
      autoResize();
    } catch {
      showNotification("Failed to queue the message", "error");
    }
  }

  function openHistoryPicker() {
    showHistory = true;
    selectedHistoryIdx = 0;
    content = "";
    autoResize();
  }

  function acceptCommand(cmd: string) {
    if (cmd === "/history") {
      openHistoryPicker();
      showCommands = false;
      textareaRef?.focus();
      return;
    }
    content = cmd + " ";
    showCommands = false;
    textareaRef?.focus();
    requestAnimationFrame(autoResize);
  }

  function acceptSkill(name: string) {
    content = "/skill:" + name + " ";
    showSkills = false;
    skillFilter = "";
    textareaRef?.focus();
    requestAnimationFrame(autoResize);
  }

  async function runNotificationDebug() {
    const types = ["info", "success", "warning", "error"] as const;

    for (let index = 1; index <= 10; index += 1) {
      if (index > 1) {
        const delay = 100 + Math.floor(Math.random() * 351);
        await new Promise<void>((resolve) => window.setTimeout(resolve, delay));
      }

      const type = types[Math.floor(Math.random() * types.length)];
      showNotification(`Debug notification ${index}/10`, type);
    }
  }

  async function handleCommand(text: string): Promise<boolean> {
    const session_id = sessionState.activeSessionId;
    if (!session_id) return false;

    const parts = text.split(/\s+/);
    const cmd = parts[0].toLowerCase();

    try {
      switch (cmd) {
        case "/debug":
          if (parts[1]?.toLowerCase() !== "noti") {
            showNotification(
              "Unknown debug command. Try /debug noti",
              "warning",
            );
            return false;
          }
          void runNotificationDebug();
          break;
        case "/cancel":
          await api.cancelSession(session_id);
          showNotification("Session cancelled", "info");
          break;
        case "/clear":
          await api.clearSession(session_id);
          showNotification("Session context cleared", "info");
          break;
        case "/yolo":
          await api.setPermissionLevel(session_id, "dangerous");
          showNotification(
            "YOLO mode enabled — all tools will be auto-approved",
            "info",
          );
          break;
        case "/undo":
          {
            const checkpoints = await api.getCheckpoints(session_id);
            if (checkpoints.length < 1) {
              showNotification("No checkpoint to undo", "error");
              return false;
            }
            const target = checkpoints[checkpoints.length - 1];
            await api.rewind(session_id, target.message_id);
            showNotification("Undo last turn", "info");
          }
          break;
        case "/safe":
          await api.setPermissionLevel(session_id, "safe");
          showNotification("Permission level set to Safe", "info");
          break;
        case "/caution":
          await api.setPermissionLevel(session_id, "caution");
          showNotification("Permission level set to Caution", "info");
          break;
        case "/compact":
          await api.compactSession(session_id);
          showNotification("Session compaction requested", "info");
          break;
        case "/steer":
          {
            const steerText = parts.slice(1).join(" ").trim();
            if (!steerText && inlineImages.length === 0) {
              showNotification(
                "Please provide steer content: /steer <content>",
                "error",
              );
              return false;
            }
            const blocks = buildContentBlocks(steerText, inlineImages);
            await api.sendSteer(session_id, blocks);
            clearInlineImages();
            showNotification("Steer message queued for next step", "info");
          }
          break;
        case "/fork":
          {
            try {
              const fork = await forkSession(session_id);
              showNotification(`Forked session: ${fork.id}`, "success");
            } catch (e) {
              showNotification(
                `Fork failed: ${e instanceof Error ? e.message : String(e)}`,
                "error",
              );
              return false;
            }
          }
          break;
        case "/continue":
          {
            try {
              await api.continueSession(session_id);
              showNotification("Agent continuing...", "info");
            } catch (e) {
              showNotification(
                `Continue failed: ${e instanceof Error ? e.message : String(e)}`,
                "error",
              );
              return false;
            }
          }
          break;
        case "/goal:stop":
          await api.stopGoal(session_id);
          {
            const session = getActiveSession();
            if (session) {
              api
                .getGoal(session_id)
                .then((g) => {
                  session.goal = g;
                })
                .catch(() => {
                  session.goal = null;
                });
            }
          }
          console.log("Goal mode stopped");
          break;
        case "/goal":
          {
            const description = parts.slice(1).join(" ").trim();
            if (!description) {
              showNotification(
                "Please provide a goal description: /goal <description>",
                "error",
              );
              return false;
            }
            await api.startGoal(session_id, description);
            {
              const session = getActiveSession();
              if (session) {
                api
                  .getGoal(session_id)
                  .then((g) => {
                    session.goal = g;
                  })
                  .catch(() => {});
              }
            }
            // rename_session will emit TitleUpdated event — alias is synced there
            try {
              await api.renameSession(session_id, description);
            } catch {
              // ignore rename failure
            }
            console.log("Goal mode activated — agent will work autonomously");
          }
          break;
        case "/history":
          openHistoryPicker();
          break;
        default:
          // Unknown command — treat as normal message
          await api.sendMessage(session_id, text);
      }
      return true;
    } catch (e: unknown) {
      const msg = api.errorMessage(e);
      console.error(`Failed to execute command ${cmd}:`, msg);
      showNotification(`Command failed: ${msg}`, "error");
      return false;
    }
  }

  async function handleSubmit() {
    if (isSending || !content.trim() || !sessionState.activeSessionId) return;
    isSending = true;
    try {
      const session_id = sessionState.activeSessionId;
      const baseText = content.trim();

      if (baseText.startsWith("/")) {
        const ok = await handleCommand(baseText);
        if (!ok) {
          content = baseText;
          autoResize();
          return;
        }
        content = "";
        autoResize();
        fileAttachments = [];
        clearInlineImages();
        return;
      }

      content = "";
      autoResize();

      // Append file attachments as suffix text
      const fileSuffix =
        fileAttachments.length > 0
          ? "\n" + fileAttachments.map((p) => `[File: ${p}]`).join("\n")
          : "";
      const text = baseText + fileSuffix;

      fileAttachments = [];

      if (inlineImages.length > 0) {
        // Message with inline images: build content blocks
        try {
          const blocks = buildContentBlocks(text, inlineImages);
          await api.sendMessageBlocks(session_id, blocks);
          clearInlineImages();
        } catch (e: unknown) {
          console.error(
            "Failed to send message with images:",
            e instanceof Error ? e.message : e,
          );
          showNotification("Failed to send message", "error");
        }
      } else {
        try {
          await api.sendMessage(session_id, text);
        } catch (e: unknown) {
          console.error(
            "Failed to send message:",
            e instanceof Error ? e.message : e,
          );
        }
      }
    } finally {
      isSending = false;
    }
  }

  async function handleCancel() {
    if (!sessionState.activeSessionId) return;
    try {
      await api.cancelSession(sessionState.activeSessionId);
    } catch (e: unknown) {
      console.error("Failed to cancel:", e instanceof Error ? e.message : e);
    }
  }

  async function handlePermissionSet(level: string) {
    const session_id = sessionState.activeSessionId;
    if (!session_id) return;
    const session = getSession(session_id);
    if (!session) return;
    try {
      await api.setPermissionLevel(session_id, level);
      session.permission_level = level;
      showNotification(`Permission level: ${level}`, "info");
    } catch (e: unknown) {
      console.error(
        "Failed to set permission level:",
        e instanceof Error ? e.message : e,
      );
      showNotification("Failed to set permission level", "error");
    }
  }

  // ── inline image helpers ──

  function addInlineImage(base64Url: string) {
    inlineImageCounter += 1;
    inlineImages = [
      ...inlineImages,
      { id: inlineImageCounter, url: base64Url },
    ];
  }

  function removeInlineImage(id: number) {
    inlineImages = inlineImages.filter((img) => img.id !== id);
  }

  function clearInlineImages() {
    inlineImages = [];
    inlineImageCounter = 0;
  }

  async function readFileAsBase64(file: File): Promise<string> {
    return new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => {
        const result = reader.result as string;
        resolve(result);
      };
      reader.onerror = reject;
      reader.readAsDataURL(file);
    });
  }

  async function handleClipboardImage(file: File) {
    if (!file.type.startsWith("image/")) {
      showNotification("Only image files are supported", "error");
      return;
    }
    try {
      const base64Url = await readFileAsBase64(file);
      addInlineImage(base64Url);
      textareaRef?.focus();
    } catch (e) {
      console.error("Failed to read image:", e);
      showNotification("Failed to read image", "error");
    }
  }

  // ── file attachments (any type, paths appended to prompt) ──
  let fileAttachments = $state<string[]>([]);
  let isSending = $state(false);

  async function attachFiles() {
    try {
      const selected = await open({ multiple: true });
      if (!selected) return;
      const paths = Array.isArray(selected) ? selected : [selected];
      const newPaths = paths.filter((p) => !fileAttachments.includes(p));
      if (newPaths.length === 0) return;
      fileAttachments = [...fileAttachments, ...newPaths];
      requestAnimationFrame(autoResize);
      textareaRef?.focus();
    } catch (e) {
      console.error("Failed to attach files:", e);
    }
  }

  function removeFileAttachment(path: string) {
    fileAttachments = fileAttachments.filter((p) => p !== path);
  }

  async function handlePaste(e: ClipboardEvent) {
    const items = e.clipboardData?.items;
    if (!items) return;

    for (const item of items) {
      if (item.type.startsWith("image/")) {
        e.preventDefault();
        const file = item.getAsFile();
        if (file) {
          await handleClipboardImage(file);
        }
      }
    }
  }

  function handleKeydown(e: KeyboardEvent) {
    // Ignore key events while IME is composing or right after composition ends
    if (e.isComposing || composing) {
      return;
    }

    // Skill picker navigation
    if (showSkills) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        if (filteredSkills.length === 0) return;
        selectedSkillIdx = (selectedSkillIdx + 1) % filteredSkills.length;
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (filteredSkills.length === 0) return;
        selectedSkillIdx =
          (selectedSkillIdx - 1 + filteredSkills.length) %
          filteredSkills.length;
        return;
      }
      if (e.key === "Tab" || e.key === "Enter") {
        e.preventDefault();
        const match = filteredSkills[selectedSkillIdx];
        if (match) acceptSkill(match.name);
        return;
      }
      if (e.key === "Escape") {
        showSkills = false;
        return;
      }
    }

    // Command picker navigation
    if (showCommands) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        if (filteredCommands.length === 0) return;
        selectedCommandIdx = (selectedCommandIdx + 1) % filteredCommands.length;
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (filteredCommands.length === 0) return;
        selectedCommandIdx =
          (selectedCommandIdx - 1 + filteredCommands.length) %
          filteredCommands.length;
        return;
      }
      if (e.key === "Tab" || e.key === "Enter") {
        e.preventDefault();
        const match = filteredCommands[selectedCommandIdx];
        if (match) acceptCommand(match[0]);
        return;
      }
      if (e.key === "Escape") {
        showCommands = false;
        return;
      }
    }

    // History picker navigation
    if (showHistory) {
      if (e.key === "ArrowDown") {
        e.preventDefault();
        if (historyEntries.length === 0) return;
        selectedHistoryIdx = (selectedHistoryIdx + 1) % historyEntries.length;
        return;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        if (historyEntries.length === 0) return;
        selectedHistoryIdx =
          (selectedHistoryIdx - 1 + historyEntries.length) %
          historyEntries.length;
        return;
      }
      if (e.key === "Tab" || e.key === "Enter") {
        e.preventDefault();
        const entry = historyEntries[selectedHistoryIdx];
        if (entry) {
          content = entry;
          showHistory = false;
          textareaRef?.focus();
          requestAnimationFrame(autoResize);
        } else {
          showHistory = false;
        }
        return;
      }
      if (e.key === "Escape") {
        showHistory = false;
        return;
      }
    }

    // Normal input
    if (e.key === "Enter" && !e.shiftKey) {
      // If this Enter is right after IME composition ends, ignore it
      if (ignoreNextEnter) {
        ignoreNextEnter = false;
        e.preventDefault();
        return;
      }
      e.preventDefault();
      const text = content.trim();
      if (text === "/history" || text.startsWith("/history ")) {
        openHistoryPicker();
        return;
      }
      if (isStreaming) {
        // "Enter again" gesture: empty input steers the queued message
        const session = activeSession;
        if (
          session &&
          !text &&
          inlineImages.length === 0 &&
          queueHead(session.id)
        ) {
          void steerQueued(session.id);
        } else {
          queueInput();
        }
      } else {
        handleSubmit();
      }
    }
  }

  function autoResize() {
    if (!textareaRef) return;
    textareaRef.style.height = "auto";
    if (!content.trim()) {
      return; // let min-height/rows decide initial height
    }
    textareaRef.style.height = Math.min(textareaRef.scrollHeight, 200) + "px";
  }

  $effect(() => {
    if (showCommands && commandListRef) {
      const buttons = commandListRef.querySelectorAll("button");
      const selected = buttons[selectedCommandIdx];
      if (selected) {
        selected.scrollIntoView({ block: "nearest", inline: "nearest" });
      }
    }
  });

  $effect(() => {
    if (showSkills && skillListRef) {
      const buttons = skillListRef.querySelectorAll("button");
      const selected = buttons[selectedSkillIdx];
      if (selected) {
        selected.scrollIntoView({ block: "nearest", inline: "nearest" });
      }
    }
  });

  $effect(() => {
    if (showHistory && historyListRef) {
      const buttons = historyListRef.querySelectorAll("button");
      const selected = buttons[selectedHistoryIdx];
      if (selected) {
        selected.scrollIntoView({ block: "nearest", inline: "nearest" });
      }
    }
  });

  $effect(() => {
    const currentId = activeSession?.id ?? null;
    if (prevSessionId !== currentId) {
      showHistory = false;
      showSkills = false;
      skillsLoadedForSessionId = null;
      availableSkills = [];
      prevSessionId = currentId;
      // Restore this session's draft; attachments are not persisted
      content = currentId ? (inputDrafts[currentId] ?? "") : "";
      clearInlineImages();
      fileAttachments = [];
      requestAnimationFrame(autoResize);
    }
  });

  // Persist the draft on every change (declared after the restore effect
  // above so a session switch never overwrites the new session's draft)
  $effect(() => {
    const currentId = activeSession?.id;
    if (currentId) inputDrafts[currentId] = content;
  });

  function getSession(session_id: string) {
    return sessionState.sessions.find((s) => s.id === session_id) ?? null;
  }

  function handleFocusOut(e: FocusEvent) {
    const container = e.currentTarget as HTMLElement;
    if (!container.contains(e.relatedTarget as Node)) {
      showCommands = false;
      showSkills = false;
      showHistory = false;
    }
  }
</script>

<div
  class="relative mb-2 rounded-md bg-card px-2 py-0 transition-shadow focus-within:ring-1 focus-within:ring-border"
  onfocusout={handleFocusOut}
>
  <!-- Command completion dropdown -->
  {#if showCommands && filteredCommands.length > 0}
    <div
      bind:this={commandListRef}
      class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-48 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50"
    >
      {#each filteredCommands as [cmd, desc], i (cmd)}
        <button
          class="flex items-center gap-2 w-full px-3 py-2 text-left text-sm transition-colors {i ===
          selectedCommandIdx
            ? 'bg-secondary'
            : 'hover:bg-secondary/50'}"
          onclick={() => acceptCommand(cmd)}
        >
          <Command size={14} class="text-muted-foreground shrink-0" />
          <span class="font-mono text-primary shrink-0">{cmd}</span>
          <span class="text-muted-foreground text-xs truncate">{desc}</span>
        </button>
      {/each}
    </div>
  {/if}

  <!-- Skill completion dropdown -->
  {#if showSkills && filteredSkills.length > 0}
    <div
      bind:this={skillListRef}
      class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-48 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50"
    >
      {#each filteredSkills as skill, i (skill.name)}
        <button
          class="flex items-center gap-2 w-full px-3 py-2 text-left text-sm transition-colors {i ===
          selectedSkillIdx
            ? 'bg-secondary'
            : 'hover:bg-secondary/50'}"
          onclick={() => acceptSkill(skill.name)}
        >
          <Wrench size={14} class="text-muted-foreground shrink-0" />
          <span class="font-mono text-primary shrink-0">{skill.name}</span>
          {#if skill.description}
            <span class="text-muted-foreground text-xs truncate"
              >{skill.description}</span
            >
          {/if}
        </button>
      {/each}
    </div>
  {/if}

  <!-- History picker -->
  {#if showHistory}
    <div
      bind:this={historyListRef}
      class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-48 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50"
    >
      {#if historyEntries.length === 0}
        <div class="px-3 py-4 text-sm text-muted-foreground text-center">
          No matching history
        </div>
      {:else}
        {#each historyEntries as entry, i (entry)}
          <button
            class="flex items-start gap-2 w-full px-3 py-2 text-left text-sm transition-colors {i ===
            selectedHistoryIdx
              ? 'bg-secondary'
              : 'hover:bg-secondary/50'}"
            onclick={() => {
              content = entry;
              showHistory = false;
              textareaRef?.focus();
              requestAnimationFrame(autoResize);
            }}
            title={entry}
          >
            <Clock size={14} class="text-muted-foreground shrink-0 mt-0.5" />
            <span class="truncate"
              >{entry.length > 80 ? entry.slice(0, 80) + "..." : entry}</span
            >
          </button>
        {/each}
      {/if}
    </div>
  {/if}

  <div>
    {#if inlineImages.length > 0}
      <div class="flex flex-wrap gap-2 px-2 pt-3 pb-1">
        {#each inlineImages as img (img.id)}
          <div class="relative group shrink-0">
            <img
              src={img.url}
              alt=""
              class="h-16 w-16 object-cover rounded-lg border border-border"
            />
            <button
              type="button"
              onclick={() => removeInlineImage(img.id)}
              class="absolute -top-1.5 -right-1.5 w-5 h-5 rounded-full bg-destructive text-destructive-foreground flex items-center justify-center opacity-0 group-hover:opacity-100 transition-opacity shadow-sm"
              title="Remove"
            >
              <X size={12} />
            </button>
          </div>
        {/each}
      </div>
    {/if}
    <div class="flex items-end gap-2 p-1 px-2">
      <textarea
        bind:this={textareaRef}
        bind:value={content}
        oninput={() => {
          detectCompletion();
          autoResize();
        }}
        onbeforeinput={blockPuaInput}
        onkeydown={handleKeydown}
        onfocus={detectCompletion}
        onblur={() => {
          /* dropdowns close via item clicks or Escape */
        }}
        oncompositionstart={() => (composing = true)}
        oncompositionend={() => {
          composing = false;
          ignoreNextEnter = true;
          setTimeout(() => (ignoreNextEnter = false), 100);
        }}
        onpaste={handlePaste}
        placeholder={isStreaming
          ? "Press Enter to queue next message..."
          : "Ask anything... (Shift+Enter newline, /command, paste image)"}
        rows={1}
        class="flex-1 resize-none bg-transparent text-sm placeholder:text-muted-foreground focus-visible:outline-none min-h-[40px] max-h-[200px] py-2.5"
      ></textarea>
      {#if isStreaming}
        <button
          type="button"
          onclick={handleCancel}
          class="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md bg-destructive text-destructive-foreground transition-all hover:bg-destructive/90 active:scale-95 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"
          aria-label="Stop generating"
          title="Stop generating"
        >
          <Square class="w-4 h-4 fill-current" />
        </button>
      {:else}
        <button
          type="button"
          onclick={handleSubmit}
          disabled={!content.trim() ||
            !sessionState.activeSessionId ||
            isSending}
          class="inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md transition-all focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background enabled:bg-primary enabled:text-primary-foreground enabled:hover:bg-primary/90 enabled:active:scale-95 disabled:cursor-not-allowed disabled:bg-secondary disabled:text-muted-foreground"
          aria-label="Send message"
          title="Send message"
        >
          <ArrowUp size={17} strokeWidth={2.25} />
        </button>
      {/if}
    </div>
  </div>
  {#if activeSession}
    <!-- File attachments -->
    {#if fileAttachments.length > 0}
      <div class="flex items-center gap-2 mt-1.5 px-2 flex-wrap">
        {#each fileAttachments as path (path)}
          <div
            class="flex items-center gap-1.5 rounded-md border border-border bg-secondary px-2 py-0.5"
          >
            <span class="text-xs text-muted-foreground truncate max-w-50"
              >{path.split("/").pop()}</span
            >
            <button
              type="button"
              onclick={() => removeFileAttachment(path)}
              class="text-muted-foreground hover:text-destructive transition-colors"
              title="Remove"
            >
              <X size={12} />
            </button>
          </div>
        {/each}
      </div>
    {/if}

    <div class="flex items-center justify-between gap-2 pb-1">
      <div class="flex min-w-0 items-center gap-1">
        <button
          type="button"
          onclick={attachFiles}
          class="inline-flex items-center gap-1 px-2 py-1 rounded-md text-xs text-muted-foreground hover:text-foreground hover:bg-secondary/50 transition-colors"
          title="Attach files"
        >
          <Paperclip size={14} />
        </button>
        <PermissionSelector
          value={(activeSession.permission_level as PermissionLevel) ??
            "caution"}
          onSelect={handlePermissionSet}
        />
        <ModelSelector
          session_id={activeSession.id}
          onContextWindowChange={(value) => (context_window = value)}
        />
      </div>
      {#if context_percent !== null}
        <div
          class="group/context flex shrink-0 items-center gap-1 rounded-md px-1.5 py-1.5 text-[10px] text-muted-foreground transition-colors hover:bg-secondary/50"
          title={context_title}
          aria-label={`Context usage ${context_percent.toFixed(1)} percent`}
        >
          <span
            class="flex items-center gap-0.5"
            role="progressbar"
            aria-valuemin="0"
            aria-valuemax="100"
            aria-valuenow={Math.round(context_progress_percent)}
          >
            {#each Array(5) as _, index (index)}
              <span
                class="h-2 w-1.5 rounded-[2px] transition-colors {index >=
                context_segments
                  ? 'bg-secondary'
                  : context_percent >= 90
                    ? 'bg-error'
                    : context_percent >= 70
                      ? 'bg-warning'
                      : 'bg-muted-foreground'}"
              ></span>
            {/each}
          </span>
          <span
            class="font-mono tabular-nums leading-none"
            class:text-warning={context_percent >= 70 && context_percent < 90}
            class:text-error={context_percent >= 90}
            >{Math.round(context_percent)}%</span
          >
        </div>
      {/if}
    </div>
  {/if}
</div>
