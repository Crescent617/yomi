<script lang="ts">
  import { getActiveSession, streamingMessages } from "../../state.svelte";
  import {
    scrollToMessageRequest,
    clearScrollToMessageRequest,
  } from "../../state.svelte";
  import { onMount, untrack } from "svelte";
  import { ArrowDown } from "lucide-svelte";
  import ActivityBubbles from "./ActivityBubbles.svelte";
  import { isActiveSessionPhase } from "../../session-phase";
  import StreamStatusLine from "./StreamStatusLine.svelte";
  import DisplayItemList from "./DisplayItemList.svelte";
  import { DisplayItemProjection } from "./display-items";
  import { guiPreferences } from "../../settings.svelte";
  import QueryNavigator from "./QueryNavigator.svelte";
  import MessageListSkeleton from "./MessageListSkeleton.svelte";
  import SearchBar from "./SearchBar.svelte";
  import { userQueryMarkers } from "./query-navigator";
  import {
    clampActiveIndex,
    findMatches,
    stepActiveIndex,
    type SearchMatch,
  } from "./message-search";
  import {
    clearSearchHighlight,
    highlightOccurrence,
  } from "./search-highlight";
  import { hasOpenModal } from "../../modal-stack";
  import { tick } from "svelte";
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
  // Initial history not fetched yet: keep the chat column mounted but show
  // a skeleton instead of a blank pane.
  const messagesLoading = $derived(
    !!activeSession &&
      !activeSession.messages_loaded &&
      activeSession.messages.length === 0 &&
      (streamingMessages[activeSession.id]?.length ?? 0) === 0,
  );

  let scrollContainer = $state<HTMLDivElement | null>(null);
  let messageContent = $state<HTMLDivElement | null>(null);
  let isNearBottom = $state(true);

  // ── Bottom pinning ──────────────────────────────────────────────────
  // `pinned` is a geometric contract: while set, the ResizeObserver glues
  // the view to the bottom through ANY layout change — stream growth,
  // activity-group collapse, deferred rendering (code highlight, mermaid,
  // images, late history), sibling bars squeezing the scroller. The pin
  // has no timer and never expires on its own.
  //
  // The pin is released only by positive evidence of user intent:
  //   · input channel — wheel-up, a touch drag toward older messages,
  //     page-up keys. These fire before any movement and can never be
  //     produced by clamps or programmatic scrolls;
  //   · geometry-gated movement — the fallback for input the channel
  //     cannot see (scrollbar drags): scroll movement counts as intent
  //     ONLY while (scrollHeight, clientHeight) matches the last observed
  //     state. Browser clamps are *caused* by geometry changes, so
  //     movement measured across one is an echo, not intent.
  //
  // Scroll events alone never carry intent: they dispatch asynchronously
  // but read live geometry, so a clamp echo observed after a regrowth is
  // indistinguishable from a user scrolling up. Treating it as intent
  // kills the pin exactly when the layout is busiest (run-end collapse,
  // entry churn) — the failure this design replaces.
  //
  // Engagement: session entry (navigation — honored regardless of the
  // autoScroll preference), a sent user message, the jump button, and,
  // only with the preference on, scrolling back onto the bottom. With
  // the preference off an engagement holds through layout churn but
  // releases on genuinely new content (the live-tail signature below).
  let pinned = $state(true);
  // Geometry baseline of the intent classifier: the (scrollTop,
  // scrollHeight, clientHeight) triple at the last explained state.
  // Programmatic scrolls re-sync it up front so their echo events read as
  // no-movement; movement measured across a geometry change is a clamp
  // echo, not intent (see onScroll).
  const geomBaseline = { scrollTop: 0, scrollHeight: 0, clientHeight: 0 };
  function syncGeomBaseline() {
    if (!scrollContainer) return;
    geomBaseline.scrollTop = scrollContainer.scrollTop;
    geomBaseline.scrollHeight = scrollContainer.scrollHeight;
    geomBaseline.clientHeight = scrollContainer.clientHeight;
  }
  let releaseSignature: string | null = null;
  let releaseAfterLoad = false;
  let suppressReengageUntil = 0;
  const REENGAGE_SUPPRESS_MS = 800;

  // Last RO-observed geometry change; gates the intent classifier below.
  // A sub-frame height dip at the bottom can clamp scrollTop and recover
  // in the same layout cycle — invisible to the baseline triple — and its
  // late scroll echo would read as upward intent. The settle window keeps
  // such echoes inert; real input still releases via wheel/touch/keys.
  let lastGeometryChangeAt = 0;
  const GEOMETRY_SETTLE_MS = 150;
  function geometryRecentlyChanged() {
    return performance.now() - lastGeometryChangeAt < GEOMETRY_SETTLE_MS;
  }

  // Browser scroll measurements are expressed in CSS pixels. Keep these
  // named so layout styling can continue to use the Tailwind/rem scale.
  const NEAR_BOTTOM_THRESHOLD = 80;
  const LEAVE_BOTTOM_THRESHOLD = 120;

  function distanceFromBottom() {
    if (!scrollContainer) return 0;
    const { scrollTop, scrollHeight, clientHeight } = scrollContainer;
    return Math.max(0, scrollHeight - scrollTop - clientHeight);
  }

  // Identity of the genuinely-newest content: the live buffer's size plus
  // the tail message's identity, block count, and text volume. Layout
  // churn (highlight, mermaid, images) and history prepends never change
  // it; a new streamed chunk or message always does.
  const liveTailSignature = $derived.by(() => {
    const session = activeSession;
    if (!session) return "";
    const streaming = streamingMessages[session.id] ?? [];
    const last = streaming.at(-1) ?? session.messages.at(-1);
    if (!last) return `${streaming.length}:-`;
    let volume: string;
    switch (last.type) {
      case "tool":
        volume = `${last.arguments.length}:${blocksVolume(last.result)}`;
        break;
      case "error":
        volume = `1:${last.content.length}`;
        break;
      default:
        volume = blocksVolume(last.content);
    }
    return `${streaming.length}:${last.id}:${volume}`;
  });

  function blocksVolume(blocks: unknown): string {
    if (!Array.isArray(blocks)) return "0:0";
    let chars = 0;
    for (const block of blocks) {
      if (typeof block?.text === "string") chars += block.text.length;
      if (typeof block?.thinking === "string") chars += block.thinking.length;
    }
    return `${blocks.length}:${chars}`;
  }

  // With the follow preference off, an engaged pin holds through layout
  // churn but releases causally on genuinely new content — the only
  // automatic release. With the preference on there is no automatic
  // release at all: the pin ends only via user intent or an in-app jump.
  $effect(() => {
    // Toggling the preference on disarms a release captured while it was
    // off; toggling it off arms nothing until the next engagement.
    if (guiPreferences.chat.autoScroll) {
      releaseAfterLoad = false;
      releaseSignature = null;
      return;
    }
    const signature = liveTailSignature;
    if (releaseAfterLoad) {
      // Arm only once the history (and with it the tail) actually exists:
      // capturing earlier would release the moment the initial load
      // creates the tail, landing the entry at the top of the session.
      if (!activeSession?.messages_loaded) return;
      releaseAfterLoad = false;
      releaseSignature = signature;
      return;
    }
    if (releaseSignature === null || signature === releaseSignature) return;
    releasePin();
  });

  function engagePin() {
    pinned = true;
    // Untracked reads: engagePin runs inside effects (entry, sent-message)
    // that must not re-fire when the preference or load state changes —
    // re-firing would yank a user reading older messages to the bottom.
    const autoScroll = untrack(() => guiPreferences.chat.autoScroll);
    const historyLoaded = untrack(
      () => activeSession?.messages_loaded ?? false,
    );
    releaseSignature =
      autoScroll || !historyLoaded ? null : untrack(() => liveTailSignature);
    releaseAfterLoad = !autoScroll && !historyLoaded;
  }

  function releasePin() {
    pinned = false;
    releaseSignature = null;
    releaseAfterLoad = false;
  }

  // ── Find in chat (⌘F) ──────────────────────────────────────────────
  // Data-level match counting over the session's message text (cheap,
  // covers collapsed blocks); the active match is highlighted through
  // the CSS Custom Highlight API — a Range registry, not DOM mutation —
  // so streaming re-renders never fight wrapper elements. Navigation
  // jumps are explicit user intent, so they release the bottom pin.
  let searchOpen = $state(false);
  let searchQuery = $state("");
  let searchActiveIndex = $state(0);
  let searchFocusTick = $state(0);
  let searchRestoreFocus: HTMLElement | null = null;
  const searchMatches = $derived(
    findMatches(
      [
        ...(activeSession?.messages ?? []),
        // 流式中的最新消息在 stop 前不入 messages，但屏上可见——
        // 并入搜索，否则流式中最新回复计 0/0、run 结束计数突变。
        ...(activeSession ? (streamingMessages[activeSession.id] ?? []) : []),
      ],
      searchQuery,
    ),
  );
  const searchActiveClamped = $derived(
    clampActiveIndex(searchActiveIndex, searchMatches.length),
  );

  function openSearch() {
    if (!activeSession) return;
    if (searchOpen) {
      searchFocusTick += 1;
      return;
    }
    searchRestoreFocus =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    searchActiveIndex = 0;
    searchOpen = true;
  }

  function closeSearch() {
    searchOpen = false;
    searchQuery = "";
    searchActiveIndex = 0;
    clearSearchHighlight();
    searchRestoreFocus?.focus();
    searchRestoreFocus = null;
  }

  function scrollToMatch(match: SearchMatch | undefined) {
    if (!match || !messageContent || !scrollContainer) return;
    const el = messageContent.querySelector<HTMLElement>(
      `[data-message-id="${match.message_id}"]`,
    );
    if (!el) return;
    handleQueryJump(); // a jump is deliberate navigation: release the pin
    const containerTop = scrollContainer.getBoundingClientRect().top;
    const offset =
      el.getBoundingClientRect().top - containerTop + scrollContainer.scrollTop;
    const reduceMotion = window.matchMedia(
      "(prefers-reduced-motion: reduce)",
    ).matches;
    scrollContainer.scrollTo({
      top: Math.max(0, offset - 96),
      behavior: reduceMotion ? "auto" : "smooth",
    });
    // The occurrence ordinal is data-level; the rendered DOM may hold
    // fewer (collapsed details, markdown re-flow) — then the Range is
    // null and the scroll alone carries the navigation. Raw query, not
    // trimmed: counting (findMatches) and locating must agree.
    highlightOccurrence(el, searchQuery, match.occurrence);
  }

  // Jump on query edits (reset to the first match) and on explicit
  // next/previous steps — but NOT on match-count churn from streaming,
  // which would yank the view mid-read. The reset lives apart from the
  // jump effect so stepping never re-fires it.
  let searchLastQuery = "";
  $effect(() => {
    if (searchQuery !== searchLastQuery) {
      searchLastQuery = searchQuery;
      searchActiveIndex = 0;
    }
  });
  $effect(() => {
    if (!searchOpen) return;
    const query = searchQuery;
    const index = searchActiveIndex;
    const matches = untrack(() => searchMatches);
    if (!query.trim() || matches.length === 0) {
      clearSearchHighlight();
      return;
    }
    const match = matches[clampActiveIndex(index, matches.length)];
    void tick().then(() => scrollToMatch(match));
  });

  function stepSearch(delta: 1 | -1) {
    searchActiveIndex = stepActiveIndex(
      searchActiveClamped,
      searchMatches.length,
      delta,
    );
  }

  // Switching sessions closes the find bar with the view it searched.
  let searchSessionId: string | null = null;
  $effect(() => {
    const id = activeSession?.id ?? null;
    if (id !== searchSessionId) {
      searchSessionId = id;
      if (searchOpen) closeSearch();
    }
  });

  function scrollToBottomNow(behavior: "instant" | "smooth" = "instant") {
    if (!scrollContainer) return;
    if (behavior === "instant") {
      scrollContainer.scrollTop = scrollContainer.scrollHeight;
    } else {
      scrollContainer.scrollTo({
        top: scrollContainer.scrollHeight,
        behavior,
      });
    }
    // Sync the intent baseline: the resulting scroll events then read as
    // no-movement against unchanged geometry — inert to the classifier.
    syncGeomBaseline();
    isNearBottom = true;
  }

  // Glue = assert the bottom, then verify. WKWebView can defer or clobber
  // a programmatic scrollTop assignment issued during resize churn (its
  // RO/scroll-event ordering differs from test engines), so one assignment
  // is not guaranteed to land. Re-assert on following frames until the
  // bottom actually holds — bounded by retry count, not time heuristics;
  // any further resize re-fires the RO glue and starts a fresh chain.
  const GLUE_MAX_RETRIES = 4;
  let glueRetryCount = 0;
  let glueVerifyFrame: number | null = null;
  function scheduleGlueVerify() {
    if (glueVerifyFrame !== null) cancelAnimationFrame(glueVerifyFrame);
    glueVerifyFrame = requestAnimationFrame(() => {
      glueVerifyFrame = null;
      if (!pinned || !scrollContainer) return;
      if (distanceFromBottom() <= 2) return;
      if (glueRetryCount >= GLUE_MAX_RETRIES) return;
      glueRetryCount += 1;
      scrollToBottomNow();
      scheduleGlueVerify();
    });
  }
  function glueToBottom() {
    scrollToBottomNow();
    glueRetryCount = 0;
    scheduleGlueVerify();
  }

  // Classify scroll movement as user intent ONLY against stable geometry:
  // movement measured across a geometry change or inside the settle window
  // is a clamp echo; movement with unchanged (scrollHeight, clientHeight)
  // can only be a real scroll.
  function onScroll() {
    if (!scrollContainer) return;
    const scrollTop = scrollContainer.scrollTop;
    const geomChanged =
      scrollContainer.scrollHeight !== geomBaseline.scrollHeight ||
      scrollContainer.clientHeight !== geomBaseline.clientHeight;
    const movedUp = scrollTop < geomBaseline.scrollTop - 1;
    const movedDown = scrollTop > geomBaseline.scrollTop + 1;
    const distance = distanceFromBottom();
    isNearBottom = distance <= NEAR_BOTTOM_THRESHOLD;
    // Pinned + moved up while the gate can't vouch for the movement = a
    // clamp echo: real input releases via wheel/touch/keys before its
    // scroll events arrive. No resize may follow to re-arm the glue, so
    // re-assert the bottom from the echo itself. Note the distance says
    // nothing here: a collapse-then-regrow echo can displace thousands
    // of px, indistinguishable from a scrollbar drag by geometry alone.
    const clampEchoRepair = pinned && movedUp && distance > 2;
    if (!geomChanged) {
      // An echo moving down lands on the bottom anyway — no window needed.
      if (movedDown && distance <= NEAR_BOTTOM_THRESHOLD && canReengage()) {
        engagePin();
      } else if (!geometryRecentlyChanged()) {
        if (movedUp && distance > LEAVE_BOTTOM_THRESHOLD) releasePin();
      } else if (clampEchoRepair) {
        glueToBottom();
      }
    } else if (clampEchoRepair) {
      glueToBottom();
    }
    syncGeomBaseline();
  }

  // A return to the bottom counts as deliberate only with the follow
  // preference on and outside an in-app jump's suppression window.
  function canReengage() {
    return (
      !pinned &&
      guiPreferences.chat.autoScroll &&
      !scrollToMessageRequest.messageId &&
      Date.now() > suppressReengageUntil
    );
  }

  function isVerticallyScrollable(node: HTMLElement): boolean {
    const { overflowY } = getComputedStyle(node);
    return (
      (overflowY === "auto" || overflowY === "scroll") &&
      node.scrollHeight > node.clientHeight + 1
    );
  }

  // Would a nested vertical scroller (tool output, thinking block) consume
  // this wheel before the page? Then it is not leaving the bottom.
  function wheelConsumedByNested(event: WheelEvent, upward: boolean): boolean {
    for (const node of event.composedPath()) {
      if (node === scrollContainer) break;
      if (!(node instanceof HTMLElement)) continue;
      if (!isVerticallyScrollable(node)) continue;
      if (
        upward
          ? node.scrollTop > 1
          : node.scrollTop < node.scrollHeight - node.clientHeight - 1
      )
        return true;
    }
    return false;
  }

  // Input-channel intent: fires before any movement, and clamps or
  // programmatic scrolls can never produce it. Release eagerly here so the
  // user is never trapped while the geometry gate above skips a few frames
  // of classification during heavy layout churn.
  function onWheel(event: WheelEvent) {
    if (Math.abs(event.deltaX) >= Math.abs(event.deltaY)) return;
    if (event.deltaY < -2) {
      if (!pinned || wheelConsumedByNested(event, true)) return;
      releasePin();
      return;
    }
    // Pushing into the bottom is a deliberate return, even when there is
    // no movement left to classify (a collapse may have clamped the user
    // onto the bottom while unpinned).
    if (
      event.deltaY > 2 &&
      distanceFromBottom() <= NEAR_BOTTOM_THRESHOLD &&
      canReengage() &&
      !wheelConsumedByNested(event, false)
    )
      engagePin();
  }

  let touchStartY: number | null = null;
  let touchNestedScroller: HTMLElement | null = null;
  function onTouchStart(event: TouchEvent) {
    touchStartY = event.touches[0]?.clientY ?? null;
    touchNestedScroller = null;
    for (const node of event.composedPath()) {
      if (node === scrollContainer) break;
      if (!(node instanceof HTMLElement)) continue;
      if (isVerticallyScrollable(node)) {
        touchNestedScroller = node;
        break;
      }
    }
  }
  function onTouchMove(event: TouchEvent) {
    if (!pinned || touchStartY === null) return;
    const y = event.touches[0]?.clientY;
    if (y === undefined || y - touchStartY <= 12) return;
    // Finger travels down: the content reveals older messages above —
    // unless a nested scroller with upward room (tool output, thinking
    // block) is consuming the gesture instead of the page.
    if (touchNestedScroller && touchNestedScroller.scrollTop > 1) return;
    releasePin();
  }

  // Would the key scroll this list? Only when focus is on the page body
  // or inside the list itself — not when another scroller (popover,
  // panel) or an editable field owns the key.
  function keyTargetsList(event: KeyboardEvent): boolean {
    const target = event.target;
    if (target === document.body || target === document.documentElement)
      return true;
    if (!(target instanceof HTMLElement)) return false;
    if (target.closest("input, textarea, select, [contenteditable]"))
      return false;
    // The key scrolls the nearest scrollable ancestor of the focus.
    for (let el: HTMLElement | null = target; el; el = el.parentElement) {
      if (!isVerticallyScrollable(el)) continue;
      return el === scrollContainer;
    }
    return false;
  }

  function handleQueryJump() {
    releasePin();
    isNearBottom = false;
    // The jump's smooth glide may land near the bottom; it is not a
    // deliberate return.
    suppressReengageUntil = Date.now() + REENGAGE_SUPPRESS_MS;
  }

  export function scrollToBottom() {
    engagePin();
    // In a quiet layout this glides; while streaming, the RO glue's
    // instant asserts preempt it within a frame — the end state (bottom,
    // pinned) is identical either way.
    scrollToBottomNow("smooth");
  }

  // Session entry is explicit navigation: land on the latest message and
  // hold through deferred rendering, regardless of the autoScroll
  // preference. No timer — the hold ends on user intent, an in-app jump,
  // or (with the preference off) genuinely new content.
  $effect(() => {
    const id = activeSession?.id;
    if (!id || !scrollContainer) return;
    engagePin();
    glueToBottom();
  });

  // A sent user message is an explicit request to resume following the
  // latest output, even when the user was previously reading older
  // messages. Gate on the message id: `session.messages` also grows on
  // mid-run appends (retries, errors, the run-end flush), and those must
  // not re-pin — only a user scroll (or a new user message) does.
  let lastSeenUserMessageId: string | null = null;
  $effect(() => {
    const session = activeSession;
    const latestUserMessage = session?.messages.findLast(
      (message) => message.type === "user",
    );
    if (!session || !latestUserMessage || !scrollContainer) return;
    if (latestUserMessage.id === lastSeenUserMessageId) return;
    lastSeenUserMessageId = latestUserMessage.id;
    engagePin();
    glueToBottom();
  });

  // Honor scroll-to-message requests (e.g. jumping from Favorites). The
  // request stays pending until the target renders (session switches load
  // messages asynchronously and event replays keep mutating the list), so
  // the actual scroll is debounced until the DOM settles; otherwise
  // bottom-pinning scrolls would cancel it. Expires after a few seconds.
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
      releasePin();
      isNearBottom = false;
      // The smooth glide below may pass near the bottom; it is not a
      // deliberate return.
      suppressReengageUntil = Date.now() + REENGAGE_SUPPRESS_MS;
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
    if (!messageContent || !scrollContainer) return;
    const container = scrollContainer;
    const resizeObserver = new ResizeObserver(() => {
      // Observe BOTH the content and the scroller: distance from bottom is a
      // function of (scrollHeight, clientHeight, scrollTop). Content changes
      // (stream chunks, group collapse) move scrollHeight, but sibling UI
      // below the list (permission/ask/queued bars appearing, window
      // resizes) moves clientHeight without touching the content — only
      // watching the content would leave those gaps un-pinned.
      //
      // While pinned, glue — through growth AND shrink, preference-
      // independent. While unpinned, only refresh the jump-button
      // visibility — and leave the classifier's geometry baseline alone,
      // so the clamp echo this resize queued still reads as "geometry
      // changed" when its scroll event runs.
      lastGeometryChangeAt = performance.now(); // feed the settle window
      if (pinned) glueToBottom();
      else isNearBottom = distanceFromBottom() <= NEAR_BOTTOM_THRESHOLD;
    });
    resizeObserver.observe(messageContent);
    resizeObserver.observe(container);
    container.addEventListener("wheel", onWheel, { passive: true });
    container.addEventListener("touchstart", onTouchStart, { passive: true });
    container.addEventListener("touchmove", onTouchMove, { passive: true });
    function onWindowKeydown(event: KeyboardEvent) {
      // ⌘F / Ctrl+F: find in chat. Mirrors the palette's guards — never
      // open behind a modal, where the bar would be invisible yet still
      // capture typed text.
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "f") {
        if (!hasOpenModal() && activeSession) {
          event.preventDefault();
          openSearch();
        }
        return;
      }
      // Scroll keys toward older content are user intent too — when they
      // target this list.
      if (
        pinned &&
        (event.key === "PageUp" ||
          event.key === "Home" ||
          (event.key === " " && event.shiftKey)) &&
        keyTargetsList(event)
      )
        releasePin();
    }
    window.addEventListener("keydown", onWindowKeydown);
    return () => {
      resizeObserver.disconnect();
      container.removeEventListener("wheel", onWheel);
      container.removeEventListener("touchstart", onTouchStart);
      container.removeEventListener("touchmove", onTouchMove);
      window.removeEventListener("keydown", onWindowKeydown);
      clearSearchHighlight();
      if (glueVerifyFrame !== null) cancelAnimationFrame(glueVerifyFrame);
    };
  });
