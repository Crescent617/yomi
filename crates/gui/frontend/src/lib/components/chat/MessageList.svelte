<script lang="ts">
  import { getActiveSession, streamingMessages } from "../../state.svelte";
  import {
    scrollToMessageRequest,
    clearScrollToMessageRequest,
  } from "../../state.svelte";
  import { onMount } from "svelte";
  import { ArrowDown } from "lucide-svelte";
  import TaskDock from "./TaskDock.svelte";
  import { isActiveSessionPhase } from "../../session-phase";
  import InlineStreamStatus from "./InlineStreamStatus.svelte";
  import DisplayItemList from "./DisplayItemList.svelte";
  import { DisplayItemProjection } from "./display-items";
  import { guiPreferences } from "../../settings.svelte";
  import QueryNavigator from "./QueryNavigator.svelte";
  import { userQueryMarkers } from "./query-navigator";
  import type { ActivityGroupOverride } from "./activity-expansion";

  const activeSession = $derived(getActiveSession());
  const displayItemProjection = new DisplayItemProjection();
  const displaySections = $derived.by(() => {
    if (!activeSession) {
      return { stableItems: [], dynamicItems: [], tailMessages: [] };
    }
    return displayItemProjection.update(
      activeSession.id,
      activeSession.messages,
      activeSession.message_rewrite_revision,
      streamingMessages[activeSession.id] ?? [],
      activeSession.phase === "streaming",
    );
  });
  const displayMessages = $derived(displaySections.tailMessages);
  let activityExpansionOverrides = $state<
    Record<string, ActivityGroupOverride>
  >({});
  const dynamicHasActivityGroup = $derived(
    displaySections.dynamicItems.some((item) => item.type === "action_group"),
  );
  const queryMarkers = $derived(
    userQueryMarkers(activeSession?.messages ?? []),
  );

  let scrollContainer = $state<HTMLDivElement | null>(null);
  let messageContent = $state<HTMLDivElement | null>(null);
  let isNearBottom = $state(true);
  let followLatest = $state(true);
  let scrollFrame: number | null = null;
  let programmaticScrollTimer: ReturnType<typeof setTimeout> | null = null;
  let isProgrammaticScroll = false;

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
    isProgrammaticScroll = true;
    if (programmaticScrollTimer !== null) clearTimeout(programmaticScrollTimer);
    scrollContainer.scrollTo({
      top: scrollContainer.scrollHeight,
      behavior,
    });
    programmaticScrollTimer = setTimeout(
      () => {
        programmaticScrollTimer = null;
        isProgrammaticScroll = false;
        updateBottomState();
      },
      behavior === "smooth" ? 500 : 0,
    );
    isNearBottom = true;
    followLatest = true;
  }

  function handleQueryJump() {
    followLatest = false;
    isNearBottom = false;
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

  // A sent user message is an explicit request to resume following the latest
  // output, even when the user was previously reading older messages.
  $effect(() => {
    const session = activeSession;
    const latestUserMessage = session?.messages.findLast(
      (message) => message.type === "user",
    );
    if (!session || !latestUserMessage || !scrollContainer) return;
    followLatest = true;
    scheduleScrollToBottom();
  });

  function onScroll() {
    if (isProgrammaticScroll) return;
    updateBottomState();
  }

  // Honor scroll-to-message requests (e.g. jumping from Favorites). The
  // request stays pending until the target renders (session switches load
  // messages asynchronously and event replays keep mutating the list), so
  // the actual scroll is debounced until the DOM settles; otherwise
  // follow-latest scrolls would cancel it. Expires after a few seconds.
  $effect(() => {
    const id = scrollToMessageRequest.messageId;
    if (!id || !messageContent || !scrollContainer) return;
    // Re-run as rendered items change so late-arriving messages are found.
    void displaySections;
    if (Date.now() - scrollToMessageRequest.at > 8000) {
      clearScrollToMessageRequest();
      return;
    }
    const el = messageContent.querySelector(`[data-message-id="${id}"]`);
    if (!el) return;
    const container = scrollContainer;
    const timer = setTimeout(() => {
      if (scrollToMessageRequest.messageId !== id) return;
      clearScrollToMessageRequest();
      followLatest = false;
      isNearBottom = false;
      const containerTop = container.getBoundingClientRect().top;
      const offset =
        el.getBoundingClientRect().top - containerTop + container.scrollTop;
      const reduceMotion = window.matchMedia(
        "(prefers-reduced-motion: reduce)",
      ).matches;
      container.scrollTo({
        top: Math.max(0, offset - container.clientHeight / 3),
        behavior: reduceMotion ? "auto" : "smooth",
      });
      el.classList.add("message-flash");
      setTimeout(() => el.classList.remove("message-flash"), 1800);
    }, 400);
    return () => clearTimeout(timer);
  });

  onMount(() => {
    if (!messageContent) return;
    const resizeObserver = new ResizeObserver(() => {
      // ResizeObserver runs before paint. Scroll synchronously so the streaming
      // status stays anchored instead of moving for one frame before the RAF.
      if (followLatest && guiPreferences.chat.autoScroll) setScrollToBottom();
      else updateBottomState();
    });
    resizeObserver.observe(messageContent);
    return () => {
      resizeObserver.disconnect();
      if (scrollFrame !== null) cancelAnimationFrame(scrollFrame);
      if (programmaticScrollTimer !== null)
        clearTimeout(programmaticScrollTimer);
    };
  });
</script>

{#if activeSession}
  <div class="h-full relative">
    <!-- Classic scrollbars (macOS w/ mouse, Windows) shrink the scroller's
         content box, so the centered message column sits half a scrollbar
         width off from the input column below. Symmetric gutters re-center
         it — only once the max-w-4xl (56rem) column actually binds; below
         that both columns are full-width and already aligned. -->
    <div
      bind:this={scrollContainer}
      onscroll={onScroll}
      class="h-full overflow-y-auto [overflow-anchor:none] @min-[56rem]:[scrollbar-gutter:stable_both-edges]"
    >
      <TaskDock />
      <div
        bind:this={messageContent}
        class="mx-auto w-full max-w-4xl px-4 lg:px-6 pt-2 pb-4"
      >
        <div class="flex flex-col gap-3">
          <DisplayItemList
            items={displaySections.stableItems}
            session_id={activeSession.id}
            markLatest={!dynamicHasActivityGroup}
            expansionOverrides={activityExpansionOverrides}
          />
          <DisplayItemList
            items={displaySections.dynamicItems}
            session_id={activeSession.id}
            activityActive={isActiveSessionPhase(activeSession.phase)}
            expansionOverrides={activityExpansionOverrides}
          />
          {#if isActiveSessionPhase(activeSession.phase)}
            <InlineStreamStatus
              session={activeSession}
              messages={displayMessages}
            />
          {/if}
        </div>
      </div>
    </div>
    <QueryNavigator
      {scrollContainer}
      {messageContent}
      queries={queryMarkers}
      onJump={handleQueryJump}
    />
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
