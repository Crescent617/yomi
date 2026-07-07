<script lang="ts">
  import { XCircle, Wrench, Lightbulb } from "lucide-svelte";
  import type { Message } from "../../state.svelte";
  import ThinkingBlock from "./ThinkingBlock.svelte";
  import ToolBlock from "../tool/ToolBlock.svelte";

  let {
    messages,
    isStreaming = false,
    isLatest = false,
  }: {
    messages: Message[];
    isStreaming?: boolean;
    isLatest?: boolean;
  } = $props();

  let expanded = $state(false);
  let userToggled = $state(false);

  // Auto-expand latest action group, auto-collapse when no longer latest
  $effect(() => {
    if (!userToggled) {
      expanded = isLatest;
    }
  });

  const stats = $derived.by(() => {
    let toolCount = 0;
    let thinkingCount = 0;
    let runningCount = 0;
    let failedCount = 0;
    let activeLabel = "";

    let latestRunningTool: { tool_name: string } | null = null;

    for (const m of messages) {
      if (m.type === "assistant" && m.thinking) {
        thinkingCount++;
      }
      if (m.type === "tool") {
        toolCount++;
        if (m.status === "running") {
          runningCount++;
          latestRunningTool = m;
        } else if (m.status === "failed") {
          failedCount++;
        }
      }
    }

    if (latestRunningTool) {
      activeLabel = `calling ${latestRunningTool.tool_name}`;
    } else if (isStreaming && thinkingCount > 0) {
      activeLabel = "thinking";
    }

    return { toolCount, thinkingCount, runningCount, failedCount, activeLabel };
  });
</script>

<div
  class="{expanded
    ? 'flex'
    : 'inline-flex'} flex-col rounded-lg border border-border/50 overflow-hidden"
>
  <button
    type="button"
    class="flex items-center gap-2 px-3 py-1.5 text-sm hover:bg-muted/40 transition-colors whitespace-nowrap"
    onclick={() => {
      userToggled = true;
      expanded = !expanded;
    }}
  >
    <!-- 状态图标 — 呼吸灯 or 失败 -->
    {#if stats.runningCount > 0 || (isStreaming && stats.thinkingCount > 0)}
      <span class="relative flex size-2 shrink-0">
        <span
          class="animate-ping absolute inline-flex h-full w-full rounded-full bg-amber-400 opacity-75"
        ></span>
        <span class="relative inline-flex rounded-full size-2 bg-amber-500"
        ></span>
      </span>
    {:else if stats.failedCount > 0}
      <XCircle class="size-4 text-red-500 shrink-0" />
    {/if}

    <!-- 标题 -->
    <span
      class="font-medium text-foreground shrink-0 inline-flex items-center gap-1.5"
    >
      {#if stats.toolCount > 0}
        <Wrench class="size-3.5 text-muted-foreground" />
        {#key stats.toolCount}
          <span class="roll-num inline-block">{stats.toolCount}</span>
        {/key}
      {/if}
      {#if stats.toolCount > 0 && stats.thinkingCount > 0}
        <span class="text-muted-foreground/40">·</span>
      {/if}
      {#if stats.thinkingCount > 0}
        <Lightbulb class="size-3.5 text-muted-foreground" />
        {#key stats.thinkingCount}
          <span class="roll-num inline-block">{stats.thinkingCount}</span>
        {/key}
      {/if}
    </span>

    {#if stats.activeLabel}
      <span class="text-muted-foreground/30">|</span>
      <span class="text-xs text-muted-foreground/70 truncate"
        >{stats.activeLabel}</span
      >
    {/if}
  </button>

  {#if expanded}
    <div class="p-2 space-y-2 border-t border-border/30 bg-muted/20 w-full">
      {#each messages as msg (msg.id)}
        {#if msg.type === "assistant" && msg.thinking}
          <ThinkingBlock
            content={msg.thinking.content}
            elapsed_ms={msg.thinking.elapsed_ms}
          />
        {/if}
        {#if msg.type === "tool"}
          <ToolBlock
            tool={{
              id: msg.tool_call_id,
              tool_name: msg.tool_name,
              status: msg.status,
              arguments: msg.arguments,
              output: msg.output,
              elapsed_ms: msg.elapsed_ms,
              subagent_session_id: msg.subagent_session_id,
            }}
          />
        {/if}
      {/each}
    </div>
  {/if}
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