</script>

{#if activeSession}
  <div class="h-full flex flex-col">
    {#if searchOpen}
      <!-- 独立一行，不做浮层：浮层在顶居中会压 assistant 正文、在右
           上会压右对齐的用户气泡；占一行零遮挡。右侧 QueryNavigator
           滑轨在消息区垂直居中处，错开不冲突。 -->
      <div class="flex shrink-0 justify-end px-3 pt-2">
        <SearchBar
          bind:query={searchQuery}
          activeIndex={searchActiveClamped}
          total={searchMatches.length}
          focusTick={searchFocusTick}
          onNext={() => stepSearch(1)}
          onPrev={() => stepSearch(-1)}
          onClose={closeSearch}
        />
      </div>
    {/if}
    <div class="relative min-h-0 flex-1">
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
        <div
          bind:this={messageContent}
          class="mx-auto w-full max-w-4xl px-4 lg:px-6 pt-2 pb-4"
        >
          {#if messagesLoading}
            <MessageListSkeleton />
          {:else}
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
                <StreamStatusLine
                  session={activeSession}
                  messages={displayMessages}
                />
              {/if}
            </div>
          {/if}
        </div>
      </div>
      <QueryNavigator
        {scrollContainer}
        {messageContent}
        queries={queryMarkers}
        onJump={handleQueryJump}
      />
      <ActivityBubbles />
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
  </div>
{:else}
  <div class="flex items-center justify-center h-full text-muted-foreground">
    No messages
  </div>
{/if}
