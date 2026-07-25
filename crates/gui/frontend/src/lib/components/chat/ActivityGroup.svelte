<script lang="ts">
  import {
    Bot,
    ChevronDown,
    FileEdit,
    FileSearch,
    Lightbulb,
    SquareTerminal,
    Wrench,
    XCircle,
  } from "lucide-svelte";
  import type { ComponentType } from "svelte";
  import type { Message } from "../../state.svelte";
  import { imageUrlsFromBlocks, textFromBlocks } from "../../session";
  import { formatElapsed } from "../../utils";
  import { buildActivityTrail, computeActivityStats } from "./activity-group";
  import { guiPreferences } from "../../settings.svelte";
  import {
    activityGroupExpanded,
    type ActivityGroupOverride,
  } from "./activity-expansion";
  import ThinkingBlock from "./ThinkingBlock.svelte";
  import ToolBlock from "../tool/ToolBlock.svelte";

  let {
    messages,
    isActiveActivity = false,
    isLatestActivity = false,
    expansionOverride = null,
    onExpansionOverride,
  }: {
    messages: Message[];
    isActiveActivity?: boolean;
    isLatestActivity?: boolean;
    expansionOverride?: ActivityGroupOverride;
    onExpansionOverride: (override: ActivityGroupOverride) => void;
  } = $props();

  let lastPreference = $state(guiPreferences.chat.activityGroupExpansion);
  const expanded = $derived(
    activityGroupExpanded(
      guiPreferences.chat.activityGroupExpansion,
      isLatestActivity,
      isActiveActivity,
      expansionOverride,
    ),
  );

  $effect(() => {
    const preference = guiPreferences.chat.activityGroupExpansion;
    if (preference !== lastPreference) {
      lastPreference = preference;
      onExpansionOverride(null);
    }
  });

  function toggleExpanded() {
    onExpansionOverride(expanded ? "closed" : "open");
  }

  interface Badge {
    icon: ComponentType;
    count: number;
    label: string;
  }

  const stats = $derived.by(() => {
    const counts = computeActivityStats(messages);
    const badges: Badge[] = [
      { icon: Lightbulb, count: counts.thinkingCount, label: "thoughts" },
      { icon: FileSearch, count: counts.searchReadCount, label: "reads" },
      { icon: FileEdit, count: counts.editWriteCount, label: "edits" },
      { icon: SquareTerminal, count: counts.shellCount, label: "commands" },
      { icon: Bot, count: counts.subagentCount, label: "agents" },
      { icon: Wrench, count: counts.otherToolCount, label: "tools" },
    ].filter((badge) => badge.count > 0);

    return {
      badges,
      failedCount: counts.failedCount,
      elapsedMs: counts.elapsedMs,
      actionCount: counts.actionCount,
    };
  });

  const trailItems = $derived.by(() => buildActivityTrail(messages));
</script>

{#if stats.actionCount > 0}
  <div class="overflow-hidden rounded-md">
    <button
      type="button"
      class="flex min-h-8 w-full items-center gap-1 text-left transition-colors hover:bg-secondary/30 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
      onclick={toggleExpanded}
      aria-expanded={expanded}
      aria-label={`${expanded ? "Collapse" : "Expand"} activity details, ${stats.actionCount} actions`}
    >
      <span class="flex w-3 shrink-0 items-center justify-center">
        <ChevronDown
          class="size-3.5 text-muted-foreground transition-transform duration-150 {expanded
            ? ''
            : '-rotate-90'}"
        />
      </span>
      <span class="flex min-w-0 flex-1 items-center gap-2">
        <span class="min-w-0 items-center gap-2 text-muted-foreground sm:flex">
          {#each stats.badges as badge (badge.label)}
            <span
              class="inline-flex shrink-0 items-center gap-1 text-[11px]"
              title={`${badge.count} ${badge.label}`}
            >
              <badge.icon class="size-3.5" /><span class="tabular-nums"
                >{badge.count}</span
              >
            </span>
          {/each}
        </span>
        {#if stats.failedCount > 0}
          <span
            class="inline-flex shrink-0 items-center gap-1 text-[11px] text-error"
            ><XCircle class="size-3.5" />{stats.failedCount} failed</span
          >
        {/if}
        {#if stats.elapsedMs > 0}
          <span
            class="ml-auto shrink-0 text-[11px] tabular-nums text-muted-foreground/70"
            >{formatElapsed(stats.elapsedMs)}</span
          >
        {/if}
      </span>
    </button>

    {#if expanded}
      <div class="bg-secondary/10 pl-0.5">
        {#each trailItems as item, index (`${item.type}-${item.id}-${index}`)}
          {#if item.type === "thought"}
            <ThinkingBlock
              content={item.content}
              elapsed_ms={item.elapsed_ms}
              isRunning={isActiveActivity && index === trailItems.length - 1}
              isFirst={index === 0}
              isLast={index === trailItems.length - 1}
            />
          {:else}
            <ToolBlock
              tool={{
                id: item.message.tool_call_id,
                tool_name: item.message.tool_name,
                status: item.message.status,
                arguments: item.message.arguments,
                output: textFromBlocks(item.message.result),
                images: imageUrlsFromBlocks(item.message.result),
                elapsed_ms: item.message.elapsed_ms,
                subagent_session_id: item.message.subagent_session_id,
              }}
              isFirst={index === 0}
              isLast={index === trailItems.length - 1}
            />
          {/if}
        {/each}
      </div>
    {/if}
  </div>
{/if}
