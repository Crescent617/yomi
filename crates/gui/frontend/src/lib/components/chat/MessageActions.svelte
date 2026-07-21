<script lang="ts">
  import { onDestroy } from "svelte";
  import { Star, Copy, Check, ImageDown, LoaderCircle } from "lucide-svelte";
  import {
    getSession,
    showNotification,
    type BotMessage,
  } from "../../state.svelte";
  import { favoriteIdFor, toggleFavorite } from "../../favorites.svelte";
  import { shareAnswerAsImage } from "../../share-card";

  let {
    session_id,
    message,
    content,
    isStreaming = false,
  }: {
    session_id: string;
    message: BotMessage;
    /** Plain markdown text of the answer (computed by the parent). */
    content: string;
    isStreaming?: boolean;
  } = $props();

  const favoriteId = $derived(favoriteIdFor(session_id, message.id));

  let copied = $state(false);
  let sharing = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  onDestroy(() => clearTimeout(copyTimer));

  async function onToggleFavorite() {
    await toggleFavorite({
      session_id,
      message_id: message.id,
      content,
      session_title: getSession(session_id)?.alias,
      message_created_at: message.created_at,
    });
  }

  async function onCopy() {
    try {
      await navigator.clipboard.writeText(content);
      clearTimeout(copyTimer);
      copied = true;
      copyTimer = setTimeout(() => (copied = false), 1500);
    } catch (e) {
      console.error("Failed to copy answer:", e);
      showNotification("Failed to copy", "error");
    }
  }

  async function onShare() {
    if (sharing) return;
    sharing = true;
    try {
      const path = await shareAnswerAsImage({
        content,
        sessionTitle: getSession(session_id)?.alias,
        date: message.created_at ? new Date(message.created_at) : new Date(),
      });
      if (path) showNotification("Share image saved", "success");
    } catch (e) {
      console.error("Failed to create share image:", e);
      showNotification("Failed to create share image", "error");
    } finally {
      sharing = false;
    }
  }

  const btnClass =
    "p-1.5 rounded-md transition-colors hover:bg-accent hover:text-foreground";
</script>

{#if !isStreaming && content}
  <div
    class="absolute -top-2.5 right-1 z-10 flex items-center gap-0.5 rounded-lg border border-border bg-card p-0.5 shadow-sm transition-opacity
           {favoriteId
      ? 'opacity-100'
      : 'opacity-0 pointer-events-none group-hover/ma:opacity-100 group-hover/ma:pointer-events-auto focus-within:opacity-100 focus-within:pointer-events-auto'}"
  >
    <button
      type="button"
      onclick={onToggleFavorite}
      class="{btnClass} {favoriteId
        ? 'text-warning'
        : 'text-muted-foreground'}"
      title={favoriteId ? "Remove from favorites" : "Add to favorites"}
      aria-label={favoriteId ? "Remove from favorites" : "Add to favorites"}
    >
      <Star class="w-3.5 h-3.5" fill={favoriteId ? "currentColor" : "none"} />
    </button>
    <button
      type="button"
      onclick={onCopy}
      class="{btnClass} {copied ? 'text-success' : 'text-muted-foreground'}"
      title={copied ? "Copied" : "Copy as markdown"}
      aria-label={copied ? "Copied" : "Copy as markdown"}
    >
      {#if copied}
        <Check class="w-3.5 h-3.5" />
      {:else}
        <Copy class="w-3.5 h-3.5" />
      {/if}
    </button>
    <button
      type="button"
      onclick={onShare}
      disabled={sharing}
      class="{btnClass} text-muted-foreground disabled:opacity-50"
      title="Share as image"
      aria-label="Share as image"
    >
      {#if sharing}
        <LoaderCircle class="w-3.5 h-3.5 animate-spin" />
      {:else}
        <ImageDown class="w-3.5 h-3.5" />
      {/if}
    </button>
  </div>
{/if}
