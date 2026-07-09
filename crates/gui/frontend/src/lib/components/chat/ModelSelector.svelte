<script lang="ts">
  import {
    getModels,
    getSessionModel,
    setSessionModel,
    errorMessage,
    type ModelInfo,
  } from "../../api";
  import { getSession } from "../../state.svelte";
  import { Cpu } from "lucide-svelte";

  interface Props {
    session_id?: string;
  }

  let { session_id }: Props = $props();

  let models = $state<ModelInfo[]>([]);
  let activeModel = $state<string>("");
  let loading = $state(false);
  let error = $state<string | null>(null);
  let open = $state(false);
  let dropdownRef = $state<HTMLDivElement | null>(null);
  let buttonRef = $state<HTMLButtonElement | null>(null);

  $effect(() => {
    loadModels();
    if (session_id) {
      loadSessionModel(session_id);
    }
  });

  async function loadModels() {
    try {
      const res = await getModels();
      models = res.models;
      if (!session_id && !activeModel && models.length > 0) {
        activeModel = models[0].name;
      }
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function loadSessionModel(sid: string) {
    try {
      const key = await getSessionModel(sid);
      // Discard stale responses if the prop changed while awaiting
      if (session_id !== sid) return;
      activeModel = key;
      error = null;
      // Note: do NOT write the resolved default into session.model_key —
      // sessions using the default model keep model_key = null semantics.
    } catch (e) {
      if (session_id !== sid) return;
      error = errorMessage(e);
    }
  }

  async function selectModel(key: string) {
    if (key === activeModel) {
      open = false;
      return;
    }
    if (!session_id) {
      activeModel = key;
      open = false;
      return;
    }
    const sid = session_id;
    loading = true;
    try {
      await setSessionModel(sid, key);
      // Update local session state so InfoBar reacts immediately
      const session = getSession(sid);
      if (session) session.model_key = key;
      // Only touch local display state if we're still on the same session
      if (session_id === sid) {
        activeModel = key;
        error = null;
      }
    } catch (e) {
      if (session_id === sid) error = errorMessage(e);
    } finally {
      loading = false;
      open = false;
    }
  }

  export function getActiveModel(): string {
    return activeModel;
  }

  function handleClickOutside(e: MouseEvent) {
    const target = e.target as HTMLElement;
    if (
      open &&
      dropdownRef &&
      !dropdownRef.contains(target) &&
      buttonRef &&
      !buttonRef.contains(target)
    ) {
      open = false;
    }
  }

  const activeLabel = $derived.by(() => {
    const m = models.find((m) => m.name === activeModel);
    return m ? m.name : activeModel || "Model";
  });
</script>

<svelte:window onclick={handleClickOutside} />

{#if models.length > 1}
  <div class="relative">
    <button
      bind:this={buttonRef}
      type="button"
      onclick={() => (open = !open)}
      class="flex items-center gap-1 px-2 py-1 rounded-md hover:bg-secondary/80 transition-colors text-xs text-muted-foreground hover:text-foreground border border-transparent hover:border-border"
      title={error ?? "Switch model"}
      disabled={loading}
    >
      <Cpu size={12} />
      <span class="max-w-[220px] truncate">{activeLabel}</span>
      {#if loading}
        <span class="animate-spin ml-0.5">⟳</span>
      {/if}
    </button>

    {#if open}
      <div
        bind:this={dropdownRef}
        class="absolute bottom-full mb-1 left-0 z-20 w-72 rounded-md border border-border bg-popover shadow-md py-1"
      >
        {#each models as m (m.name)}
          <button
            class="w-full flex items-center gap-2 px-3 py-1.5 text-xs text-left hover:bg-secondary/50 {m.name ===
            activeModel
              ? 'bg-secondary/30 font-medium'
              : ''}"
            onclick={() => selectModel(m.name)}
          >
            <span class="truncate">{m.name} ({m.model_id})</span>
            <span class="text-muted-foreground/40 shrink-0 ml-auto"
              >{m.context_window.toLocaleString()} ctx</span
            >
          </button>
        {/each}
      </div>
    {/if}
  </div>
{/if}
