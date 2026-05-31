<script lang="ts">
  import { getActiveSession } from "../../state.svelte";
  import UserBubble from "./UserBubble.svelte";
  import AssistantBubble from "./AssistantBubble.svelte";
  import SystemBubble from "./SystemBubble.svelte";

  const activeSession = $derived(getActiveSession());

  let scrollContainer = $state<HTMLDivElement | null>(null);
  let lastMessageCount = $state(0);
  let isNearBottom = $state(true);

  function checkNearBottom() {
    if (!scrollContainer) return true;
    const threshold = 80; // px from bottom
    const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
    return scrollHeight - scrollTop - clientHeight <= threshold;
  }

  // Auto-scroll to bottom only when user is already near bottom
  $effect(() => {
    const msgCount = activeSession?.messages?.length ?? 0;
    if (scrollContainer && msgCount > lastMessageCount) {
      if (isNearBottom) {
        requestAnimationFrame(() => {
          scrollContainer!.scrollTop = scrollContainer!.scrollHeight;
        });
      }
    }
    lastMessageCount = msgCount;
  });

  function onScroll() {
    isNearBottom = checkNearBottom();
  }
</script>

{#if activeSession}
  <div bind:this={scrollContainer} onscroll={onScroll} class="h-full overflow-y-auto px-4 py-4 space-y-4">
    {#each activeSession.messages as message, index (message.id)}
      {@const isLastMessage = index === activeSession.messages.length - 1}
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
