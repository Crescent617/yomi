<script lang="ts">
  import { textFromBlocks, hasText } from "../../session";
  import type { DisplayItem } from "./display-items";
  import { keyDisplayItems } from "./display-items";
  import UserBubble from "./UserBubble.svelte";
  import AssistantBubble from "./AssistantBubble.svelte";
  import SteerBlock from "./SteerBlock.svelte";
  import ErrorBubble from "./ErrorBubble.svelte";
  import ActivityGroup from "./ActivityGroup.svelte";
  import TextBlock from "./TextBlock.svelte";
  import { formatMessageTime } from "../../utils";

  import type { ActivityGroupOverride } from "./activity-expansion";

  let {
    items,
    session_id,
    markLatest = true,
    expansionOverrides,
  }: {
    items: DisplayItem[];
    session_id: string;
    markLatest?: boolean;
    expansionOverrides: Record<string, ActivityGroupOverride>;
  } = $props();

  const keyedItems = $derived(keyDisplayItems(items));
  const lastActivityIndex = $derived(
    markLatest
      ? keyedItems.findLastIndex((item) => item.type === "action_group")
      : -1,
  );
</script>

{#snippet messageTimestamp(createdAt: string | undefined, isStreaming: boolean)}
  {#if createdAt && !isStreaming}
    <div
      class="mt-1 flex justify-end pr-1 text-[10px] leading-none text-muted-foreground/55 transition-colors group-hover:text-muted-foreground"
    >
      <time datetime={createdAt}>{formatMessageTime(createdAt)}</time>
    </div>
  {/if}
{/snippet}

{#each keyedItems as item, itemIndex (item.key)}
  {#if item.type === "error_group"}
    <div class="group relative">
      <ErrorBubble messages={item.messages} />
    </div>
  {:else if item.type === "message"}
    {@const msg = item.message}
    <div
      class="group relative"
      class:my-2={msg.type === "user"}
      data-user-query-id={msg.type === "user" ? msg.id : undefined}
    >
      {#if msg.type === "user"}
        <UserBubble message={msg} {session_id} />
      {:else if msg.type === "steer"}
        <SteerBlock
          content={textFromBlocks(msg.content)}
          created_at={msg.created_at}
        />
      {:else if msg.type === "assistant"}
        <AssistantBubble message={msg} isStreaming={item.isStreaming} />
      {/if}
      {#if msg.type === "user"}
        {@render messageTimestamp(msg.created_at, item.isStreaming)}
      {/if}
    </div>
  {:else}
    <div class="group relative -mb-2 space-y-1">
      <ActivityGroup
        messages={item.messages}
        isActiveActivity={item.isActiveActivity}
        isLatestActivity={itemIndex === lastActivityIndex}
        expansionOverride={expansionOverrides[item.key] ?? null}
        onExpansionOverride={(override) =>
          (expansionOverrides[item.key] = override)}
      />
      {#each item.messages as m, messageIndex (`${m.type}-${m.id}-${messageIndex}`)}
        {#if m.type === "assistant" && hasText(m.content)}
          <TextBlock
            content={textFromBlocks(m.content)}
            isStreaming={item.isStreaming}
          />
        {/if}
      {/each}
    </div>
  {/if}
{/each}
