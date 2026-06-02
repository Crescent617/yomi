<script lang="ts">
  import { getActiveSession, getDisplayMessages } from "../../state.svelte";
  import UserBubble from "./UserBubble.svelte";
  import AssistantBubble from "./AssistantBubble.svelte";
  import SystemBubble from "./SystemBubble.svelte";

  const activeSession = $derived(getActiveSession());
  const displayMessages = $derived(getDisplayMessages(activeSession?.id ?? ""));

  let scrollContainer = $state<HTMLDivElement | null>(null);
  let isNearBottom = $state(true);

  let { onNearBottomChange }: { onNearBottomChange?: (near: boolean) => void } = $props();

  function checkNearBottom() {
    if (!scrollContainer) return true;
    const threshold = 20; // px from bottom
    const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
    return scrollHeight - scrollTop - clientHeight <= threshold;
  }

  export function scrollToBottom() {
    if (!scrollContainer) return;
    scrollContainer.scrollTop = scrollContainer.scrollHeight;
    isNearBottom = true;
  }

  // Track last message fingerprint for detecting changes during streaming
  let lastFingerprint = $state("");

  function getFingerprint() {
    if (!displayMessages?.length) return "";
    const last = displayMessages[displayMessages.length - 1];
    const parts: string[] = [last.id, last.role, last.content.length.toString()];
    if (last.thinking) parts.push(last.thinking.content.length.toString());
    if (last.tools) {
      for (const t of last.tools) {
        parts.push(t.id, t.status, (t.output ?? "").length.toString());
      }
    }
    return parts.join("|");
  }

  // Auto-scroll to bottom only when user is already near bottom
  $effect(() => {
    const fp = getFingerprint();
    if (fp === lastFingerprint) return;
    lastFingerprint = fp;

    if (scrollContainer && isNearBottom) {
      requestAnimationFrame(() => {
        scrollContainer!.scrollTop = scrollContainer!.scrollHeight;
      });
    }
  });

  function onScroll() {
    isNearBottom = checkNearBottom();
    onNearBottomChange?.(isNearBottom);
  }
</script>

{#if activeSession}
  <div bind:this={scrollContainer} onscroll={onScroll} class="h-full overflow-y-auto px-4 py-4 space-y-4" style="scrollbar-width: thin;">
    {#each displayMessages as message, index (message.id)}
      {@const isLastMessage = index === displayMessages.length - 1}
      {@const isStreaming = activeSession.streaming && isLastMessage}
      {#if message.role === "user"}
        <UserBubble {message} />
      {:else if message.role === "system"}
        <SystemBubble {message} />
      {:else}
        <AssistantBubble {message} {isStreaming} />
      {/if}
    {/each}
  </div>
{:else}
  <div class="flex items-center justify-center h-full text-muted-foreground">
    No messages
  </div>
{/if}
