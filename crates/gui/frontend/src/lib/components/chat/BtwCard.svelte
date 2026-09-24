<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { fly } from "svelte/transition";
  import { X, RotateCcw } from "lucide-svelte";
  import { showNotification } from "../../state.svelte";
  import { errorMessage } from "../../api";
  import { hasOpenModal } from "../../modal-stack";
  import {
    btwState,
    closeBtwCard,
    askBtw,
    BTW_TOOL_FALLBACK,
  } from "../../btw.svelte";
  import TextBlock from "./TextBlock.svelte";

  /**
   * `/btw` 旁问浮卡：非模态、锚定对话区右上、关闭即销毁。主对话在底下
   * 照常流式滚动；卡片只响应 Esc/✕ 与悬停滚动，不抢输入框焦点（关闭
   * 按钮 mousedown preventDefault，点击不挪焦点）。答案正文复用消息
   * 流的增量 markdown 管线（TextBlock），流式成本保持 O(delta)。
   * 挂载在 handleChatClick 容器内：答案里的链接与消息流走同一个
   * openDefault 拦截，不会触发 webview 内导航。
   */
  let { sessionId }: { sessionId: string } = $props();

  // 只渲染本会话发起的卡。
  let card = $derived(
    btwState.card && btwState.card.sessionId === sessionId
      ? btwState.card
      : null,
  );

  function handleKeydown(e: KeyboardEvent) {
    // 只关"看得到的"卡（别的会话的卡不动）；有模态层开着时让 Esc 先
    // 服务顶层（与 MessageList 同款 modal-stack 约定）。不 stopPropagation：
    // 输入框/picker 自己的 Esc 语义各行其是。
    if (e.key === "Escape" && card && !hasOpenModal()) closeBtwCard();
  }

  function retry() {
    const c = btwState.card;
    if (!c) return;
    askBtw(c.sessionId, c.question).catch((e) =>
      showNotification(`旁问失败：${errorMessage(e)}`, "error"),
    );
  }

  onMount(() => window.addEventListener("keydown", handleKeydown));
  onDestroy(() => window.removeEventListener("keydown", handleKeydown));
</script>

{#if card}
  <div
    class="absolute right-4 top-4 z-40 flex max-h-[55%] w-[400px] max-w-[calc(100%-2rem)] flex-col overflow-hidden rounded-lg border border-border bg-popover shadow-lg"
    transition:fly={{ y: -8, duration: 150 }}
    aria-label="btw 旁问"
  >
    <div class="flex items-center gap-2 px-3 pt-2.5">
      <span class="micro-label shrink-0 text-primary">BTW</span>
      <span
        class="min-w-0 flex-1 truncate text-xs text-muted-foreground"
        title={card.question}
      >
        {card.question}
      </span>
      <button
        type="button"
        class="shrink-0 rounded-md p-1 text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground"
        onmousedown={(e) => e.preventDefault()}
        onclick={closeBtwCard}
        aria-label="关闭旁问 (Esc)"
        title="关闭 (Esc)"
      >
        <X size={13} aria-hidden="true" />
      </button>
    </div>

    <div
      class="min-h-0 overflow-y-auto overscroll-y-contain px-3 pb-3 pt-1.5 text-sm"
    >
      {#if card.status === "pending" || (card.status === "streaming" && !card.text)}
        <div class="flex items-center gap-1.5 py-1.5" aria-label="等待回答">
          <span class="btw-dot"></span>
          <span class="btw-dot"></span>
          <span class="btw-dot"></span>
        </div>
      {:else}
        {#if card.text}
          <TextBlock
            content={card.text}
            isStreaming={card.status === "streaming"}
          />
        {/if}
        {#if card.toolUseFallback}
          <p class="mt-1 text-sm italic text-muted-foreground">
            {BTW_TOOL_FALLBACK}
          </p>
        {:else if card.status === "done" && !card.text}
          <p class="mt-1 text-sm italic text-muted-foreground">
            （没有回答内容）
          </p>
        {/if}
        {#if card.status === "error"}
          <div class="flex items-center gap-2 text-sm">
            <span class="min-w-0 flex-1 text-error">{card.error}</span>
            <button
              type="button"
              class="inline-flex shrink-0 items-center gap-1 rounded-md border border-border bg-secondary px-2 py-1 text-xs text-secondary-foreground transition-colors hover:bg-accent"
              onmousedown={(e) => e.preventDefault()}
              onclick={retry}
              aria-label="重试旁问"
              title="重试"
            >
              <RotateCcw size={12} aria-hidden="true" />
              重试
            </button>
          </div>
        {/if}
      {/if}
    </div>
  </div>
{/if}

<style>
  .btw-dot {
    width: 5px;
    height: 5px;
    border-radius: 9999px;
    background: hsl(var(--muted-foreground));
    animation: btw-dot-bounce 1.2s ease-in-out infinite;
  }
  .btw-dot:nth-child(2) {
    animation-delay: 0.15s;
  }
  .btw-dot:nth-child(3) {
    animation-delay: 0.3s;
  }
  @keyframes btw-dot-bounce {
    0%,
    100% {
      opacity: 0.3;
      transform: translateY(0);
    }
    40% {
      opacity: 1;
      transform: translateY(-3px);
    }
  }
</style>
