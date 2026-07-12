<script lang="ts">
  import { ArrowRight, Bot } from "lucide-svelte";
  import type { SubagentInfo } from "../../api";
  import { activateSession } from "../../session";
  import StatusDot from "../layout/StatusDot.svelte";
  import {
    formatSubagentPhase,
    runningSubagentsSummary,
    subagentDescription,
  } from "./running-subagents";

  let {
    subagents,
    compact = false,
  }: { subagents: SubagentInfo[]; compact?: boolean } = $props();
</script>

{#if compact}
  <span
    class="inline-flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground"
  >
    <Bot class="size-3.5 shrink-0 text-info" />
    <span class="truncate">{runningSubagentsSummary(subagents)}</span>
  </span>
{:else}
  <section
    class="border-t border-border px-3 py-2.5"
    aria-labelledby="running-agents-heading"
  >
    <h3
      id="running-agents-heading"
      class="mb-1.5 text-[11px] font-medium uppercase tracking-wide text-muted-foreground"
    >
      Running agents
    </h3>
    <div class="space-y-0.5">
      {#each subagents as subagent (subagent.id)}
        <button
          type="button"
          onclick={() => void activateSession(subagent.id)}
          class="group flex w-full items-center gap-2.5 rounded-md px-1 py-1.5 text-left transition-colors hover:bg-secondary/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
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
  </section>
{/if}
