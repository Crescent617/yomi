<script lang="ts">
  import {
    Activity,
    Check,
    CircleArrowUp,
    Coffee,
    Copy,
    Github,
    Globe,
    House,
    LoaderCircle,
    Terminal,
  } from "lucide-svelte";
  import {
    projectState,
    requestActivePanel,
    runningSessions,
    showNotification,
    streamingSessions,
  } from "../../state.svelte";
  import { activateSession } from "../../session";
  import { errorMessage, openDefault } from "../../api";
  import * as api from "../../api";
  import { clock } from "../../clock.svelte";
  import { elapsedLabel } from "./status-activity";
  import AttentionBox from "./AttentionBox.svelte";
  import PopoverPanel from "../ui/PopoverPanel.svelte";
  import {
    guiPreferences,
    saveGuiPreferences,
    snapshotGuiPreferences,
  } from "../../settings.svelte";
  import { connectionState } from "../../connection.svelte";
  import {
    startUpdateChecker,
    stopUpdateChecker,
    updateCheckState,
  } from "../../update-check.svelte";
  import { getVersion } from "@tauri-apps/api/app";
  import { onMount } from "svelte";

  let version = $state("");
  let open = $state(false);
  let copiedOutputPath = $state<string | null>(null);
  let copyResetTimer: ReturnType<typeof setTimeout> | undefined;
  let activityRef = $state<HTMLDivElement>();
  let cardRef = $state<HTMLDivElement>();

  // ── Update check (GitHub Releases) ───────────────────────────────────
  let updateOpen = $state(false);
  let updateRef = $state<HTMLDivElement>();
  let updateCardRef = $state<HTMLDivElement>();
  const updateVersion = $derived(
    updateCheckState.status === "available"
      ? updateCheckState.latest_version
      : null,
  );
  const showUpdate = $derived(
    updateVersion !== null &&
      guiPreferences.updates.dismissed_version !== updateVersion,
  );

  async function openReleasePage() {
    const url = updateCheckState.release_url;
    if (!url) return;
    try {
      await openDefault(url);
    } catch (error) {
      showNotification(`Failed to open link: ${errorMessage(error)}`, "error");
    }
  }

  async function dismissUpdate() {
    if (!updateVersion) return;
    updateOpen = false;
    const prefs = snapshotGuiPreferences();
    prefs.updates.dismissed_version = updateVersion;
    try {
      await saveGuiPreferences(prefs);
    } catch (error) {
      showNotification(
        `Failed to save preferences: ${errorMessage(error)}`,
        "error",
      );
    }
  }

  // ── Connection (local / remote daemon) ───────────────────────────────
  let connInfo = $derived(connectionState.info);
  let connOpen = $state(false);
  let connInput = $state("");
  let connBusy = $state(false);
  let connError = $state<string | null>(null);
  let connRef = $state<HTMLDivElement>();
  let connCardRef = $state<HTMLDivElement>();

  function remoteHostLabel(addr: string): string {
    return addr.replace(/^[a-z]+:\/\//i, "");
  }

  function isWsAddr(addr: string): boolean {
    return /^wss?:\/\//i.test(addr);
  }

  function openConnPopover() {
    connError = null;
    connInput =
      connInfo?.mode === "remote"
        ? connInfo.addr
        : (guiPreferences.connection.remote_addr ?? "");
    connOpen = true;
  }

  async function submitConnect() {
    const addr = connInput.trim();
    if (!/^(wss?|tcp|unix):\/\//.test(addr)) {
      connError = "Address must start with ws://, wss://, tcp:// or unix://";
      return;
    }
    connBusy = true;
    connError = null;
    try {
      await api.connectRemote(addr);
      guiPreferences.connection.remote_addr = addr;
      try {
        await saveGuiPreferences(snapshotGuiPreferences());
      } catch (error) {
        console.warn("Failed to save remote daemon address:", error);
      }
      // Kernel swapped — always reload, even if persisting the address failed.
      window.location.reload();
    } catch (error) {
      connError = errorMessage(error);
      connBusy = false;
    }
  }

  async function backToLocal() {
    connBusy = true;
    connError = null;
    try {
      await api.disconnectRemote();
      window.location.reload();
    } catch (error) {
      connError = errorMessage(error);
      connBusy = false;
    }
  }

  onMount(() => {
    getVersion()
      .then((v) => (version = v))
      .catch(() => {});
    startUpdateChecker();
    return () => {
      stopUpdateChecker();
      if (copyResetTimer) clearTimeout(copyResetTimer);
    };
  });

  const runningShells = $derived(
    runningSessions.flatMap((session) =>
      session.background_shells.map((shell) => ({ session, shell })),
    ),
  );
  const shellCount = $derived(runningShells.length);
  const streamingCount = $derived(streamingSessions().length);

  $effect(() => {
    if (shellCount === 0 && streamingCount === 0) open = false;
  });

  function sessionTitle(session: (typeof runningSessions)[number]): string {
    return session.title ?? (session.parent_id ? "Subagent" : "Untitled");
  }

  function projectName(projectId: string | null): string | null {
    if (!projectId) return null;
    return (
      projectState.projects.find((project) => project.id === projectId)?.name ??
      null
    );
  }

  function handleClickOutside(event: MouseEvent) {
    const target = event.target as Node;
    if (open && !activityRef?.contains(target) && !cardRef?.contains(target)) {
      open = false;
    }
    if (
      connOpen &&
      !connRef?.contains(target) &&
      !connCardRef?.contains(target)
    ) {
      connOpen = false;
    }
    if (
      updateOpen &&
      !updateRef?.contains(target) &&
      !updateCardRef?.contains(target)
    ) {
      updateOpen = false;
    }
  }

  async function copyOutputPath(outputPath: string) {
    try {
      await navigator.clipboard.writeText(outputPath);
      copiedOutputPath = outputPath;
      if (copyResetTimer) clearTimeout(copyResetTimer);
      copyResetTimer = setTimeout(() => (copiedOutputPath = null), 1500);
    } catch (error) {
      showNotification(
        `Failed to copy shell log path: ${errorMessage(error)}`,
        "error",
      );
    }
  }

  // ── Keep-awake power assertion ───────────────────────────────────────
  let keepAwake = $derived(guiPreferences.power.keep_awake);
  let keepAwakeBusy = $state(false);

  async function toggleKeepAwake() {
    if (keepAwakeBusy) return;
    keepAwakeBusy = true;
    const next = !keepAwake;
    try {
      // Apply to the OS first; the preference only follows on success.
      await api.setKeepAwake(next);
      const prefs = snapshotGuiPreferences();
      prefs.power.keep_awake = next;
      await saveGuiPreferences(prefs);
    } catch (error) {
      showNotification(
        `Failed to toggle keep-awake: ${errorMessage(error)}`,
        "error",
      );
    } finally {
      keepAwakeBusy = false;
    }
  }

  async function openSession(sessionId: string) {
    open = false;
    if (!requestActivePanel("chat")) return;
    try {
      await activateSession(sessionId);
    } catch {
      // activateSession reports the error and restores the previous session.
    }
  }
</script>

<svelte:window onclick={handleClickOutside} />

<div
  class="shrink-0 h-7 border-t border-border bg-card flex items-center px-3 text-xs select-none gap-2"
>
  <div bind:this={connRef} class="relative flex items-center">
    <button
      type="button"
      class="flex items-center gap-1.5 rounded px-1 py-0.5 transition-colors hover:bg-secondary/70 {connInfo?.mode ===
      'remote'
        ? 'text-info'
        : 'text-foreground'}"
      aria-expanded={connOpen}
      title={connInfo
        ? connInfo.mode === "remote"
          ? `Remote daemon: ${connInfo.addr}`
          : isWsAddr(connInfo.addr)
            ? `Default daemon (via YOMI_SOCKET): ${connInfo.addr}`
            : `Local daemon: ${connInfo.addr}`
        : "Connection"}
      onclick={() => (connOpen ? (connOpen = false) : openConnPopover())}
    >
      {#if connInfo?.mode === "remote"}
        <Globe class="w-3 h-3" />
        <span class="micro-label text-info">REMOTE</span>
        <span class="max-w-32 truncate font-mono text-[10px] text-foreground"
          >{remoteHostLabel(connInfo.addr)}</span
        >
      {:else if connInfo && isWsAddr(connInfo.addr)}
        <Globe class="w-3 h-3" />
        <span class="micro-label">REMOTE</span>
        <span class="max-w-32 truncate font-mono text-[10px] text-foreground"
          >{remoteHostLabel(connInfo.addr)}</span
        >
      {:else}
        <span class="micro-label">LOCAL</span>
      {/if}
    </button>

    {#if connOpen}
      <PopoverPanel
        bind:ref={connCardRef}
        title="Connection"
        padded
        class="absolute bottom-full left-0 z-30 mb-1 w-80"
      >
        <div class="space-y-0.5">
          <div class="micro-label text-foreground">Current</div>
          <div class="flex items-center gap-1.5 text-xs">
            {#if connInfo?.mode === "remote"}
              <Globe class="h-3 w-3 shrink-0 text-info" />
              <span>Remote daemon</span>
            {:else if connInfo && isWsAddr(connInfo.addr)}
              <Globe class="h-3 w-3 shrink-0" />
              <span>Default daemon</span>
            {:else}
              <House class="h-3 w-3 shrink-0" />
              <span>Local daemon</span>
            {/if}
          </div>
          {#if connInfo}
            <div
              class="truncate font-mono text-[10px] text-foreground"
              title={connInfo.addr}
            >
              {connInfo.addr}{#if connInfo.mode === "local" && isWsAddr(connInfo.addr)}
                &nbsp;· via YOMI_SOCKET{/if}
            </div>
          {/if}
        </div>

        <div class="h-px bg-border"></div>

        <div class="space-y-1.5">
          <label
            for="remote-addr-input"
            class="micro-label block text-foreground"
          >
            Remote daemon URL
          </label>
          <input
            id="remote-addr-input"
            type="text"
            bind:value={connInput}
            placeholder="wss://host:port"
            disabled={connBusy}
            autocapitalize="off"
            autocorrect="off"
            spellcheck="false"
            class="w-full rounded-md border border-border bg-background px-2 py-1.5 font-mono text-xs placeholder:text-foreground focus:outline-none focus:ring-1 focus:ring-ring disabled:opacity-60"
            onkeydown={(e: KeyboardEvent) => {
              if (e.key === "Enter" && !connBusy) void submitConnect();
            }}
          />
          {#if connError}
            <p class="text-[11px] leading-snug text-error">{connError}</p>
          {/if}
          <div class="flex items-center gap-2 pt-0.5">
            <button
              type="button"
              disabled={connBusy || !connInput.trim()}
              class="inline-flex h-7 items-center gap-1.5 rounded-md bg-primary px-2.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90 disabled:opacity-50"
              onclick={() => void submitConnect()}
            >
              {#if connBusy}
                <LoaderCircle class="h-3 w-3 animate-spin" />
              {/if}
              Connect
            </button>
            {#if connInfo?.mode === "remote"}
              <button
                type="button"
                disabled={connBusy}
                class="inline-flex h-7 items-center rounded-md border border-border bg-secondary px-2.5 text-xs font-medium text-foreground transition-colors hover:bg-secondary/80 disabled:opacity-50"
                onclick={() => void backToLocal()}
              >
                Back to local
              </button>
            {/if}
          </div>
        </div>
      </PopoverPanel>
    {/if}
  </div>

  <div class="h-3 w-px shrink-0 bg-border" aria-hidden="true"></div>

  <div
    bind:this={activityRef}
    class="relative flex items-center gap-1.5 min-w-0"
  >
    {#if streamingCount > 0 || shellCount > 0}
      <button
        type="button"
        class="flex items-center gap-1.5 rounded px-1 py-0.5 transition-colors hover:bg-secondary/70"
        aria-expanded={open}
        title="Show running activity"
        onclick={() => (open = !open)}
      >
        {#if streamingCount > 0}
          <span class="micro-label truncate">
            <span class="text-primary tabular-nums">{streamingCount}</span>
            <span class="text-foreground">
              {streamingCount > 1 ? "agents" : "agent"}</span
            >
          </span>
        {/if}
        {#if streamingCount > 0 && shellCount > 0}
          <span class="text-border" aria-hidden="true">·</span>
        {/if}
        {#if shellCount > 0}
          <span class="micro-label truncate">
            <span class="text-primary tabular-nums">{shellCount}</span>
            <span class="text-foreground">
              {shellCount > 1 ? "shells" : "shell"}</span
            >
          </span>
        {/if}
      </button>
    {:else}
      <span class="micro-label text-foreground">Ready</span>
    {/if}

    {#if open}
      <PopoverPanel
        bind:ref={cardRef}
        title="Running activity"
        class="absolute bottom-full left-0 z-30 mb-1 w-96 max-w-[calc(100vw-1.5rem)]"
      >
        {#each streamingSessions() as session (session.id)}
          {@const project = projectName(session.project_id)}
          <button
            type="button"
            class="popover-list-item flex w-full items-start gap-2 px-3 py-2 text-left"
            onclick={() => openSession(session.id)}
            title="Open session"
          >
            <Activity
              class="mt-0.5 h-3.5 w-3.5 shrink-0 animate-pulse text-primary"
            />
            <span class="min-w-0 flex-1">
              <span class="block truncate text-xs font-medium">
                {sessionTitle(session)}
              </span>
              <span class="block truncate text-[10px] text-foreground">
                {session.phase.replaceAll("_", " ")}{#if project}
                  · {project}{/if}
              </span>
            </span>
          </button>
        {/each}
        {#if streamingCount > 0 && shellCount > 0}
          <div class="mx-3 my-1 h-px bg-border"></div>
        {/if}
        {#each runningShells as item (item.shell.task_id)}
          {@const project = projectName(item.session.project_id)}
          <div
            class="popover-list-item flex w-full items-start gap-2 px-3 py-2"
          >
            <button
              type="button"
              class="flex min-w-0 flex-1 items-start gap-2 text-left"
              onclick={() => openSession(item.session.id)}
              title="Open session"
            >
              <Terminal class="mt-0.5 h-3.5 w-3.5 shrink-0 text-info" />
              <span class="min-w-0 flex-1">
                <span
                  class="block truncate font-mono text-xs font-medium"
                  title={item.shell.command}
                >
                  {item.shell.command}
                </span>
                <span class="block truncate text-[10px] text-foreground">
                  PID {item.shell.pid} · {elapsedLabel(
                    item.shell.started_at,
                    clock.now,
                  )} · {project ?? sessionTitle(item.session)}
                </span>
              </span>
            </button>
            <button
              type="button"
              class="inline-flex h-6 shrink-0 items-center gap-1 rounded px-1.5 text-[10px] text-foreground transition-colors hover:bg-secondary hover:text-foreground"
              onclick={() => void copyOutputPath(item.shell.output_path)}
              title={`Copy log path: ${item.shell.output_path}`}
              aria-label={`Copy log path for ${item.shell.task_id}`}
            >
              {#if copiedOutputPath === item.shell.output_path}
                <Check class="h-3 w-3 text-success" />
                <span>Copied</span>
              {:else}
                <Copy class="h-3 w-3" />
                <span>Log</span>
              {/if}
            </button>
            <span
              class="mt-2 h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-info"
            ></span>
          </div>
        {/each}
      </PopoverPanel>
    {/if}
  </div>

  <div class="flex-1"></div>

  <div class="flex items-center gap-2">
    <!-- Keep-awake: hold an OS power assertion so the device won't sleep -->
    <button
      type="button"
      class="grid size-5 place-items-center rounded transition-colors hover:bg-secondary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50 {keepAwake
        ? 'text-primary'
        : 'text-foreground hover:text-foreground'}"
      title={keepAwake
        ? "Keep awake on — this device won't sleep while Yomi runs (the display may still turn off)"
        : "Keep awake off — click to prevent this device from sleeping"}
      aria-pressed={keepAwake}
      aria-label="Toggle keep awake"
      disabled={keepAwakeBusy}
      onclick={() => void toggleKeepAwake()}
    >
      <Coffee class="size-3.5" aria-hidden="true" />
    </button>

    {#if showUpdate && updateVersion}
      <div bind:this={updateRef} class="relative flex items-center">
        <button
          type="button"
          class="flex items-center gap-1 rounded px-1 py-0.5 text-primary transition-colors hover:bg-secondary/70"
          aria-expanded={updateOpen}
          title="Update available: v{updateVersion} — click for details"
          onclick={() => (updateOpen = !updateOpen)}
        >
          <CircleArrowUp class="w-3 h-3" aria-hidden="true" />
          <span class="micro-label">v{updateVersion}</span>
        </button>

        {#if updateOpen}
          <PopoverPanel
            bind:ref={updateCardRef}
            title="Update available"
            padded
            class="absolute bottom-full right-0 z-30 mb-1 w-64"
          >
            <div class="space-y-0.5">
              <div class="flex items-center justify-between gap-2 text-xs">
                <span class="micro-label text-foreground">Current</span>
                <span class="font-mono text-[10px]">v{version}</span>
              </div>
              <div class="flex items-center justify-between gap-2 text-xs">
                <span class="micro-label text-foreground">Latest</span>
                <span class="font-mono text-[10px] text-primary">
                  v{updateVersion}
                </span>
              </div>
              {#if updateCheckState.published_at}
                <div class="text-[10px] text-foreground">
                  Published {new Date(
                    updateCheckState.published_at,
                  ).toLocaleDateString()}
                </div>
              {/if}
            </div>

            <div class="h-px bg-border"></div>

            <div class="flex items-center gap-2">
              <button
                type="button"
                class="inline-flex h-7 items-center gap-1.5 rounded-md bg-primary px-2.5 text-xs font-medium text-primary-foreground transition-colors hover:bg-primary/90"
                onclick={() => void openReleasePage()}
              >
                View release
              </button>
              <button
                type="button"
                class="inline-flex h-7 items-center rounded-md border border-border bg-secondary px-2.5 text-xs font-medium text-foreground transition-colors hover:bg-secondary/80"
                onclick={() => void dismissUpdate()}
              >
                Dismiss
              </button>
            </div>
          </PopoverPanel>
        {/if}
      </div>
    {/if}

    {#if version}
      <a
        href="https://github.com/Crescent617/yomi"
        class="micro-label [font-family:var(--font-sans)] text-foreground hover:text-foreground flex items-center gap-0.5 transition-colors"
        title="Open Yomi on GitHub"
        onclick={(event) => {
          event.preventDefault();
          void openDefault(event.currentTarget.href).catch((error) => {
            showNotification(
              `Failed to open link: ${errorMessage(error)}`,
              "error",
            );
          });
        }}
      >
        <Github class="w-3 h-3" aria-hidden="true" />
        <span>v{version}</span>
      </a>
    {/if}
    <AttentionBox />
  </div>
</div>
