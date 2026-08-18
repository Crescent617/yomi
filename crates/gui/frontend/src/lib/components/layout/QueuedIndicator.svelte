<script lang="ts">
  import { Send } from "lucide-svelte";
  import { pendingOf } from "../../mailbox.svelte";

  /**
   * Pending indicator for session lists (event-driven: the mailbox_changed
   * notification keeps the counts fresh).
   * - icon: small send glyph for list rows
   * - dot: corner badge for avatar stacks
   */
  let {
    session_id,
    variant = "icon",
  }: {
    session_id: string;
    variant?: "icon" | "dot";
  } = $props();

  const pending = $derived(pendingOf(session_id) > 0);
</script>

{#if pending}
  {#if variant === "dot"}
    <span
      class="absolute -bottom-0.5 -right-0.5 h-1.5 w-1.5 rounded-full bg-muted-foreground ring-2 ring-card"
      aria-label="Message queued"
      title="Message queued"
    ></span>
  {:else}
    <span
      class="inline-flex shrink-0 text-muted-foreground"
      title="Message queued"
    >
      <Send size={10} aria-label="Message queued" />
    </span>
  {/if}
{/if}
