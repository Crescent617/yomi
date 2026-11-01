<script lang="ts">
  import { AlertTriangle } from "lucide-svelte";
  import Modal from "./Modal.svelte";

  interface Props {
    open: boolean;
    title: string;
    message: string;
    confirmText?: string;
    cancelText?: string;
    /** Disable both buttons (e.g. an in-flight action owns the dialog). */
    disabled?: boolean;
    onConfirm: () => void;
    onCancel: () => void;
  }

  let {
    open,
    title,
    message,
    confirmText = "Confirm",
    cancelText = "Cancel",
    disabled = false,
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
      {disabled}
      class="px-4 py-2 rounded-lg text-sm border border-input text-foreground hover:bg-secondary transition-colors disabled:opacity-50"
    >
      {cancelText}
    </button>
    <button
      type="button"
      onclick={onConfirm}
      {disabled}
      class="px-4 py-2 rounded-lg text-sm bg-destructive text-destructive-foreground hover:bg-destructive/90 transition-colors disabled:opacity-50"
    >
      {confirmText}
    </button>
  {/snippet}
</Modal>
