<script lang="ts">
  import { ArrowRight, ChevronDown, ChevronRight } from "lucide-svelte";
  import type { ToolCall } from "../../state.svelte";
  import {
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
    isFirst = false,
    isLast = false,
  }: {
    tool: ToolCall;
    isFirst?: boolean;
    isLast?: boolean;
  } = $props();

  let expanded = $state(false);
  const target = $derived(extractTarget(tool.tool_name, tool.arguments ?? ""));
  const meta = $derived(extraMeta(tool.tool_name, tool.arguments ?? ""));
  const isSubagent = $derived(Boolean(tool.subagent_session_id));
  const label = $derived(
    isSubagent
      ? "Agent"
      : tool.tool_name
        ? tool.tool_name.charAt(0).toUpperCase() + tool.tool_name.slice(1)
        : "Tool",
  );

  function statusDotClass(status: string): string {
    switch (status) {
      case "running":
        return "bg-primary";
      case "completed":
        return "bg-success";
      case "failed":
        return "bg-error";
      default:
        return "bg-muted-foreground";
    }
  }
</script>

<div class="relative flex gap-1">
  <div class="relative w-3 shrink-0" aria-hidden="true">
    {#if !(isFirst && isLast)}
      <span
        class="absolute left-1/2 w-px -translate-x-1/2 bg-border/70 {isFirst
          ? 'bottom-0 top-[18px]'
          : isLast
            ? 'bottom-[calc(100%-18px)] top-0'
            : 'inset-y-0'}"
      ></span>
    {/if}
    <span
      class="absolute left-1/2 top-[18px] z-10 flex size-3 -translate-x-1/2 -translate-y-1/2 items-center justify-center rounded-full border border-border bg-background"
    >
      {#if tool.status === "running"}
        <span class="relative flex size-1.5">
          <span
            class="absolute size-full animate-ping rounded-full bg-primary/60"
          ></span>
          <span class="relative size-1.5 rounded-full bg-primary"></span>
        </span>
      {:else}
        <span class="size-1.5 rounded-full {statusDotClass(tool.status)}"
        ></span>
      {/if}
    </span>
  </div>

  <div class="min-w-0 flex-1 py-1">
    <div
      class="flex min-h-7 items-center gap-2 rounded-md px-0.5 transition-colors hover:bg-secondary/40"
    >
      <button
        type="button"
        onclick={() => (expanded = !expanded)}
        class="flex min-w-0 flex-1 items-center gap-2 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        aria-expanded={expanded}
      >
        <ToolIcon
          toolName={tool.tool_name}
          isRunning={tool.status === "running"}
          className="size-3.5 shrink-0 text-muted-foreground"
        />
        <span class="shrink-0 text-xs font-medium text-foreground">{label}</span
        >
        {#if target}
          <span
            class="min-w-0 flex-1 truncate text-[11px] text-muted-foreground {isSubagent
              ? ''
              : 'font-mono'}"
          >
            {target}
          </span>
        {:else if tool.arguments}
          <span
            class="min-w-0 flex-1 truncate font-mono text-[11px] text-muted-foreground"
          >
            {compactArgs(tool.arguments, 80)}
          </span>
        {:else}
          <span class="flex-1"></span>
        {/if}
        {#if meta}
          <span
            class="hidden shrink-0 text-[10px] text-muted-foreground/70 sm:inline"
          >
            {meta}
          </span>
        {/if}
        {#if tool.elapsed_ms && tool.elapsed_ms > 0}
          <span
            class="shrink-0 text-[11px] tabular-nums text-muted-foreground/70"
          >
            {formatElapsed(tool.elapsed_ms)}
          </span>
        {/if}
        {#if expanded}
          <ChevronDown class="size-3.5 shrink-0 text-muted-foreground" />
        {:else}
          <ChevronRight class="size-3.5 shrink-0 text-muted-foreground" />
        {/if}
      </button>

      {#if tool.subagent_session_id}
        <button
          type="button"
          onclick={() => handleJumpToSubagent(tool.subagent_session_id!)}
          class="group/open inline-flex h-6 shrink-0 items-center gap-1 rounded px-1.5 text-[11px] font-medium text-muted-foreground transition-colors hover:bg-primary/10 hover:text-primary focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          title="Open subagent session"
          aria-label="Open subagent session"
        >
          <span class="hidden sm:inline">Open</span>
          <ArrowRight
            class="size-3 transition-transform group-hover/open:translate-x-0.5"
          />
        </button>
      {/if}
    </div>

    {#if expanded}
      <div class="mt-1 overflow-hidden px-0.5">
        <ToolBody {tool} embedded />
      </div>
    {/if}
  </div>
</div>
