<script lang="ts">
  import { Trash2 } from "lucide-svelte";

  /**
   * 长按删除按钮：按下后 destructive 色从左侧扫满（1s）即触发删除，
   * 中途松手取消——确认动作的本地反馈，免确认弹窗。
   */
  interface Props {
    /** aria-label / title 前缀（会自动追加"长按确认"提示） */
    label: string;
    /** 图标尺寸 */
    size?: number;
    /** 可选文字（有文字时按钮呈 pill 形） */
    text?: string;
    disabled?: boolean;
    ondelete: () => void;
    class?: string;
  }

  let {
    label,
    size = 16,
    text,
    disabled = false,
    ondelete,
    class: cls = "",
  }: Props = $props();

  let holding = $state(false);
  let fired = false;

  function down(e: PointerEvent) {
    if (disabled || e.button !== 0) return;
    fired = false;
    holding = true;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
  }

  function release() {
    holding = false;
  }

  function complete() {
    if (!holding || fired) return;
    fired = true;
    holding = false;
    ondelete();
  }
</script>

<button
  type="button"
  class="lpd {text ? 'px-3 py-1.5' : 'size-8'} {cls}"
  class:holding
  {disabled}
  title="{label} · 长按确认"
  aria-label="{label}（长按确认）"
  onpointerdown={down}
  onpointerup={release}
  onpointercancel={release}
  oncontextmenu={(e) => e.preventDefault()}
>
  <span class="fill" onanimationend={complete}></span>
  <Trash2 {size} class="rel shrink-0" />
  {#if text}<span class="rel">{text}</span>{/if}
</button>

<style>
  .lpd {
    position: relative;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    gap: 0.375rem;
    overflow: hidden;
    border-radius: 0.375rem;
    font-size: 0.875rem;
    color: hsl(var(--muted-foreground));
    transition:
      color 0.15s ease,
      background-color 0.15s ease;
    touch-action: manipulation;
    user-select: none;
    -webkit-user-select: none;
  }
  .lpd:hover:not(:disabled) {
    color: hsl(var(--destructive));
    background: hsl(var(--destructive) / 0.1);
  }
  .lpd:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .lpd:focus-visible {
    outline: none;
    box-shadow: 0 0 0 1px hsl(var(--ring));
  }
  .rel {
    position: relative;
  }
  .fill {
    position: absolute;
    inset: 0;
    background: hsl(var(--destructive) / 0.22);
    transform: scaleX(0);
    transform-origin: left;
    pointer-events: none;
  }
  .holding {
    color: hsl(var(--destructive));
  }
  .holding .fill {
    animation: lpd-fill 1s linear forwards;
  }
  @keyframes lpd-fill {
    to {
      transform: scaleX(1);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .holding .fill {
      animation-duration: 1ms;
    }
  }
</style>
