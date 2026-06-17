<script lang="ts">
  import type { ChatMessage } from "../../state.svelte";
  import TextBlock from "./TextBlock.svelte";

  let {
    message,
    isStreaming = false,
  }: { message: ChatMessage; isStreaming?: boolean } = $props();
</script>

<div class="w-full space-y-2">
  {#if message.content}
    <TextBlock content={message.content} {isStreaming} />
  {/if}
</div>
