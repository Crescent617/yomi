<script lang="ts">
  import {
    Navigation,
    X,
    Pencil,
    Hourglass,
    CornerDownRight,
  } from "lucide-svelte";
  import type { SessionState } from "../../state.svelte";
  import { showNotification } from "../../state.svelte";
  import type { MailboxItem } from "../../api";
  import {
    mailboxBySession,
    retractMailboxItem,
    steerQueueHead,
  } from "../../mailbox.svelte";

  /**
   * Pending 面板：渲染 kernel mailbox 的 pending 条目（steer + queue，
   * 多条堆叠）。本地手势保持单槽语义——queue 队首可 steer/编辑/撤回，
   * steer 条目只能撤回。
   */
  let {
    session,
    onEdit,
  }: {
    session: SessionState;
    onEdit: (text: string) => void;
  } = $props();

  const VISIBLE = 3;

  const snap = $derived(mailboxBySession[session.id]);
  const items = $derived([...(snap?.steer ?? []), ...(snap?.queue ?? [])]);
  const visible = $derived(items.slice(0, VISIBLE));
  const overflow = $derived(items.length - visible.length);

  async function retract(item: MailboxItem) {
    try {
      await retractMailboxItem(session.id, item.id);
    } catch {
      showNotification("Failed to retract the pending message", "error");
    }
  }

  async function steer() {
    try {
      if (await steerQueueHead(session.id)) {
        showNotification("Steer message queued for next step", "info");
      }
    } catch {
      showNotification("Failed to send steer", "error");
    }
  }

  async function edit(item: MailboxItem) {
    const text = item.text ?? item.preview;
    await retract(item);
    onEdit(text);
  }
</script>

{#snippet row(item: MailboxItem)}
  <div class="flex items-center gap-2 py-0.5">
    <span
      class="inline-flex items-center gap-1 text-xs text-muted-foreground shrink-0 w-14"
    >
      {#if item.kind === "steer"}
        <CornerDownRight class="w-3 h-3" />steer
      {:else}
        <Hourglass class="w-3 h-3" />queue
      {/if}
    </span>
    <span class="flex-1 min-w-0 text-sm truncate"
      >{item.preview}{#if item.has_image}
        <span class="text-muted-foreground"> 📎</span>
      {/if}</span
    >
    {#if item.kind === "queue"}
      <button
        type="button"
        onclick={steer}
        class="shrink-0 text-primary hover:text-primary/80 transition-colors"
        title="Steer the message into the current run"
      >
        <Navigation class="w-3.5 h-3.5" />
      </button>
      <button
        type="button"
        onclick={() => edit(item)}
        class="shrink-0 text-muted-foreground hover:text-foreground transition-colors"
        title="Edit and resend"
      >
        <Pencil class="w-3.5 h-3.5" />
      </button>
    {/if}
    <button
      type="button"
      onclick={() => retract(item)}
      class="shrink-0 text-muted-foreground hover:text-foreground transition-colors"
      title="Retract"
    >
      <X class="w-3.5 h-3.5" />
    </button>
  </div>
{/snippet}

{#if items.length > 0}
  <div
    class="mx-4 mb-2 rounded-md border border-border bg-secondary/50 px-3 py-2"
  >
    <div class="text-xs text-muted-foreground mb-1">
      Pending · {items.length}
    </div>
    {#each visible as item (item.id)}
      {@render row(item)}
    {/each}
    {#if overflow > 0}
      <div class="text-xs text-muted-foreground mt-0.5">
        … and {overflow} more
      </div>
    {/if}
  </div>
{/if}
