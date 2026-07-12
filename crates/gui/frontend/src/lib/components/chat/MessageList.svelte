<script lang="ts">
  import { getActiveSession, streamingMessages } from "../../state.svelte";
  import { onMount } from "svelte";
  import { ArrowDown } from "lucide-svelte";
  import TaskDock from "./TaskDock.svelte";
  import InlineStreamStatus from "./InlineStreamStatus.svelte";
  import DisplayItemList from "./DisplayItemList.svelte";
  import { DisplayItemProjection, keyDisplayItems } from "./display-items";

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
      activeSession.is_running &&
        (activeSession.phase === "streaming" ||
          activeSession.phase === "executing_tool"),
    );
  });
  const displayItems = $derived(
    keyDisplayItems([
      ...displaySections.stableItems,
      ...displaySections.dynamicItems,
    ]),
  );
  const displayMessages = $derived(displaySections.tailMessages);

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
      // ResizeObserver runs before paint. Scroll synchronously so the streaming
      // status stays anchored instead of moving for one frame before the RAF.
      if (followLatest) setScrollToBottom();
      else updateBottomState();
    });
    resizeObserver.observe(messageContent);
    return () => {
      resizeObserver.disconnect();
      if (scrollFrame !== null) cancelAnimationFrame(scrollFrame);
    };
  });
</script>

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
          <DisplayItemList items={displayItems} session_id={activeSession.id} />
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
