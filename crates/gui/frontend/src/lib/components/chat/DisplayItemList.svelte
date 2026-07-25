<script lang="ts">
  import { textFromBlocks, hasText } from "../../session";
  import type { DisplayItem } from "./display-items";
  import { keyDisplayItems, liveActivityIndex } from "./display-items";
  import UserBubble from "./UserBubble.svelte";
  import AssistantBubble from "./AssistantBubble.svelte";
  import SteerBlock from "./SteerBlock.svelte";
  import ErrorBubble from "./ErrorBubble.svelte";
  import ActivityGroup from "./ActivityGroup.svelte";
  import MessageActions from "./MessageActions.svelte";
  import TextBlock from "./TextBlock.svelte";
  import { formatMessageTime } from "../../utils";

  import type { ActivityGroupOverride } from "./activity-expansion";

  let {
    items,
    session_id,
    markLatest = true,
    activityActive = false,
    expansionOverrides,
  }: {
    items: DisplayItem[];
    session_id: string;
    markLatest?: boolean;
    activityActive?: boolean;
    expansionOverrides: Record<string, ActivityGroupOverride>;
  } = $props();

  const keyedItems = $derived(keyDisplayItems(items));
  const lastActivityIndex = $derived(
    markLatest
      ? keyedItems.findLastIndex((item) => item.type === "action_group")
      : -1,
  );
  const liveIndex = $derived(
    activityActive ? liveActivityIndex(keyedItems) : -1,
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
      class="group group/ma relative"
      class:my-2={msg.type === "user"}
      data-message-id={msg.id}
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
        <AssistantBubble
          message={msg}
          isStreaming={item.isStreaming}
          {session_id}
        />
      {/if}
      {#if msg.type === "user"}
        {@render messageTimestamp(msg.created_at, item.isStreaming)}
      {/if}
    </div>
  {:else}
    <div class="group relative -mb-2 space-y-1">
      <ActivityGroup
        messages={item.messages}
        isActiveActivity={itemIndex === liveIndex}
        isLatestActivity={itemIndex === lastActivityIndex}
        expansionOverride={expansionOverrides[item.key] ?? null}
        onExpansionOverride={(override) =>
          (expansionOverrides[item.key] = override)}
      />
      {#each item.messages as m, messageIndex (`${m.type}-${m.id}-${messageIndex}`)}
        {#if m.type === "assistant" && hasText(m.content)}
          {@const text = textFromBlocks(m.content)}
          <div class="group/ma relative" data-message-id={m.id}>
            <MessageActions
              {session_id}
              message={m}
              content={text}
              isStreaming={item.isStreaming}
            />
            <TextBlock content={text} isStreaming={item.isStreaming} />
          </div>
        {/if}
      {/each}
    </div>
  {/if}
{/each}
