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
  import { findThinking, textFromBlocks } from "../../session";
  import { formatElapsed } from "../../utils";
  import { isAgentActivity } from "./activity-group";
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

  const SEARCH_READ_TOOLS = new Set([
    "read",
    "readfile",
    "grep",
    "grepsearch",
    "glob",
    "globsearch",
    "websearch",
    "webfetch",
  ]);

  interface Badge {
    icon: ComponentType;
    count: number;
    label: string;
  }

  const stats = $derived.by(() => {
    const materializedToolIds = new Set(
      messages
        .filter((message) => message.type === "tool")
        .map((message) => message.tool_call_id),
    );
    let subagentCount = 0;
    let editWriteCount = 0;
    let shellCount = 0;
    let searchReadCount = 0;
    let thinkingCount = 0;
    let otherToolCount = 0;
    let failedCount = 0;
    let elapsedMs = 0;

    for (const message of messages) {
      if (message.type === "assistant") {
        const thinking = findThinking(message.content);
        if (thinking) {
          thinkingCount += 1;
          elapsedMs += thinking.elapsed_ms ?? 0;
        }
        if (message.tool_calls?.length) {
          otherToolCount += message.tool_calls.filter(
            (call) => !materializedToolIds.has(call.id),
          ).length;
        }
        continue;
      }
      if (message.type !== "tool") continue;
      elapsedMs += message.elapsed_ms ?? 0;
      if (message.status === "failed") failedCount += 1;
      const name = message.tool_name.toLowerCase().replace(/[_-]/g, "");
      if (isAgentActivity(message)) subagentCount += 1;
      else if (["write", "writefile", "edit", "editfile"].includes(name))
        editWriteCount += 1;
      else if (["shell", "bash", "command"].includes(name)) shellCount += 1;
      else if (SEARCH_READ_TOOLS.has(name)) searchReadCount += 1;
      else otherToolCount += 1;
    }

    const badges: Badge[] = [
      { icon: Lightbulb, count: thinkingCount, label: "thoughts" },
      { icon: FileSearch, count: searchReadCount, label: "reads" },
      { icon: FileEdit, count: editWriteCount, label: "edits" },
      { icon: SquareTerminal, count: shellCount, label: "commands" },
      { icon: Bot, count: subagentCount, label: "agents" },
      { icon: Wrench, count: otherToolCount, label: "tools" },
    ].filter((badge) => badge.count > 0);

    return {
      badges,
      failedCount,
      elapsedMs,
      actionCount:
        thinkingCount +
        searchReadCount +
        editWriteCount +
        shellCount +
        subagentCount +
        otherToolCount,
    };
  });

  const trailItems = $derived.by(() => {
    const items: Array<
      | { type: "thought"; id: string; content: string; elapsed_ms: number }
      | {
          type: "tool";
          id: string;
          message: Extract<Message, { type: "tool" }>;
        }
    > = [];
    for (const message of messages) {
      if (message.type === "assistant") {
        const thinking = findThinking(message.content);
        if (thinking)
          items.push({
            type: "thought",
            id: message.id,
            content: thinking.content,
            elapsed_ms: thinking.elapsed_ms,
          });
      } else if (message.type === "tool") {
        items.push({ type: "tool", id: message.id, message });
      }
    }
    return items;
  });
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
