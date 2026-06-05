<script lang="ts">
  import type { ChatMessage } from "../../state.svelte";
  import TextBlock from "./TextBlock.svelte";
  import ThinkingBlock from "./ThinkingBlock.svelte";
  import ToolBlock from "./ToolBlock.svelte";

  let { message, isStreaming = false }: { message: ChatMessage; isStreaming?: boolean } = $props();
</script>

<div class="w-full space-y-2">
  <!-- Thinking block -->
  {#if message.thinking}
    <ThinkingBlock content={message.thinking.content} elapsedMs={message.thinking.elapsedMs} {isStreaming} />
  {/if}

  <!-- Text content -->
  {#if message.content}
    <TextBlock content={message.content} />
  {/if}

  <!-- Tool blocks -->
  {#if message.tools && message.tools.length > 0}
    <div class="space-y-1">
      {#each message.tools as tool (tool.id)}
        <ToolBlock {tool} />
      {/each}
    </div>
  {/if}
</div>
