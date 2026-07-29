<script lang="ts">
  import { ArrowRight, Bot } from "lucide-svelte";
  import type { SubagentInfo } from "../../api";
  import { activateSession } from "../../session";
  import StatusDot from "../layout/StatusDot.svelte";
  import {
    formatSubagentPhase,
    subagentDescription,
  } from "./running-subagents";

  let { subagents }: { subagents: SubagentInfo[] } = $props();
</script>

<div class="py-1">
  {#each subagents as subagent (subagent.id)}
    <button
      type="button"
      onclick={() => void activateSession(subagent.id)}
      class="popover-list-item group flex w-full items-center gap-2.5 px-3 py-2 text-left"
      aria-label={`Open ${subagentDescription(subagent)}`}
    >
      <span
        class="flex size-6 shrink-0 items-center justify-center rounded-md bg-info/10 text-info"
      >
        <Bot class="size-3.5" />
      </span>
      <span class="min-w-0 flex-1">
        <span class="block truncate text-sm font-medium text-foreground">
          {subagentDescription(subagent)}
        </span>
        <span
          class="flex items-center gap-1.5 text-[11px] capitalize text-muted-foreground"
        >
          <StatusDot phase={subagent.phase} />
          {formatSubagentPhase(subagent.phase)}
          {#if subagent.model_key}
            <span aria-hidden="true">·</span>
            <span class="truncate normal-case">{subagent.model_key}</span>
          {/if}
        </span>
      </span>
      <ArrowRight
        class="size-3.5 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5 group-hover:text-foreground"
      />
    </button>
  {/each}
</div>
