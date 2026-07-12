<script lang="ts">
  import type { SessionState } from "../../state.svelte";
  import { getSession } from "../../state.svelte";
  import { activateSession } from "../../session";
  import { ArrowUpRight, Bot } from "lucide-svelte";

  let { session }: { session: SessionState } = $props();

  interface BreadcrumbItem {
    id: string;
    label: string;
    isSubagent: boolean;
  }

  function buildChain(s: SessionState): BreadcrumbItem[] {
    const items: BreadcrumbItem[] = [];
    let current: SessionState | undefined = s;
    const seen = new Set<string>();
    while (current && !seen.has(current.id)) {
      seen.add(current.id);
      items.push({
        id: current.id,
        label: current.alias || current.id.slice(0, 8),
        isSubagent: !!current.parent_session_id,
      });
      if (!current.parent_session_id) break;
      const parentId = current.parent_session_id;
      current = getSession(parentId);
      if (!current) {
        // Parent not loaded — show ellipsis placeholder with its ID for navigation
        items.push({
          id: parentId,
          label: "…",
          isSubagent: false,
        });
        break;
      }
    }
    return items.reverse();
  }

  const chain = $derived(buildChain(session));

  async function handleClick(id: string) {
    if (id === session.id) return;
    await activateSession(id);
  }
</script>

{#if chain.length > 1}
  <div
    class="flex items-center gap-1 text-xs px-3 py-1.5 border-b border-subtle"
  >
    {#each chain as item, i (item.id)}
      {#if i > 0}
        <span class="text-muted-foreground opacity-50">/</span>
      {/if}
      {#if item.id === session.id}
        <span class="flex items-center gap-1 font-medium text-foreground">
          {#if item.isSubagent}
            <Bot class="w-3 h-3 opacity-60" />
          {/if}
          {item.label}
        </span>
      {:else}
        <button
          type="button"
          class="flex items-center gap-1 text-muted-foreground hover:text-foreground transition-colors cursor-pointer"
          onclick={() => handleClick(item.id)}
        >
          {#if item.isSubagent}
            <Bot class="w-3 h-3 opacity-60" />
          {/if}
          {item.label}
          {#if item.label !== "…"}
            <ArrowUpRight class="w-2.5 h-2.5 opacity-50" />
          {/if}
        </button>
      {/if}
    {/each}
  </div>
{/if}
