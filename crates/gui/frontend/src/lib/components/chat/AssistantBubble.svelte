<script lang="ts">
  import { textFromBlocks, hasText } from "../../session";
  import type { BotMessage } from "../../state.svelte";
  import { parseAttachments } from "../../attachments";
  import { stripEndTurnMarker } from "../../end-turn-marker";
  import TextBlock from "./TextBlock.svelte";
  import MessageActions from "./MessageActions.svelte";
  import AttachmentChips from "./AttachmentChips.svelte";

  let {
    message,
    session_id,
    isStreaming = false,
  }: {
    message: BotMessage;
    session_id: string;
    isStreaming?: boolean;
  } = $props();
</script>

<div class="w-full space-y-2">
  {#if hasText(message.content)}
    {@const parsed = parseAttachments(
      stripEndTurnMarker(textFromBlocks(message.content)),
    )}
    <MessageActions
      {session_id}
      {message}
      content={parsed.cleaned}
      {isStreaming}
    />
    <TextBlock content={parsed.cleaned} {isStreaming} />
    <AttachmentChips paths={parsed.paths} {session_id} />
  {/if}
</div>
