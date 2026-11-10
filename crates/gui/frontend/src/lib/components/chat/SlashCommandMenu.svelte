<script lang="ts">
  // 斜杠命令补全下拉，主窗口与小窗输入框共用。自身负责选中项滚动可见；
  // 定位与显隐由调用方（放在相对定位容器里，按需渲染）。
  import { Command } from "lucide-svelte";

  let {
    commands,
    selected,
    onSelect,
  }: {
    commands: readonly (readonly [string, string])[];
    selected: number;
    onSelect: (cmd: string) => void;
  } = $props();

  let listEl = $state<HTMLDivElement | null>(null);

  $effect(() => {
    if (!listEl) return;
    const buttons = listEl.querySelectorAll("button");
    buttons[selected]?.scrollIntoView({ block: "nearest", inline: "nearest" });
  });
</script>

<div
  bind:this={listEl}
  class="absolute bottom-full left-0 right-0 mb-1 mx-3 max-h-48 overflow-y-auto rounded-lg border border-border bg-background shadow-lg z-50"
>
  {#each commands as [cmd, desc], i (cmd)}
    <button
      type="button"
      class="flex w-full items-center gap-2 px-3 py-2 text-left text-sm transition-colors {i ===
      selected
        ? 'bg-secondary'
        : 'hover:bg-secondary/50'}"
      onclick={() => onSelect(cmd)}
    >
      <Command size={14} class="shrink-0 text-muted-foreground" />
      <span class="shrink-0 font-mono text-primary">{cmd}</span>
      <span class="truncate text-xs text-muted-foreground">{desc}</span>
    </button>
  {/each}
</div>
