<script lang="ts">
  import {
    getActiveSession,
    getDisplayMessages,
    textFromBlocks,
    hasText,
    findThinking,
  } from "../../state.svelte";
  import { onMount } from "svelte";
  import { ArrowDown } from "lucide-svelte";
  import UserBubble from "./UserBubble.svelte";
  import AssistantBubble from "./AssistantBubble.svelte";
  import ErrorBubble from "./ErrorBubble.svelte";
  import ActivityGroup from "./ActivityGroup.svelte";
  import TextBlock from "./TextBlock.svelte";
  import TaskDock from "./TaskDock.svelte";
  import InlineStreamStatus from "./InlineStreamStatus.svelte";
  import type { ErrorMessage, Message } from "../../state.svelte";
  import { formatMessageTime } from "../../utils";
  import { isActivityTail } from "./activity-group";

  const activeSession = $derived(getActiveSession());
  const displayMessages = $derived(getDisplayMessages(activeSession?.id ?? ""));

  let scrollContainer = $state<HTMLDivElement | null>(null);
  let messageContent = $state<HTMLDivElement | null>(null);
  let isNearBottom = $state(true);
  let followLatest = $state(true);
  let scrollFrame: number | null = null;

  // Browser scroll measurements are expressed in CSS pixels. Keep these
  // named so layout styling can continue to use the Tailwind/rem scale.
  const NEAR_BOTTOM_THRESHOLD = 80;
  const LEAVE_BOTTOM_THRESHOLD = 120;

  function distanceFromBottom() {
    if (!scrollContainer) return 0;
    const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
    return Math.max(0, scrollHeight - scrollTop - clientHeight);
  }

  function updateBottomState() {
    const distance = distanceFromBottom();
    isNearBottom = distance <= NEAR_BOTTOM_THRESHOLD;
    if (distance <= NEAR_BOTTOM_THRESHOLD) followLatest = true;
    else if (distance > LEAVE_BOTTOM_THRESHOLD) followLatest = false;
  }

  function setScrollToBottom(behavior: "auto" | "smooth" = "auto") {
    if (!scrollContainer) return;
    scrollContainer.scrollTo({
      top: scrollContainer.scrollHeight,
      behavior,
    });
    isNearBottom = true;
    followLatest = true;
  }

  export function scrollToBottom() {
    setScrollToBottom("smooth");
  }

  function scheduleScrollToBottom() {
    if (scrollFrame !== null) return;
    scrollFrame = requestAnimationFrame(() => {
      scrollFrame = null;
      setScrollToBottom();
    });
  }

  // Scroll to bottom on session switch
  $effect(() => {
    const id = activeSession?.id;
    if (id && scrollContainer) {
      followLatest = true;
      scheduleScrollToBottom();
    }
  });

  function onScroll() {
    updateBottomState();
  }

  onMount(() => {
    if (!messageContent) return;
    const resizeObserver = new ResizeObserver(() => {
      if (followLatest) scheduleScrollToBottom();
      else updateBottomState();
    });
    resizeObserver.observe(messageContent);
    return () => {
      resizeObserver.disconnect();
      if (scrollFrame !== null) cancelAnimationFrame(scrollFrame);
    };
  });

  // ── action group logic ──
  type DisplayItem =
    | { type: "message"; message: Message; isStreaming: boolean }
    | { type: "error_group"; messages: ErrorMessage[] }
    | {
        type: "action_group";
        messages: Message[];
        isStreaming: boolean;
        isActiveActivity: boolean;
      };

  function buildDisplayItems(
    messages: Message[],
    streaming: boolean,
    activityActive: boolean,
  ): DisplayItem[] {
    const items: DisplayItem[] = [];
    let group: Message[] = [];
    let errors: ErrorMessage[] = [];

    const flushErrors = () => {
      if (errors.length > 0) {
        items.push({ type: "error_group", messages: [...errors] });
        errors = [];
      }
    };

    const flush = () => {
      if (group.length > 0) {
        const isTailGroup =
          group[group.length - 1] === messages[messages.length - 1];
        const isGroupStreaming = streaming && isTailGroup;
        const isActiveActivity =
          activityActive && isTailGroup && isActivityTail(messages.at(-1));
        items.push({
          type: "action_group",
          messages: [...group],
          isStreaming: isGroupStreaming,
          isActiveActivity,
        });
        group = [];
      }
    };

    for (let i = 0; i < messages.length; i++) {
      const msg = messages[i];
      const isLast = i === messages.length - 1;

      if (msg.type === "error") {
        flush();
        errors.push(msg);
        continue;
      }

      flushErrors();

      if (msg.type !== "assistant" && msg.type !== "tool") {
        flush();
        items.push({ type: "message", message: msg, isStreaming: false });
        continue;
      }

      if (msg.type === "tool") {
        group.push(msg);
        continue;
      }

      // BotMessage
      const hasTextContent = hasText(msg.content);
      const hasThinking = findThinking(msg.content) !== null;
      const hasToolCalls = msg.tool_calls && msg.tool_calls.length > 0;

      if (hasThinking || hasToolCalls) {
        group.push(msg);
        if (hasTextContent) flush();
      } else {
        flush();
        items.push({
          type: "message",
          message: msg,
          isStreaming: streaming && isLast,
        });
      }
    }

    flush();
    flushErrors();
    return items;
  }

  const displayItems = $derived(
    activeSession
      ? buildDisplayItems(
          displayMessages,
          activeSession.phase === "streaming",
          activeSession.is_running &&
            (activeSession.phase === "streaming" ||
              activeSession.phase === "executing_tool"),
        )
      : [],
  );
