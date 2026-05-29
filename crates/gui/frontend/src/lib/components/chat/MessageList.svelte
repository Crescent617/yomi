<script lang="ts">
  import { getActiveSession } from "../../state.svelte";
  import UserBubble from "./UserBubble.svelte";
  import AssistantBubble from "./AssistantBubble.svelte";

  const activeSession = $derived(getActiveSession());
</script>

{#if activeSession}
  <div class="h-full overflow-y-auto px-4 py-4 space-y-4">
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
