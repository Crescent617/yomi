<script lang="ts">
  import { Check, Copy, Undo2 } from "lucide-svelte";
  import LoadingIndicator from "../ui/LoadingIndicator.svelte";
  import ConfirmDialog from "../ui/ConfirmDialog.svelte";
  import type { Message } from "../../state.svelte";
  import { showNotification, getSession } from "../../state.svelte";
  import { textFromBlocks } from "../../session";
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
        (checkpoint) =>
          checkpoint.message_id === message.id || checkpoint.id === message.id,
      ),
  );

  let showConfirm = $state(false);
  let copied = $state(false);
  let reverting = $state(false);
  let copyResetTimer: ReturnType<typeof setTimeout> | null = null;

  function openConfirm() {
    if (!reverting) showConfirm = true;
  }

  function closeConfirm() {
    if (!reverting) showConfirm = false;
  }

  async function doRevert() {
    if (reverting) return;
    reverting = true;
    try {
      await api.rewind(session_id, message.id);
      showConfirm = false;
    } catch (e) {
      showNotification("Failed to rewind: " + api.errorMessage(e), "error");
    } finally {
      reverting = false;
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
      copied = true;
      if (copyResetTimer) clearTimeout(copyResetTimer);
      copyResetTimer = setTimeout(() => {
        copied = false;
        copyResetTimer = null;
      }, 1500);
    } catch {
      showNotification("Failed to copy text", "error");
    }
  }
</script>

<div
  class="inline-flex flex-col items-center gap-0.5 rounded-lg border border-border/60 bg-background/95 p-0.5 shadow-sm backdrop-blur-sm"
>
  <button
    type="button"
    onclick={copyText}
    class="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    class:text-success={copied}
    aria-label={copied ? "Message copied" : "Copy message"}
    title={copied ? "Copied" : "Copy message"}
  >
    {#if copied}
      <Check size={14} strokeWidth={2.25} />
    {:else}
      <Copy size={14} strokeWidth={2} />
    {/if}
  </button>

  {#if hasCheckpoint}
    <span class="h-px w-4 bg-border/60" aria-hidden="true"></span>
    <button
      type="button"
      onclick={openConfirm}
      disabled={reverting}
      class="inline-flex size-7 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:cursor-wait disabled:opacity-60"
      aria-label="Rewind to this message"
      title="Rewind to this message"
    >
      {#if reverting}
        <LoadingIndicator size="sm" label="Rewinding message" />
      {:else}
        <Undo2 size={14} strokeWidth={2} />
      {/if}
    </button>
  {/if}
</div>

<ConfirmDialog
  open={showConfirm}
  title="Rewind to this message?"
  message="This will remove all messages and changes created after this point."
  confirmText={reverting ? "Rewinding..." : "Rewind"}
  cancelText="Cancel"
  onConfirm={doRevert}
  onCancel={closeConfirm}
/>
