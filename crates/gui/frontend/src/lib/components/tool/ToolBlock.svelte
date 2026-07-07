<script lang="ts">
  import {
    ChevronDown,
    ChevronUp,
    ArrowUpRight,
    Bot,
  } from "lucide-svelte";
  import type { ToolCall } from "../../state.svelte";
  import {
    statusColor,
    compactArgs,
    extractTarget,
    extraMeta,
    formatElapsed,
    handleJumpToSubagent,
  } from "./tool-utils";
  import ToolIcon from "./ToolIcon.svelte";
  import ToolBody from "./ToolBody.svelte";

  let {
    tool,
    expanded: initialExpanded = false,
  }: { tool: ToolCall; expanded?: boolean } = $props();

  let expanded = $state(initialExpanded);

  const target = $derived(extractTarget(tool.tool_name, tool.arguments ?? ""));
  const meta = $derived(extraMeta(tool.tool_name, tool.arguments ?? ""));

  let showSessionId = $state(false);
</script>

<div
  class="rounded-md border text-sm overflow-hidden {statusColor(tool.status)}"
>
  <!-- Header — always visible, clickable -->
  <button
    type="button"
    class="w-full flex items-center gap-2 px-3 py-1.5 text-left transition-colors"
    onclick={() => (expanded = !expanded)}
  >
    <ToolIcon
      toolName={tool.tool_name}
      isRunning={tool.status === "running"}
      className="w-4 h-4 shrink-0"
    />

    <span class="font-semibold capitalize shrink-0">{tool.tool_name}</span>

    {#if target}
      <span class="text-xs opacity-70 truncate">{target}</span>
    {:else if tool.arguments}
      <span class="text-xs opacity-60 truncate"
        >{compactArgs(tool.arguments, 80)}</span
      >
    {/if}
    {#if meta}
      <span class="text-xs opacity-50 shrink-0">· {meta}</span>
    {/if}

    {#if tool.elapsed_ms && tool.elapsed_ms > 1000}
      <span class="text-xs opacity-60 shrink-0"
        >{formatElapsed(tool.elapsed_ms)}</span
      >
    {/if}
    {#if tool.progress && tool.status === "running"}
      <span class="text-xs opacity-60 truncate">· {tool.progress}</span>
    {/if}
    {#if tool.tokens}
      <span class="text-xs opacity-60 shrink-0">· {tool.tokens} tokens</span>
    {/if}
    {#if tool.subagent_session_id}
      <div class="ml-auto flex items-center gap-1">
        <button
          type="button"
          class="relative inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[10px] text-muted-foreground hover:text-foreground bg-transparent hover:bg-background dark:hover:bg-muted transition-colors"
          onmouseenter={() => (showSessionId = true)}
          onmouseleave={() => (showSessionId = false)}
          onclick={(e) => {
            e.stopPropagation();
            handleJumpToSubagent(tool.subagent_session_id!);
          }}
        >
          <Bot class="w-3 h-3 opacity-60" />
          <ArrowUpRight class="w-3 h-3 opacity-50" />
          {#if showSessionId}
            <div
              class="absolute left-1/2 -translate-x-1/2 bottom-full mb-2 z-50 pointer-events-none"
            >
              <div
                class="absolute left-1/2 -translate-x-1/2 -bottom-[3px] w-2 h-2 bg-card rotate-45 border-r border-b border-border/20"
              ></div>
              <div
                class="relative px-3 py-2 bg-card rounded-lg border border-border/20 text-[11px] text-foreground whitespace-nowrap font-mono"
              >
                {tool.subagent_session_id}
              </div>
            </div>
          {/if}
        </button>
        <span>
          {#if expanded}
            <ChevronUp class="w-3.5 h-3.5 opacity-50" />
          {:else}
            <ChevronDown class="w-3.5 h-3.5 opacity-50" />
          {/if}
        </span>
      </div>
    {:else}
      <span class="ml-auto">
        {#if expanded}
          <ChevronUp class="w-3.5 h-3.5 opacity-50" />
        {:else}
          <ChevronDown class="w-3.5 h-3.5 opacity-50" />
        {/if}
      </span>
    {/if}
  </button>

  {#if expanded}
    <ToolBody {tool} />
  {/if}
</div>
