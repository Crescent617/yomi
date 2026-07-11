<script lang="ts">
  import {
    getModels,
    getSessionModel,
    setSessionModel,
    errorMessage,
    type ModelInfo,
  } from "../../api";
  import { getSession } from "../../state.svelte";
  import { getHomeModel, setHomeModel } from "../../settings.svelte";
  import { formatTokens } from "../../utils";
  import { Cpu } from "lucide-svelte";

  interface Props {
    session_id?: string;
    onContextWindowChange?: (context_window: number) => void;
  }

  let { session_id, onContextWindowChange }: Props = $props();

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
      if (!session_id && models.length > 0) {
        const savedModel = await getHomeModel();
        if (session_id) return;
        const nextModel =
          savedModel && models.some((model) => model.name === savedModel)
            ? savedModel
            : models[0].name;
        activeModel = nextModel;
        if (savedModel !== nextModel) {
          await setHomeModel(nextModel);
        }
      }
    } catch (e) {
      error = errorMessage(e);
    }
  }

  async function loadSessionModel(sid: string) {
    try {
      const key = await getSessionModel(sid);
      if (session_id !== sid) return;
      activeModel = key;
      error = null;
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
      await setHomeModel(key);
      open = false;
      return;
    }
    const sid = session_id;
    loading = true;
    try {
      await setSessionModel(sid, key);
      const session = getSession(sid);
      if (session) session.model_key = key;
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

  const activeModelInfo = $derived(
    models.find((model) => model.name === activeModel) ?? null,
  );

  const activeLabel = $derived(activeModelInfo?.name ?? activeModel ?? "Model");

  $effect(() => {
    onContextWindowChange?.(activeModelInfo?.context_window ?? 0);
  });
</script>

<svelte:window onclick={handleClickOutside} />

{#if activeModelInfo || models.length > 0}
  <div class="relative min-w-0">
    {#if models.length > 1}
      <button
        bind:this={buttonRef}
        type="button"
        onclick={() => (open = !open)}
        class="flex max-w-60 items-center gap-1 rounded-md border border-transparent px-2 py-1 text-xs text-muted-foreground transition-colors hover:border-border hover:bg-secondary/80 hover:text-foreground"
        title={error ?? "Switch model"}
        disabled={loading}
      >
        <Cpu size={12} class="shrink-0" />
        <span class="truncate">{activeLabel}</span>
        {#if loading}
          <span class="ml-0.5 animate-spin">⟳</span>
        {/if}
      </button>
    {:else}
      <span
        class="flex max-w-60 items-center gap-1 px-2 py-1 text-xs text-muted-foreground"
        title={activeModelInfo?.model_id ?? activeLabel}
      >
        <Cpu size={12} class="shrink-0" />
        <span class="truncate">{activeLabel}</span>
      </span>
    {/if}

    {#if open}
      <div
        bind:this={dropdownRef}
        class="absolute bottom-full left-0 z-20 mb-1 w-72 rounded-md border border-border bg-popover py-1 shadow-md"
      >
        {#each models as model (model.name)}
          <button
            class="flex w-full items-center gap-2 px-3 py-1.5 text-left text-xs hover:bg-secondary/50 {model.name ===
            activeModel
              ? 'bg-secondary/30 font-medium'
              : ''}"
            onclick={() => selectModel(model.name)}
          >
            <span class="truncate">{model.name} ({model.model_id})</span>
            <span class="ml-auto shrink-0 text-muted-foreground/50"
              >{formatTokens(model.context_window)} ctx</span
            >
          </button>
        {/each}
      </div>
    {/if}
  </div>
{/if}
