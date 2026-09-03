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
    <!-- 终态才剥标记：流式中间帧若恰以标记结尾，剥了会让"正文中间的
         惰性标记"瞬态消失再复现（闪烁）；流式尾帧的瞬时可见是与 TUI
         一致的统一边界。 -->
    {@const raw = textFromBlocks(message.content)}
    {@const parsed = parseAttachments(
      isStreaming ? raw : stripEndTurnMarker(raw),
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
