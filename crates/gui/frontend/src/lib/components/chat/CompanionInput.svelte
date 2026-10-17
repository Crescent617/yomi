<script lang="ts">
  // 伙伴窗 slim 输入：Enter 发送、Shift+Enter 换行。不用完整 ChatInput——
  // 面板定位是"给正在跑的 agent 快速补一句"，附件/模型选择等 chrome
  // 留在主窗口。
  import { ArrowUp } from "lucide-svelte";

  let {
    disabled,
    placeholder,
    onSend,
  }: {
    disabled: boolean;
    placeholder: string;
    onSend: (text: string) => Promise<void>;
  } = $props();

  let text = $state("");
  let sending = $state(false);
  let textarea: HTMLTextAreaElement | undefined;

  const can_send = $derived(!disabled && !sending && text.trim().length > 0);

  async function submit() {
    const value = text.trim();
    if (!can_send || !value) return;
    sending = true;
    try {
      await onSend(value);
      text = "";
      resize();
    } catch {
      // 发送失败保留草稿（错误提示由 onSend 负责上报）。
    } finally {
      sending = false;
      textarea?.focus();
    }
  }

  function onkeydown(event: KeyboardEvent) {
    if (event.key === "Enter" && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      void submit();
    }
  }

  function resize() {
    if (!textarea) return;
    textarea.style.height = "auto";
    textarea.style.height = `${Math.min(textarea.scrollHeight, 120)}px`;
  }

  export function focus() {
    textarea?.focus();
  }
</script>

<div class="border-t border-subtle p-2">
  <div
    class="flex items-end gap-2 rounded-md border border-subtle bg-code-bg px-2 py-1.5 focus-within:border-primary"
  >
    <textarea
      bind:this={textarea}
      bind:value={text}
      oninput={resize}
      {onkeydown}
      {disabled}
      {placeholder}
      rows="1"
      aria-label="Companion message input"
      class="max-h-[120px] flex-1 resize-none bg-transparent text-sm outline-none placeholder:text-muted-foreground disabled:opacity-50"
    ></textarea>
    <button
      type="button"
      onclick={() => void submit()}
      disabled={!can_send}
      title="Send message"
      aria-label="Send message"
      class="rounded-full bg-primary/10 p-1.5 text-primary transition-colors hover:bg-primary/20 disabled:bg-transparent disabled:text-muted-foreground"
    >
      <ArrowUp size={14} />
    </button>
  </div>
</div>
