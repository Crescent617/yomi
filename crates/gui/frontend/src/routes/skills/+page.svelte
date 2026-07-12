<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../lib/api";
  import { sessionState } from "../../lib/state.svelte";
  import LoadingIndicator from "../../lib/components/ui/LoadingIndicator.svelte";
  import LoadingSkeleton from "../../lib/components/ui/LoadingSkeleton.svelte";
  import { Wrench, RefreshCw } from "lucide-svelte";

  let skills = $state<api.SkillInfo[]>([]);
  let loading = $state(true);
  let error = $state("");

  async function loadSkills() {
    const session_id = sessionState.activeSessionId;
    if (!session_id) {
      skills = [];
      loading = false;
      return;
    }
    try {
      loading = true;
      error = "";
      skills = await api.listSessionSkills(session_id);
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  onMount(loadSkills);

  async function refresh() {
    await loadSkills();
  }
</script>

<div class="p-6 max-w-2xl mx-auto">
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-2xl font-bold flex items-center gap-2">
      <Wrench size={24} />
      Skills
    </h1>
    <button
      onclick={refresh}
      disabled={loading}
      class="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg border border-border hover:bg-secondary transition-colors text-sm disabled:cursor-wait disabled:opacity-60"
    >
      {#if loading}
        <LoadingIndicator size="sm" label="Refreshing skills" />
      {:else}
        <RefreshCw size={14} />
      {/if}
      Refresh
    </button>
  </div>

  {#if loading && skills.length === 0}
    <div class="space-y-3" role="status" aria-label="Loading skills">
      {#each Array(3) as _, index (index)}
        <div class="space-y-2 rounded-lg border border-border p-4">
          <LoadingSkeleton class="h-4 w-32" />
          <LoadingSkeleton class="h-3 w-3/4" />
        </div>
      {/each}
    </div>
  {:else if error}
    <div class="text-destructive">{error}</div>
  {:else if skills.length === 0}
    <div class="text-muted-foreground">No skills loaded</div>
  {:else}
    <div class="space-y-2">
      {#each skills as skill (skill.name)}
        <div
          class="rounded-lg border border-border p-4 hover:bg-secondary/50 transition-colors"
        >
          <div class="font-medium">{skill.name}</div>
          {#if skill.description}
            <div class="text-sm text-muted-foreground mt-1">
              {skill.description}
            </div>
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>
