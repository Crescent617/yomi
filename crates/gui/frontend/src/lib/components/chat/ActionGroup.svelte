<script lang="ts">
  import { XCircle, Wrench, Lightbulb } from "lucide-svelte";
  import type { ChatMessage } from "../../state.svelte";
  import ThinkingBlock from "./ThinkingBlock.svelte";
  import ToolBlock from "./ToolBlock.svelte";

  let {
    messages,
    isStreaming = false,
  }: {
    messages: ChatMessage[];
    isStreaming?: boolean;
  } = $props();

  let expanded = $state(false);

  const stats = $derived.by(() => {
    let toolCount = 0;
    let thinkingCount = 0;
    let runningCount = 0;
    let failedCount = 0;
    const seenNames = new Set<string>();
    const runningNames: string[] = [];

    for (const m of messages) {
      if (m.thinking) thinkingCount++;
      if (m.tools) {
        for (const t of m.tools) {
          toolCount++;
          if (t.status === "running") {
            runningCount++;
            if (!seenNames.has(t.toolName)) {
              seenNames.add(t.toolName);
              runningNames.push(t.toolName);
            }
          } else if (t.status === "failed") {
            failedCount++;
          }
        }
      }
    }

    const activeLabel = runningNames.length > 0
      ? `calling ${runningNames.join(", ")}`
      : isStreaming && thinkingCount > 0
        ? "thinking"
        : "";

    return { toolCount, thinkingCount, runningCount, failedCount, activeLabel };
  });
</script>

<div class="{expanded ? 'flex' : 'inline-flex'} flex-col rounded-lg border border-border/50 overflow-hidden">
  <button
    type="button"
    class="flex items-center gap-2 px-3 py-1.5 text-sm hover:bg-muted/40 transition-colors whitespace-nowrap"
    onclick={() => expanded = !expanded}
  >
    <!-- 状态图标 — 呼吸灯 or 失败 -->
    {#if isStreaming || stats.runningCount > 0}
      <span class="relative flex size-2 shrink-0">
        <span class="animate-ping absolute inline-flex h-full w-full rounded-full bg-amber-400 opacity-75"></span>
        <span class="relative inline-flex rounded-full size-2 bg-amber-500"></span>
      </span>
    {:else if stats.failedCount > 0}
      <XCircle class="size-4 text-red-500 shrink-0" />
    {/if}

    <!-- 标题 -->
    <span class="font-medium text-foreground shrink-0 inline-flex items-center gap-1.5">
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
      <span class="text-xs text-muted-foreground/70 truncate">{stats.activeLabel}</span>
    {/if}
  </button>

  {#if expanded}
    <div class="p-2 space-y-2 border-t border-border/30 bg-muted/20 w-full">
      {#each messages as msg (`msg-${msg.id}`)}
        {#if msg.thinking}
          <ThinkingBlock content={msg.thinking.content} elapsedMs={msg.thinking.elapsedMs} />
        {/if}
        {#if msg.tools && msg.tools.length > 0}
          {#each msg.tools as tool (`${msg.id}-${tool.id}`)}
            <ToolBlock {tool} />
          {/each}
        {/if}
      {/each}
    </div>
  {/if}
</div>

<style>
  @keyframes roll-in {
    0% { transform: translateY(60%); opacity: 0; }
    100% { transform: translateY(0); opacity: 1; }
  }
  .roll-num {
    animation: roll-in 0.25s ease-out;
  }
</style>
