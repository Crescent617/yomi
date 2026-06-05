<script lang="ts">
  import { onMount } from "svelte";
  import * as api from "../../lib/api";
  import { Wrench, RefreshCw } from "lucide-svelte";

  let skills = $state<unknown[]>([]);
  let loading = $state(true);
  let error = $state("");

  onMount(async () => {
    try {
      skills = await api.listSkills();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      loading = false;
    }
  });

  async function reload() {
    try {
      await api.reloadConfig();
      skills = await api.listSkills();
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    }
  }
</script>

<div class="p-6 max-w-2xl mx-auto">
  <div class="flex items-center justify-between mb-6">
    <h1 class="text-2xl font-bold flex items-center gap-2">
      <Wrench size={24} />
      Skills
    </h1>
    <button
      onclick={reload}
      class="inline-flex items-center gap-1.5 px-3 py-2 rounded-lg border border-border hover:bg-secondary transition-colors text-sm"
    >
      <RefreshCw size={14} />
      Reload
    </button>
  </div>

  {#if loading}
    <div class="text-muted-foreground">Loading skills...</div>
  {:else if error}
    <div class="text-destructive">{error}</div>
  {:else if skills.length === 0}
    <div class="text-muted-foreground">No skills loaded</div>
  {:else}
    <div class="space-y-2">
      {#each skills as skill (JSON.stringify(skill))}
        <div class="rounded-lg border border-border p-4 hover:bg-secondary/50 transition-colors">
          <pre class="text-xs overflow-x-auto">{JSON.stringify(skill, null, 2)}</pre>
        </div>
      {/each}
    </div>
  {/if}
</div>
