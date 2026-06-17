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

  const streamingCount = $derived(
    sessionState.sessions.filter(
      (s) => s.phase === "streaming" || s.phase === "executing_tool",
    ).length,
  );

  const anyStreaming = $derived(streamingCount > 0);
</script>

<div
  class="shrink-0 h-7 border-t border-border bg-background flex items-center px-3 text-xs select-none gap-3"
>
  <!-- Left: Streaming indicator -->
  <div class="flex items-center gap-1.5 min-w-0">
    {#if anyStreaming}
      <span class="flex items-center gap-1 text-primary">
        <Activity class="w-3 h-3 animate-pulse" />
        <span class="truncate">
          {streamingCount > 1 ? `${streamingCount} streaming` : "Streaming..."}
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
        <Wifi class="w-3 h-3 text-green-500" />
        <span class="text-green-500">Connected</span>
      {:else if appState.connectionStatus === "connecting"}
        <WifiOff class="w-3 h-3 text-amber-500" />
        <span class="text-amber-500">Connecting...</span>
      {/if}
    </div>
    {#if version}
      <span class="text-muted-foreground/70 text-[10px]">v{version}</span>
    {/if}
  </div>
</div>
