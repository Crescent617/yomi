<script lang="ts">
  // 中断标记的优雅呈现：一条细分隔线 + 居中 micro-label，作为 turn 的
  // 视觉终点，而非又一条"用户消息"。
  let { text }: { text: string } = $props();

  const label = $derived(
    text.trim().match(/^\[interrupted:\s*(.+?)\s*\]$/)?.[1] ?? text.trim(),
  );
</script>

<div
  class="my-4 flex items-center gap-3"
  role="separator"
  aria-label="interrupted"
>
  <div class="h-px flex-1 bg-subtle"></div>
  <span class="text-[10px] uppercase leading-none text-muted-foreground"
    >interrupted · {label}</span
  >
  <div class="h-px flex-1 bg-subtle"></div>
</div>
