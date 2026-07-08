<script lang="ts">
  import { textFromBlocks, hasText } from "../../state.svelte";
  import type { BotMessage } from "../../state.svelte";
  import TextBlock from "./TextBlock.svelte";

  let {
    message,
    isStreaming = false,
  }: { message: BotMessage; isStreaming?: boolean } = $props();
</script>

<div class="w-full space-y-2">
  {#if hasText(message.content)}
    <TextBlock content={textFromBlocks(message.content)} {isStreaming} />
  {/if}
</div>
