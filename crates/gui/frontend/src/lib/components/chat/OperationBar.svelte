<script lang="ts">
  import { Copy, Undo } from "lucide-svelte";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";
  import type { ChatMessage } from "../../state.svelte";
  import { showNotification, getSession } from "../../state.svelte";
  import * as api from "../../api";

  let {
    message,
    sessionId,
  }: {
    message: ChatMessage;
    sessionId: string;
  } = $props();

  const checkpoints = $derived(getSession(sessionId)?.checkpoints ?? []);

  const hasCheckpoint = $derived(
    Array.isArray(checkpoints) &&
      checkpoints.some(
        (cp: any) => cp.messageId === message.id || cp.id === message.id,
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
      await api.rewind(sessionId, message.id);
    } catch (e) {
      showNotification(
        "Failed to revert: " + (e instanceof Error ? e.message : ""),
        "error",
        3000,
      );
    }
  }

  async function copyText() {
    try {
      await navigator.clipboard.writeText(message.content || "");
      showNotification("Text copied", "success", 2000);
    } catch {
      showNotification("Failed to copy text", "error", 2000);
    }
  }

</script>

<div class="flex items-center gap-0 opacity-0 group-hover:opacity-100 transition-opacity">
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
