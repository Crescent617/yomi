<script lang="ts">
  import TextBlock from "./TextBlock.svelte";

  // `<details>/<summary>` 折叠块的 yomi 呈现：安静的一行摘要 + 左侧细线
  // 缩进正文。原生 details 提供交互，样式全走语义色。
  let { summary, body }: { summary: string; body: string } = $props();
</script>

<details class="details-block" open>
  <summary>
    <svg
      class="chevron"
      width="12"
      height="12"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      stroke-width="2"
      stroke-linecap="round"
      stroke-linejoin="round"
      aria-hidden="true"
    >
      <path d="m9 18 6-6-6-6" />
    </svg>
    <span>{summary || "详情"}</span>
  </summary>
  <div class="details-body">
    <TextBlock content={body} isStreaming={false} />
  </div>
</details>

<style>
  .details-block {
    margin: 0.35rem 0;
  }
  summary {
    display: flex;
    align-items: center;
    gap: 0.375rem;
    cursor: pointer;
    user-select: none;
    list-style: none;
    font-size: 0.8125rem;
    color: hsl(var(--muted-foreground));
    transition: color 0.15s ease;
  }
  summary::-webkit-details-marker {
    display: none;
  }
  summary:hover {
    color: hsl(var(--foreground));
  }
  .chevron {
    flex-shrink: 0;
    transition: transform 0.15s ease;
  }
  details[open] > summary .chevron {
    transform: rotate(90deg);
  }
  .details-body {
    margin-top: 0.25rem;
    margin-left: 0.375rem;
    padding-left: 0.75rem;
    border-left: 1px solid hsl(var(--border));
  }
</style>
