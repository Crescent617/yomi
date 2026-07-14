<script lang="ts">
  import { Activity, Github, Terminal, Wifi, WifiOff } from "lucide-svelte";
  import {
    appState,
    projectState,
    requestActivePanel,
    runningSessions,
    showNotification,
  } from "../../state.svelte";
  import { activateSession } from "../../session";
  import { errorMessage, openDefault } from "../../api";
  import { clock } from "../../clock.svelte";
  import { elapsedLabel, shellActivitySummary } from "./status-activity";
  import { getVersion } from "@tauri-apps/api/app";
  import { onMount } from "svelte";

  let version = $state("");
  let open = $state(false);
  let buttonRef = $state<HTMLButtonElement>();
  let cardRef = $state<HTMLDivElement>();

  onMount(() => {
    getVersion()
      .then((v) => (version = v))
      .catch(() => {});
  });

  const runningShells = $derived(
    runningSessions.flatMap((session) =>
      session.background_shells.map((shell) => ({ session, shell })),
    ),
  );
  const shellCount = $derived(runningShells.length);
  const summary = $derived(shellActivitySummary(shellCount));

  $effect(() => {
    if (shellCount === 0) open = false;
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
    if (open && !buttonRef?.contains(target) && !cardRef?.contains(target)) {
      open = false;
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
  class="shrink-0 h-7 border-t border-border bg-card flex items-center px-3 text-xs select-none gap-3"
>
  <div class="relative flex items-center gap-1.5 min-w-0">
    {#if shellCount > 0}
      <button
        bind:this={buttonRef}
        type="button"
        class="flex items-center gap-1 rounded px-1 py-0.5 text-primary transition-colors hover:bg-secondary/70"
        aria-expanded={open}
        title="Show running background shells"
        onclick={() => (open = !open)}
      >
        <Activity class="w-3 h-3 animate-pulse" />
        <span class="truncate">{summary}</span>
      </button>

      {#if open}
        <div
          bind:this={cardRef}
          class="absolute bottom-full left-0 z-30 mb-1 w-96 max-w-[calc(100vw-1.5rem)] overflow-hidden rounded-lg border border-border bg-popover text-popover-foreground shadow-xl"
        >
          <div class="border-b border-border px-3 py-2 font-medium">
            Running background shells
          </div>
          <div class="max-h-80 overflow-y-auto py-1">
            {#each runningShells as item (item.shell.task_id)}
              {@const project = projectName(item.session.project_id)}
              <button
                type="button"
                class="flex w-full items-start gap-2 px-3 py-2 text-left transition-colors hover:bg-secondary/50"
                onclick={() => openSession(item.session.id)}
              >
                <Terminal class="mt-0.5 h-3.5 w-3.5 shrink-0 text-info" />
                <span class="min-w-0 flex-1">
                  <span
                    class="block truncate font-mono text-xs font-medium"
                    title={item.shell.command}
                  >
                    {item.shell.command}
                  </span>
                  <span
                    class="block truncate text-[10px] text-muted-foreground"
                    title={item.shell.output_path}
                  >
                    PID {item.shell.pid} · {elapsedLabel(
                      item.shell.started_at,
                      clock.now,
                    )} · {project ?? sessionTitle(item.session)}
                  </span>
                </span>
                <span
                  class="mt-1.5 h-1.5 w-1.5 shrink-0 animate-pulse rounded-full bg-info"
                ></span>
              </button>
            {/each}
          </div>
        </div>
      {/if}
    {:else}
      <span class="text-muted-foreground">Ready</span>
    {/if}
  </div>

  <div class="flex-1"></div>

  <div class="flex items-center gap-3">
    <div class="flex items-center gap-1.5">
      {#if appState.connectionStatus === "connected"}
        <Wifi class="w-3 h-3 text-success" />
        <span class="text-success">Connected</span>
      {:else if appState.connectionStatus === "connecting"}
        <WifiOff class="w-3 h-3 text-warning" />
        <span class="text-warning">Connecting...</span>
      {/if}
    </div>
    {#if version}
      <a
        href="https://github.com/Crescent617/yomi"
        class="text-muted-foreground/70 hover:text-foreground flex items-center gap-1 text-[10px] transition-colors"
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
  </div>
</div>
