<script lang="ts">
  import { getActiveSession } from "../../state.svelte";
  import UserBubble from "./UserBubble.svelte";
  import AssistantBubble from "./AssistantBubble.svelte";

  const activeSession = $derived(getActiveSession());

  let scrollContainer = $state<HTMLDivElement | null>(null);
  let lastMessageCount = $state(0);

  // Auto-scroll to bottom when new messages arrive
  $effect(() => {
    const msgCount = activeSession?.messages?.length ?? 0;
    if (scrollContainer && msgCount > lastMessageCount) {
      scrollContainer.scrollTop = scrollContainer.scrollHeight;
    }
    lastMessageCount = msgCount;
  });
</script>

{#if activeSession}
  <div bind:this={scrollContainer} class="h-full overflow-y-auto px-4 py-4 space-y-4">
    {#each activeSession.messages as message (message.id)}
      {#if message.role === "user"}
        <UserBubble {message} />
      {:else}
        <AssistantBubble {message} />
      {/if}
    {/each}
  </div>
{:else}
  <div class="flex items-center justify-center h-full text-muted-foreground">
    No messages
  </div>
{/if}
