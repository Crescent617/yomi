<script lang="ts">
  import {
    getActiveSession,
    getDisplayMessages,
    textFromBlocks,
    hasText,
    findThinking,
  } from "../../state.svelte";
  import { ArrowDown } from "lucide-svelte";
  import UserBubble from "./UserBubble.svelte";
  import AssistantBubble from "./AssistantBubble.svelte";
  import ErrorBubble from "./ErrorBubble.svelte";
  import ActionGroup from "./ActionGroup.svelte";
  import TextBlock from "./TextBlock.svelte";
  import GoalBar from "./GoalBar.svelte";
  import type { Message } from "../../state.svelte";
  import { formatMessageTime } from "../../utils";

  const activeSession = $derived(getActiveSession());
  const displayMessages = $derived(getDisplayMessages(activeSession?.id ?? ""));

  let scrollContainer = $state<HTMLDivElement | null>(null);
  let isNearBottom = $state(true);

  function checkNearBottom() {
    if (!scrollContainer) return true;
    const threshold = 80; // px from bottom — relaxed to avoid flicker during streaming
    const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
    return scrollHeight - scrollTop - clientHeight <= threshold;
  }

  export function scrollToBottom() {
    if (!scrollContainer) return;
    scrollContainer.scrollTop = scrollContainer.scrollHeight;
    isNearBottom = true;
  }

  // Track last message fingerprint for detecting changes during streaming
  let lastFp = "";

  function getFingerprint(msgs: Message[]): string {
    if (!msgs.length) return "";
    const last = msgs[msgs.length - 1];
    const parts: string[] = [last.id];
    if (last.type !== "tool" && last.type !== "error") {
      parts.push(`${textFromBlocks(last.content).length}`);
    }
    if (last.type === "assistant") {
      const thinking = findThinking(last.content);
      if (thinking) {
        parts.push(`${thinking.content.length}`);
      }
    }
    if (last.type === "assistant" && last.tool_calls) {
      for (const t of last.tool_calls) {
        parts.push(t.id, `${t.arguments?.length}`);
      }
    }
    if (last.type === "tool") {
      parts.push(`${textFromBlocks(last.result).length}`, last.status);
    }
    return parts.join("|");
  }

  // Auto-scroll to bottom only when user is already near bottom
  $effect(() => {
    const msgs = displayMessages;
    const fp = getFingerprint(msgs);
    if (fp === lastFp) return;
    lastFp = fp;

    if (scrollContainer && isNearBottom) {
      requestAnimationFrame(() => {
        scrollContainer!.scrollTop = scrollContainer!.scrollHeight;
      });
    }
  });

  // Scroll to bottom on session switch
  $effect(() => {
    const id = activeSession?.id;
    if (id && scrollContainer) {
      lastFp = ""; // reset fingerprint so next content triggers scroll
      requestAnimationFrame(() => {
        scrollContainer!.scrollTop = scrollContainer!.scrollHeight;
        isNearBottom = true;
      });
    }
  });

  function onScroll() {
    isNearBottom = checkNearBottom();
  }

  // ── action group logic ──
  type DisplayItem =
    | { type: "message"; message: Message; isStreaming: boolean }
    | { type: "action_group"; messages: Message[]; isStreaming: boolean };

  function buildDisplayItems(
    messages: Message[],
    streaming: boolean,
  ): DisplayItem[] {
    const items: DisplayItem[] = [];
    let group: Message[] = [];

    const flush = () => {
      if (group.length > 0) {
        const isGroupStreaming =
          streaming &&
          group[group.length - 1] === messages[messages.length - 1];
        items.push({
          type: "action_group",
          messages: [...group],
          isStreaming: isGroupStreaming,
        });
        group = [];
      }
    };

    for (let i = 0; i < messages.length; i++) {
      const msg = messages[i];
      const isLast = i === messages.length - 1;

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
    return items;
  }

  const displayItems = $derived(
    activeSession
      ? buildDisplayItems(displayMessages, activeSession.phase === "streaming")
      : [],
  );

  const lastActionGroupIndex = $derived(
    displayItems.reduce(
      (lastIdx, item, idx) => (item.type === "action_group" ? idx : lastIdx),
      -1,
    ),
  );
</script>

{#if activeSession}
  <div class="h-full relative">
    <div
      bind:this={scrollContainer}
      onscroll={onScroll}
      class="h-full overflow-y-auto"
    >
      <div class="container mx-auto px-4 lg:px-6 pt-2 pb-4">
        <div class="space-y-4">
          {#each displayItems as item, index (item.type === "message" ? item.message.id : `group-${item.messages[0]?.id ?? index}`)}
            {#if item.type === "message"}
              {@const msg = item.message}
              <div class="group relative">
                {#if msg.type === "user"}
                  <UserBubble message={msg} session_id={activeSession.id} />
                {:else if msg.type === "error"}
                  <ErrorBubble message={msg} />
                {:else if msg.type === "assistant"}
                  <AssistantBubble
                    message={msg}
                    isStreaming={item.isStreaming}
                  />
                {/if}
                {#if msg.created_at && !item.isStreaming}
                  <div
                    class="absolute right-2 -bottom-5 text-[11px] text-muted-foreground/50 opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none z-20"
                  >
                    {formatMessageTime(msg.created_at)}
                  </div>
                {/if}
              </div>
            {:else}
              <div class="group relative space-y-1">
                <ActionGroup
                  messages={item.messages}
                  isStreaming={item.isStreaming}
                  isLatest={index === lastActionGroupIndex}
                />
                {#each item.messages as m (m.id)}
                  {#if m.type === "assistant" && hasText(m.content)}
                    <div class="w-full space-y-1 mt-3">
                      <TextBlock
                        content={textFromBlocks(m.content)}
                        isStreaming={item.isStreaming}
                      />
                    </div>
                  {/if}
                {/each}
                {#if item.messages[item.messages.length - 1]?.created_at && !item.isStreaming}
                  <div
                    class="absolute left-2 -bottom-4 text-[10px] text-muted-foreground/50 opacity-0 group-hover:opacity-100 transition-opacity pointer-events-none z-20"
                  >
                    {formatMessageTime(
                      item.messages[item.messages.length - 1].created_at,
                    )}
                  </div>
                {/if}
              </div>
            {/if}
          {/each}
        </div>
      </div>
    </div>
    <GoalBar />
    {#if !isNearBottom}
      <button
        type="button"
        onclick={scrollToBottom}
        class="absolute bottom-4 left-1/2 -translate-x-1/2 z-10 flex items-center gap-1 px-3 py-1.5 rounded-full bg-primary text-primary-foreground text-xs shadow-lg hover:bg-primary/90 transition-colors"
      >
        <ArrowDown class="w-3 h-3" />
        Bottom
      </button>
    {/if}
  </div>
{:else}
  <div class="flex items-center justify-center h-full text-muted-foreground">
    No messages
  </div>
{/if}
