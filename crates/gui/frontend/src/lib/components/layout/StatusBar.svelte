<script lang="ts">
  import { Activity, Wifi, WifiOff } from "lucide-svelte";
  import { sessionState, appState } from "../../state.svelte";
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
      <span class="text-muted-foreground/70 text-[10px]">v{version}</span>
    {/if}
  </div>
</div>
