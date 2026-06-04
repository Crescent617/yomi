<script lang="ts">
  import { Edit3, X, Send, Navigation } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import type { TaggedContentBlock } from "../../types";
  import { showNotification } from "../../state.svelte";

  let { session, onEdit, onSteer }: { session: SessionState; onEdit: (text: string) => void; onSteer: (blocks: TaggedContentBlock[]) => void } = $props();

  function handleSteer() {
    if (!session.queuedInput) return;
    const blocks = session.queuedInput.blocks ?? [{ type: "text", text: session.queuedInput.text }];
    onSteer(blocks);
    session.queuedInput = null;
  }

  function handleEdit() {
    if (!session.queuedInput) return;
    onEdit(session.queuedInput.text);
    session.queuedInput = null;
  }

  function handleCancel() {
    session.queuedInput = null;
    showNotification("Queued message cancelled", "info", 2000);
  }
</script>

{#if session.queuedInput}
  <div class="mx-4 mb-2 rounded-lg border border-border bg-secondary/50 px-3 py-2 flex items-center gap-3">
    <Send class="w-3.5 h-3.5 text-muted-foreground shrink-0" />
    <div class="flex-1 min-w-0">
      <div class="text-xs text-muted-foreground mb-0.5">Queued — will send when streaming ends</div>
      <div class="text-sm truncate">{session.queuedInput.text}</div>
    </div>
    <button
      type="button"
      onclick={handleSteer}
      class="shrink-0 inline-flex items-center gap-1 text-xs text-primary hover:text-primary/80 transition-colors"
      title="Send as steer message"
    >
      <Navigation class="w-3.5 h-3.5" />
      Steer
    </button>
    <button
      type="button"
      onclick={handleEdit}
      class="shrink-0 inline-flex items-center gap-1 text-xs text-primary hover:text-primary/80 transition-colors"
      title="Edit queued message"
    >
      <Edit3 class="w-3.5 h-3.5" />
      Edit
    </button>
    <button
      type="button"
      onclick={handleCancel}
      class="shrink-0 inline-flex items-center gap-1 text-xs text-muted-foreground hover:text-destructive transition-colors"
      title="Cancel queued message"
    >
      <X class="w-3.5 h-3.5" />
      Cancel
    </button>
  </div>
{/if}
