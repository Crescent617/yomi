<script lang="ts">
  import { Copy, Undo } from "lucide-svelte";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";
  import type { Message } from "../../state.svelte";
  import {
    textFromBlocks,
    showNotification,
    getSession,
  } from "../../state.svelte";
  import * as api from "../../api";

  let {
    message,
    session_id,
  }: {
    message: Message;
    session_id: string;
  } = $props();

  const checkpoints = $derived(
    (getSession(session_id)?.checkpoints ?? []) as Array<{
      message_id?: string;
      id?: string;
    }>,
  );

  const hasCheckpoint = $derived(
    Array.isArray(checkpoints) &&
      checkpoints.some(
        (cp: { message_id?: string; id?: string }) =>
          cp.message_id === message.id || cp.id === message.id,
      ),
  );

  let showConfirm = $state(false);

  function openConfirm() {
    showConfirm = true;
  }

  function closeConfirm() {
    showConfirm = false;
  }

  async function doRevert() {
    showConfirm = false;
    try {
      await api.rewind(session_id, message.id);
    } catch (e) {
      showNotification("Failed to revert: " + api.errorMessage(e), "error");
    }
  }

  async function copyText() {
    if (message.type === "tool") return;
    try {
      const text =
        message.type === "error"
          ? message.content
          : textFromBlocks(message.content);
      await navigator.clipboard.writeText(text);
      showNotification("Text copied", "success");
    } catch {
      showNotification("Failed to copy text", "error");
    }
  }
</script>

<div class="flex items-center gap-0 opacity-100 transition-opacity">
  <button
    type="button"
    onclick={copyText}
    class="inline-flex items-center p-1 text-xs text-muted-foreground hover:text-foreground hover:bg-muted/60 rounded transition-colors"
    title="Copy text"
  >
    <Copy size={16} />
  </button>
  {#if hasCheckpoint}
    <button
      type="button"
      onclick={openConfirm}
      class="inline-flex items-center p-1 text-xs text-muted-foreground hover:text-foreground hover:bg-muted/60 rounded transition-colors"
      title="Revert to this checkpoint"
    >
      <Undo size={16} />
    </button>
  {/if}
</div>

<ConfirmDialog
  open={showConfirm}
  title="Revert to checkpoint?"
  message="This will undo all changes and messages after this point."
  confirmText="Revert"
  cancelText="Cancel"
  onConfirm={doRevert}
  onCancel={closeConfirm}
/>