</script>

{#snippet messageTimestamp(
  createdAt: string | undefined,
  isStreaming: boolean,
  isUser = false,
)}
  {#if createdAt && !isStreaming}
    <div
      class="flex text-[10px] leading-none text-muted-foreground/55 transition-colors group-hover:text-muted-foreground {isUser
        ? 'mt-1 justify-end pr-1'
        : 'justify-start pl-1'}"
    >
      <time datetime={createdAt}>{formatMessageTime(createdAt)}</time>
    </div>
  {/if}
{/snippet}

{#if activeSession}
  <div class="h-full relative">
    <div
      bind:this={scrollContainer}
      onscroll={onScroll}
      class="h-full overflow-y-auto"
    >
      <TaskDock />
      <div
        bind:this={messageContent}
        class="container mx-auto px-4 lg:px-6 pt-2 pb-4"
      >
        <div class="flex flex-col gap-3">
          {#each displayItems as item, index (item.type === "message" ? item.message.id : `${item.type}-${item.messages[0]?.id ?? index}`)}
            {#if item.type === "error_group"}
              <div class="group relative">
                <ErrorBubble messages={item.messages} />
              </div>
            {:else if item.type === "message"}
              {@const msg = item.message}
              <div class="group relative" class:my-2={msg.type === "user"}>
                {#if msg.type === "user"}
                  <UserBubble message={msg} session_id={activeSession.id} />
                {:else if msg.type === "assistant"}
                  <AssistantBubble
                    message={msg}
                    isStreaming={item.isStreaming}
                  />
                {/if}
                {@render messageTimestamp(
                  msg.created_at,
                  item.isStreaming,
                  msg.type === "user",
                )}
              </div>
            {:else}
              <div class="group relative -mb-2 space-y-1">
                <ActivityGroup
                  messages={item.messages}
                  isActiveActivity={item.isActiveActivity}
                />
                {#each item.messages as m (m.id)}
                  {#if m.type === "assistant" && hasText(m.content)}
                    <div class="w-full space-y-1">
                      <TextBlock
                        content={textFromBlocks(m.content)}
                        isStreaming={item.isStreaming}
                      />
                      {@render messageTimestamp(
                        m.created_at,
                        item.isStreaming,
                      )}
                    </div>
                  {/if}
                {/each}
              </div>
            {/if}
          {/each}
          {#if activeSession.is_running}
            <InlineStreamStatus
              session={activeSession}
              messages={displayMessages}
            />
          {/if}
        </div>
      </div>
    </div>
    {#if !isNearBottom}
      <button
        type="button"
        onclick={scrollToBottom}
        class="absolute bottom-3 left-1/2 z-10 inline-flex h-8 w-8 -translate-x-1/2 items-center justify-center rounded-md border border-border bg-card text-muted-foreground shadow-md transition-colors hover:bg-secondary hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background"
        aria-label="Jump to latest message"
        title="Jump to latest message"
      >
        <ArrowDown size={15} strokeWidth={2.25} />
      </button>
    {/if}
  </div>
{:else}
  <div class="flex items-center justify-center h-full text-muted-foreground">
    No messages
  </div>
{/if}
