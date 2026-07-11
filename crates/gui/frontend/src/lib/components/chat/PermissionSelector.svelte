<script lang="ts">
  import { Check } from "lucide-svelte";
  import {
    levelColor,
    levelDescription,
    levelIcon,
    levelLabel,
    type PermissionLevel,
  } from "../../permission";

  let {
    value,
    onSelect,
    disabled = false,
  }: {
    value: PermissionLevel;
    onSelect: (level: PermissionLevel) => void | Promise<void>;
    disabled?: boolean;
  } = $props();

  let open = $state(false);
  let root: HTMLDivElement | null = $state(null);
  const levels: PermissionLevel[] = ["safe", "caution", "dangerous"];
  const ActiveIcon = $derived(levelIcon(value));

  function select(level: PermissionLevel) {
    open = false;
    void onSelect(level);
  }

  function handleWindowClick(event: MouseEvent) {
    if (open && root && !root.contains(event.target as Node)) open = false;
  }
</script>

<svelte:window onclick={handleWindowClick} />

<div class="relative" bind:this={root}>
  <button
    type="button"
    onclick={() => (open = !open)}
    onkeydown={(event) => {
      if (event.key === "Escape") open = false;
    }}
    class="inline-flex h-7 w-7 items-center justify-center rounded-md border transition-colors hover:bg-secondary disabled:cursor-not-allowed disabled:opacity-50 {levelColor(
      value,
    )}"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={`Permission level: ${levelLabel(value)}`}
    title={levelDescription(value)}
    {disabled}
  >
    <ActiveIcon size={14} />
  </button>

  {#if open}
    <div
      class="absolute bottom-full left-0 z-50 mb-1 w-72 overflow-hidden rounded-lg border border-border bg-popover py-1 shadow-lg"
      role="menu"
      aria-label="Permission level"
    >
      {#each levels as level (level)}
        {@const Icon = levelIcon(level)}
        <button
          type="button"
          role="menuitemradio"
          aria-checked={value === level}
          onclick={() => select(level)}
          class="flex w-full items-start gap-2.5 px-3 py-2 text-left transition-colors hover:bg-secondary/70 {value ===
          level
            ? 'bg-secondary/50'
            : ''}"
        >
          <Icon
            size={15}
            class="mt-0.5 shrink-0 {level === 'safe'
              ? 'text-success'
              : level === 'caution'
                ? 'text-warning'
                : 'text-error'}"
          />
          <span class="min-w-0 flex-1">
            <span class="block text-xs font-medium text-foreground">
              {levelLabel(level)}
            </span>
            <span class="mt-0.5 block text-[11px] text-muted-foreground">
              {levelDescription(level)}
            </span>
          </span>
          {#if value === level}
            <Check size={14} class="mt-0.5 shrink-0 text-primary" />
          {/if}
        </button>
      {/each}
    </div>
  {/if}
</div>
