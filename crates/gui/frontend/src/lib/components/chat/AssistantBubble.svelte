<script lang="ts">
  import { textFromBlocks, hasText } from "../../session";
  import type { BotMessage } from "../../state.svelte";
  import TextBlock from "./TextBlock.svelte";
  import MessageActions from "./MessageActions.svelte";

  let {
    message,
    session_id,
    isStreaming = false,
  }: { message: BotMessage; session_id: string; isStreaming?: boolean } =
    $props();
</script>

<div class="w-full space-y-2">
  {#if hasText(message.content)}
    {@const content = textFromBlocks(message.content)}
    <MessageActions {session_id} {message} {content} {isStreaming} />
    <TextBlock {content} {isStreaming} />
  {/if}
</div>
