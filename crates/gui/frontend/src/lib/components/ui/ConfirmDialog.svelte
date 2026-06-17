<script lang="ts">
  import { AlertTriangle } from "lucide-svelte";

  interface Props {
    open: boolean;
    title: string;
    message: string;
    confirmText?: string;
    cancelText?: string;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let {
    open,
    title,
    message,
    confirmText = "Confirm",
    cancelText = "Cancel",
    onConfirm,
    onCancel,
  }: Props = $props();

  function handleKeydown(e: KeyboardEvent) {
    if (!open) return;
    if (e.key === "Escape") onCancel();
    if (e.key === "Enter") onConfirm();
  }

  function portal(node: HTMLElement) {
    document.body.appendChild(node);
    return {
      destroy() {
        if (node.parentNode) {
          node.parentNode.removeChild(node);
        }
      },
    };
  }
</script>

<svelte:window onkeydown={handleKeydown} />

{#if open}
  <!-- svelte-ignore a11y_click_events_have_key_events -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    use:portal
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/40 backdrop-blur-sm p-4"
    onclick={(e) => {
      if (e.target === e.currentTarget) onCancel();
    }}
  >
    <div
      class="bg-background rounded-xl border border-border shadow-2xl w-full max-w-sm flex flex-col"
    >
      <div class="flex items-center gap-3 px-5 py-4 border-b border-border">
        <AlertTriangle class="w-5 h-5 text-amber-500 shrink-0" />
        <h2 class="text-sm font-semibold">{title}</h2>
      </div>
      <div
        class="px-5 py-4 text-sm text-muted-foreground leading-relaxed whitespace-pre-line"
      >
        {message}
      </div>
      <div
        class="flex items-center justify-end gap-2 px-5 py-4 border-t border-border"
      >
        <button
          type="button"
          onclick={onCancel}
          class="px-4 py-2 rounded-lg text-sm border border-input hover:bg-secondary transition-colors"
        >
          {cancelText}
        </button>
        <button
          type="button"
          onclick={onConfirm}
          class="px-4 py-2 rounded-lg text-sm bg-destructive text-destructive-foreground hover:bg-destructive/90 transition-colors"
        >
          {confirmText}
        </button>
      </div>
    </div>
  </div>
{/if}
