<script lang="ts">
  import { AlertTriangle } from "lucide-svelte";
  import Modal from "./Modal.svelte";

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
</script>

<Modal
  {open}
  size="sm"
  showClose={false}
  onClose={onCancel}
  onSubmit={onConfirm}
>
  {#snippet header()}
    <div class="flex items-center gap-3">
      <AlertTriangle class="w-5 h-5 text-warning shrink-0" />
      <h2 class="text-sm font-semibold text-foreground">{title}</h2>
    </div>
  {/snippet}

  <div
    class="text-sm text-muted-foreground leading-relaxed whitespace-pre-line"
  >
    {message}
  </div>

  {#snippet footer()}
    <button
      type="button"
      onclick={onCancel}
      class="px-4 py-2 rounded-lg text-sm border border-input text-foreground hover:bg-secondary transition-colors"
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
  {/snippet}
</Modal>
