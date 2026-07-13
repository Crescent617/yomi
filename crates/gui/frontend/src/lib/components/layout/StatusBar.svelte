<script lang="ts">
  import { Activity, Github, Wifi, WifiOff } from "lucide-svelte";
  import { sessionState, appState, showNotification } from "../../state.svelte";
  import { errorMessage, openDefault } from "../../api";
  import { getVersion } from "@tauri-apps/api/app";
  import { onMount } from "svelte";

  let version = $state("");

  onMount(() => {
    getVersion()
      .then((v) => (version = v))
      .catch(() => {});
  });

  const runningCount = $derived(
    sessionState.sessions.filter(
      (session) =>
        session.phase === "streaming" ||
        session.phase === "executing_tool" ||
        session.phase === "compacting",
    ).length,
  );

  const anyRunning = $derived(runningCount > 0);
</script>

<div
  class="shrink-0 h-7 border-t border-border bg-card flex items-center px-3 text-xs select-none gap-3"
>
  <!-- Left: background activity -->
  <div class="flex items-center gap-1.5 min-w-0">
    {#if anyRunning}
      <span class="flex items-center gap-1 text-primary">
        <Activity class="w-3 h-3 animate-pulse" />
        <span class="truncate">
          {runningCount === 1
            ? "1 session running"
            : `${runningCount} sessions running`}
        </span>
      </span>
    {:else}
      <span class="text-muted-foreground">Ready</span>
    {/if}
  </div>

  <div class="flex-1"></div>

  <!-- Right: Connection + Version -->
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
