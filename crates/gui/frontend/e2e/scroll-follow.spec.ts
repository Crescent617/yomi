import { expect, test } from "@playwright/test";

/**
 * Scroll-following regression tests around the activity group collapsing at
 * run end. The collapse shrinks the content by thousands of pixels; the
 * browser then clamps scrollTop down to the new maximum. That clamp is a
 * content-height change, not a user scroll, so it must never re-engage
 * following for a user who scrolled up to read.
 */

test("activity group collapse at run end does not re-engage following", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const phaseLib = await import("/src/lib/session-phase.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "scroll-collapse-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const userMessage = {
      id: "u1",
      type: "user" as const,
      content: [{ type: "text", text: "run" }],
      created_at: now,
    };
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });
    const toolMessage = (id: string, lines: number) => ({
      id,
      type: "tool" as const,
      tool_call_id: `call-${id}`,
      tool_name: "shell",
      status: "completed",
      arguments: "{}",
      result: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    const streamed = [
      userMessage,
      assistantText("a1", 100),
      ...Array.from({ length: 8 }, (_, i) => toolMessage(`t${i}`, 60)),
    ];
    state.streamingMessages[session.id] = streamed;
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distanceFromBottom = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;

    // Sanity: following the stream at the bottom.
    const initiallyFollowing = distanceFromBottom() <= 80;

    // User scrolls up to re-read the answer while tools stream below. The
    // position is inside the region the collapse will clamp away.
    const maxScroll = scroller.scrollHeight - scroller.clientHeight;
    scroller.scrollTop = maxScroll - 800;
    scroller.dispatchEvent(new Event("scroll"));
    await tick();
    await new Promise((resolve) => setTimeout(resolve, 60));
    const scrollTopBeforeCollapse = scroller.scrollTop;

    // Run ends: streaming buffer is committed, phase flips to idle, and the
    // live activity group collapses.
    sessionLib.appendSessionMessages(session, streamed);
    state.streamingMessages[session.id] = [];
    phaseLib.setSessionPhase(session, "idle");
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const clampedByCollapse = scroller.scrollTop < scrollTopBeforeCollapse;

    // New output arrives afterwards. If the collapse wrongly re-engaged
    // following, the view jumps to the bottom; otherwise it stays put.
    state.streamingMessages[session.id] = [assistantText("a2", 100)];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    return {
      initiallyFollowing,
      scrollTopBeforeCollapse,
      clampedByCollapse,
      distanceAfterNewOutput: distanceFromBottom(),
    };
  });

  expect(result.initiallyFollowing).toBe(true);
  expect(result.scrollTopBeforeCollapse).toBeGreaterThan(0);
  // The scenario only exercises the bug when the collapse actually clamped
  // the user's scroll position.
  expect(result.clampedByCollapse).toBe(true);
  expect(result.distanceAfterNewOutput).toBeGreaterThan(120);
});

test("session entry lands at the bottom while history loads, autoScroll off", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { guiPreferences } = await import("/src/lib/settings.svelte.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    // Entry navigation scrolls independently of the follow preference.
    guiPreferences.chat.autoScroll = false;

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    // Mount without an active session, then activate one that already shows
    // partial (live) messages while its full history is still loading —
    // mirrors entering a running session.
    mount(MessageList, { target });

    const session = sessionLib.createSessionState({
      id: "entry-scroll-test",
      phase: "idle",
      messages_loaded: false,
    });
    state.sessionState.sessions.push(session);
    // Mutate through the $state proxy — the raw object returned by
    // createSessionState is not reactive.
    const live = state.sessionState.sessions.find((s) => s.id === session.id);
    if (!live) throw new Error("session not registered");

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const userMessage = (id: string) => ({
      id,
      type: "user" as const,
      content: [{ type: "text", text: "hi" }],
      created_at: now,
    });
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });
    live.messages.push(userMessage("u1"), assistantText("a1", 150));
    state.sessionState.activeSessionId = session.id;
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const atBottomAfterPartial = distance() <= 2;

    // Full history arrives: older messages land ABOVE the live tail, which
    // keeps the same latest user message — so neither the session-switch
    // scroll nor the new-user-message scroll fires again.
    live.messages.unshift(userMessage("u0"), assistantText("a0", 150));
    live.messages_loaded = true;
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    return {
      atBottomAfterPartial,
      distance: distance(),
      scrollHeight: scroller.scrollHeight,
      clientHeight: scroller.clientHeight,
    };
  });

  expect(result.scrollHeight).toBeGreaterThan(result.clientHeight);
  expect(result.atBottomAfterPartial).toBe(true);
  expect(result.distance).toBeLessThanOrEqual(2);
});

test("scrolling back down to the bottom re-engages following", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "scroll-reengage-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    state.streamingMessages[session.id] = [
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text", text: "run" }],
        created_at: now,
      },
      assistantText("a1", 200),
    ];
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distanceFromBottom = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;

    // Scroll up to stop following, then back down to the bottom.
    scroller.scrollTop = 0;
    scroller.dispatchEvent(new Event("scroll"));
    await tick();
    scroller.scrollTop = scroller.scrollHeight - scroller.clientHeight;
    scroller.dispatchEvent(new Event("scroll"));
    await tick();
    await new Promise((resolve) => setTimeout(resolve, 60));

    // New streamed output must be followed again.
    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      assistantText("a2", 100),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    return { distanceAfterNewOutput: distanceFromBottom() };
  });

  expect(result.distanceAfterNewOutput).toBeLessThanOrEqual(80);
});
test("streaming growth stays pinned, including large single chunks", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "stream-pin-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    state.streamingMessages[session.id] = [
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text", text: "run" }],
        created_at: now,
      },
      assistantText("a1", 100),
    ];
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;

    const checkpoints: number[] = [];
    // Several small chunks, then one huge chunk (code-block-sized) at once.
    for (const [id, lines] of [
      ["a2", 40],
      ["a3", 40],
      ["a4", 400],
      ["a5", 30],
    ] as const) {
      state.streamingMessages[session.id] = [
        ...state.streamingMessages[session.id]!,
        assistantText(id, lines),
      ];
      await tick();
      await new Promise(requestAnimationFrame);
      await new Promise((resolve) => setTimeout(resolve, 30));
      checkpoints.push(distance());
    }
    return { checkpoints };
  });

  for (const distance of result.checkpoints) {
    expect(distance).toBeLessThanOrEqual(2);
  }
});

test("scrolling up during streaming frees the view; a new user message re-pins it", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "stream-free-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    const live = state.sessionState.sessions.find((s) => s.id === session.id);
    if (!live) throw new Error("session not registered");

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    state.streamingMessages[session.id] = [
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text", text: "run" }],
        created_at: now,
      },
      assistantText("a1", 200),
    ];
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;

    // User scrolls up to read while the run continues.
    scroller.scrollTop -= 500;
    scroller.dispatchEvent(new Event("scroll"));
    await tick();

    // More output arrives: the view must stay where the user left it.
    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      assistantText("a2", 100),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const distanceWhileFree = distance();
    const jumpButtonVisible =
      target.querySelector('button[aria-label="Jump to latest message"]') !==
      null;

    // Sending a new message is an explicit request to follow the reply.
    live.messages.push({
      id: "u2",
      type: "user" as const,
      content: [{ type: "text", text: "again" }],
      created_at: now,
    });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const distanceAfterSend = distance();

    return { distanceWhileFree, jumpButtonVisible, distanceAfterSend };
  });

  expect(result.distanceWhileFree).toBeGreaterThan(120);
  expect(result.jumpButtonVisible).toBe(true);
  expect(result.distanceAfterSend).toBeLessThanOrEqual(2);
});

test("group collapse while FOLLOWING keeps the bottom pinned", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const phaseLib = await import("/src/lib/session-phase.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "collapse-pin-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const toolMessage = (id: string, lines: number) => ({
      id,
      type: "tool" as const,
      tool_call_id: `call-${id}`,
      tool_name: "shell",
      status: "completed",
      arguments: "{}",
      result: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    // Tools streaming with the live group expanded, pinned to the bottom.
    state.streamingMessages[session.id] = [
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text", text: "run" }],
        created_at: now,
      },
      ...Array.from({ length: 6 }, (_, i) => toolMessage(`t${i}`, 60)),
    ];
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const pinnedBefore = distance() <= 2;

    // The final answer starts streaming: the tail stops being an activity
    // group, so the expanded group collapses mid-stream.
    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      assistantText("final", 20),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const afterMidStreamCollapse = distance();

    // The run ends: phase flips idle, the buffer is committed and sealed.
    sessionLib.appendSessionMessages(
      session,
      state.streamingMessages[session.id]!,
    );
    state.streamingMessages[session.id] = [];
    phaseLib.setSessionPhase(session, "idle");
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const afterRunEnd = distance();

    return { pinnedBefore, afterMidStreamCollapse, afterRunEnd };
  });

  expect(result.pinnedBefore).toBe(true);
  expect(result.afterMidStreamCollapse).toBeLessThanOrEqual(2);
  expect(result.afterRunEnd).toBeLessThanOrEqual(2);
});

test("clientHeight shrink below the list re-pins without content growth", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    // Idle session with committed long history — no streaming, nothing will
    // grow the content after entry.
    const session = sessionLib.createSessionState({
      id: "clientheight-test",
      phase: "idle",
      messages_loaded: true,
    });
    state.sessionState.sessions.push(session);
    const live = state.sessionState.sessions.find((s) => s.id === session.id);
    if (!live) throw new Error("session not registered");

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    live.messages.push(
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text", text: "hi" }],
        created_at: now,
      },
      {
        id: "a1",
        type: "assistant" as const,
        content: [{ type: "text", text: filler(300) }],
        created_at: now,
      },
    );
    state.sessionState.activeSessionId = session.id;
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const atBottomOnEntry = distance() <= 2;

    // A bar (permission / ask-user / queued input) appears below the list
    // and squeezes the scroller — no content change whatsoever.
    target.style.height = "450px";
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    return { atBottomOnEntry, distanceAfterShrink: distance() };
  });

  expect(result.atBottomOnEntry).toBe(true);
  expect(result.distanceAfterShrink).toBeLessThanOrEqual(2);
});

test("entry pin holds through deferred layout churn; new content releases it (autoScroll off)", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { guiPreferences } = await import("/src/lib/settings.svelte.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    // Entry navigation pins independently of the follow preference.
    guiPreferences.chat.autoScroll = false;

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "deferred-churn-test",
      phase: "idle",
      messages_loaded: true,
    });
    state.sessionState.sessions.push(session);
    const live = state.sessionState.sessions.find((s) => s.id === session.id);
    if (!live) throw new Error("session not registered");

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    live.messages.push(
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text", text: "hi" }],
        created_at: now,
      },
      {
        id: "a1",
        type: "assistant" as const,
        content: [{ type: "text", text: filler(200) }],
        created_at: now,
      },
    );
    state.sessionState.activeSessionId = session.id;
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const atEntry = distance() <= 2;

    // Deferred rendering (code highlight, mermaid, images) grows the LAYOUT
    // without touching message data. Waves are spaced far beyond any quiet
    // window — the pin must not be time-bounded.
    const messageEl = target.querySelector<HTMLElement>(
      '[data-message-id="a1"]',
    );
    if (!messageEl) throw new Error("message element not found");
    messageEl.style.minHeight = `${messageEl.offsetHeight + 900}px`;
    await new Promise((resolve) => setTimeout(resolve, 500));
    const afterWave1 = distance();
    messageEl.style.minHeight = `${messageEl.offsetHeight + 900}px`;
    await new Promise((resolve) => setTimeout(resolve, 500));
    const afterWave2 = distance();

    // Genuinely new content: with the preference off, the pin releases
    // causally and the growth is no longer glued.
    live.messages.push({
      id: "a2",
      type: "assistant" as const,
      content: [{ type: "text", text: filler(80) }],
      created_at: now,
    });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const afterNewContent = distance();

    return { atEntry, afterWave1, afterWave2, afterNewContent };
  });

  expect(result.atEntry).toBe(true);
  expect(result.afterWave1).toBeLessThanOrEqual(2);
  expect(result.afterWave2).toBeLessThanOrEqual(2);
  expect(result.afterNewContent).toBeGreaterThan(2);
});

test("a clamp echo observed after regrowth cannot kill the pin", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "clamp-echo-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const userMessage = {
      id: "u1",
      type: "user" as const,
      content: [{ type: "text", text: "run" }],
      created_at: now,
    };
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    state.streamingMessages[session.id] = [userMessage, assistantText("a1", 300)];
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const pinnedBefore = distance() <= 2;

    // Reproduce the engine-ordering race deterministically: a collapse
    // clamps scrollTop and queues a scroll event, but the event is only
    // observed AFTER the stream regrows the content — and before the
    // ResizeObserver re-glues. Everything stays in this task, so no
    // rendering update (and no RO callback) can interleave.
    state.streamingMessages[session.id] = [userMessage, assistantText("a1", 40)];
    await tick();
    void scroller.scrollHeight; // force layout: the shrink clamps scrollTop
    state.streamingMessages[session.id] = [
      userMessage,
      assistantText("a1", 40),
      assistantText("a2", 200),
    ];
    await tick();
    void scroller.scrollHeight; // force layout: regrown; scrollTop stays low
    scroller.dispatchEvent(new Event("scroll")); // the stale clamp echo

    // Back to the event loop: the RO glue runs iff the pin survived.
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const afterEcho = distance();

    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      assistantText("a3", 100),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const afterMoreOutput = distance();

    return { pinnedBefore, afterEcho, afterMoreOutput };
  });

  expect(result.pinnedBefore).toBe(true);
  expect(result.afterEcho).toBeLessThanOrEqual(2);
  expect(result.afterMoreOutput).toBeLessThanOrEqual(2);
});

test("wheel up releases immediately; scrolling back down re-engages", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "wheel-release-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    state.streamingMessages[session.id] = [
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text", text: "run" }],
        created_at: now,
      },
      assistantText("a1", 200),
    ];
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const pinnedBefore = distance() <= 2;

    // A wheel-up gesture releases the pin on the spot — before any scroll
    // movement, and even mid-churn when the geometry gate skips events.
    scroller.dispatchEvent(
      new WheelEvent("wheel", { deltaY: -120, bubbles: true, composed: true }),
    );
    await tick();
    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      assistantText("a2", 100),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const distanceWhileFree = distance();

    // The user scrolls back onto the bottom: the first scroll event absorbs
    // the geometry change, the second classifies the movement.
    scroller.dispatchEvent(new Event("scroll"));
    scroller.scrollTop = scroller.scrollHeight - scroller.clientHeight;
    scroller.dispatchEvent(new Event("scroll"));
    await tick();
    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      assistantText("a3", 100),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const distanceAfterReturn = distance();

    return { pinnedBefore, distanceWhileFree, distanceAfterReturn };
  });

  expect(result.pinnedBefore).toBe(true);
  expect(result.distanceWhileFree).toBeGreaterThan(120);
  expect(result.distanceAfterReturn).toBeLessThanOrEqual(2);
});

test("wheel up over a scrollable tool output does not release the pin", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "nested-wheel-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const userMessage = {
      id: "u1",
      type: "user" as const,
      content: [{ type: "text", text: "run" }],
      created_at: now,
    };
    const toolMessage = (id: string, lines: number) => ({
      id,
      type: "tool" as const,
      tool_call_id: `call-${id}`,
      tool_name: "shell",
      status: "completed" as const,
      arguments: "{}",
      result: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    // A tool card with a long output: expand it so its body becomes a
    // nested vertical scroller (max-h-96) inside the live activity group.
    state.streamingMessages[session.id] = [userMessage, toolMessage("t1", 100)];
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const toolToggle = target.querySelector<HTMLButtonElement>(
      "button[aria-expanded]:not([aria-label])",
    );
    if (!toolToggle) throw new Error("tool card toggle not found");
    toolToggle.click();
    await tick();
    const nested = target.querySelector<HTMLElement>(
      ".max-h-96.overflow-y-auto",
    );
    if (!nested) throw new Error("nested tool scroller not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;

    // The nested scroller can consume an upward wheel: the page must not
    // treat it as leaving the bottom.
    nested.scrollTop = 50;
    nested.dispatchEvent(
      new WheelEvent("wheel", { deltaY: -120, bubbles: true, composed: true }),
    );
    await tick();
    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      toolMessage("t2", 100),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const afterNestedWheel = distance();

    // At the nested scroller's own top the wheel chains to the page: that
    // IS leaving the bottom. Push several cards so the resulting growth
    // is large enough to tell "released" apart from "glued".
    nested.scrollTop = 0;
    nested.dispatchEvent(
      new WheelEvent("wheel", { deltaY: -120, bubbles: true, composed: true }),
    );
    await tick();
    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      ...Array.from({ length: 6 }, (_, i) => toolMessage(`t3-${i}`, 100)),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const afterChainedWheel = distance();

    return { afterNestedWheel, afterChainedWheel };
  });

  expect(result.afterNestedWheel).toBeLessThanOrEqual(2);
  expect(result.afterChainedWheel).toBeGreaterThan(120);
});


test("cold session entry holds the pin through the initial history load, autoScroll off", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { guiPreferences } = await import("/src/lib/settings.svelte.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    guiPreferences.chat.autoScroll = false;

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    // A cold session: no messages yet, history still loading (skeleton).
    // The entry pin must survive the load creating the tail — the initial
    // history is not "genuinely new content".
    const session = sessionLib.createSessionState({
      id: "cold-entry-test",
      phase: "idle",
      messages_loaded: false,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    sessionLib.loadSessionMessages(session.id, [
      {
        id: "u1",
        kind: "user",
        content: [{ type: "text", text: "hi" }],
        created_at: now,
      },
      {
        id: "a1",
        kind: "assistant",
        content: [{ type: "text", text: filler(300) }],
        created_at: now,
      },
    ]);
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const afterHistoryLoad = distance();

    // Only genuinely new content may release the entry pin.
    const live = state.sessionState.sessions.find((s) => s.id === session.id);
    if (!live) throw new Error("session not registered");
    live.messages.push({
      id: "a2",
      type: "assistant",
      content: [{ type: "text", text: filler(80) }],
      created_at: now,
    });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const afterNewContent = distance();

    return { afterHistoryLoad, afterNewContent };
  });

  expect(result.afterHistoryLoad).toBeLessThanOrEqual(2);
  expect(result.afterNewContent).toBeGreaterThan(2);
});

test("toggling the autoScroll preference does not yank a reader to the bottom", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { guiPreferences } = await import("/src/lib/settings.svelte.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "pref-toggle-test",
      phase: "idle",
      messages_loaded: true,
    });
    state.sessionState.sessions.push(session);
    const live = state.sessionState.sessions.find((s) => s.id === session.id);
    if (!live) throw new Error("session not registered");

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    live.messages.push(
      {
        id: "u1",
        type: "user",
        content: [{ type: "text", text: "hi" }],
        created_at: now,
      },
      {
        id: "a1",
        type: "assistant",
        content: [{ type: "text", text: filler(300) }],
        created_at: now,
      },
    );
    state.sessionState.activeSessionId = session.id;
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;

    // The user scrolls up to read.
    scroller.scrollTop = 0;
    scroller.dispatchEvent(new Event("scroll"));
    await tick();
    const distanceWhileReading = distance();

    // Flipping the preference is a settings change, not a navigation: it
    // must not re-fire the entry scroll.
    guiPreferences.chat.autoScroll = !guiPreferences.chat.autoScroll;
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));
    const afterToggle = distance();

    return { distanceWhileReading, afterToggle };
  });

  expect(result.distanceWhileReading).toBeGreaterThan(120);
  expect(result.afterToggle).toBeGreaterThan(120);
});


test("PageUp on the page releases the pin during streaming", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "key-release-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    state.streamingMessages[session.id] = [
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text", text: "run" }],
        created_at: now,
      },
      assistantText("a1", 200),
    ];
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const pinnedBefore = distance() <= 2;

    // Focus is on the page body: PageUp targets the message list.
    document.body.dispatchEvent(
      new KeyboardEvent("keydown", { key: "PageUp", bubbles: true }),
    );
    await tick();
    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      assistantText("a2", 100),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    return { pinnedBefore, distanceWhileFree: distance() };
  });

  expect(result.pinnedBefore).toBe(true);
  expect(result.distanceWhileFree).toBeGreaterThan(120);
});

test("a touch drag toward older messages releases the pin", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    // Some engines expose the Touch interface but forbid constructing it;
    // the handler is a touchscreen path, so skip there instead of faking
    // event shapes.
    try {
      new Touch({ identifier: 0, target: document.body, clientY: 0 });
    } catch {
      return { skipped: true };
    }

    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "touch-release-test",
      phase: "streaming",
      is_running: true,
    });
    state.sessionState.sessions.push(session);
    state.sessionState.activeSessionId = session.id;

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    const assistantText = (id: string, lines: number) => ({
      id,
      type: "assistant" as const,
      content: [{ type: "text", text: filler(lines) }],
      created_at: now,
    });

    state.streamingMessages[session.id] = [
      {
        id: "u1",
        type: "user" as const,
        content: [{ type: "text", text: "run" }],
        created_at: now,
      },
      assistantText("a1", 200),
    ];
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    const pinnedBefore = distance() <= 2;

    const touchAt = (y: number) =>
      new Touch({ identifier: 0, target: scroller, clientY: y });
    scroller.dispatchEvent(
      new TouchEvent("touchstart", { touches: [touchAt(300)], bubbles: true }),
    );
    scroller.dispatchEvent(
      new TouchEvent("touchmove", { touches: [touchAt(360)], bubbles: true }),
    );
    await tick();
    state.streamingMessages[session.id] = [
      ...state.streamingMessages[session.id]!,
      assistantText("a2", 100),
    ];
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    return { skipped: false, pinnedBefore, distanceWhileFree: distance() };
  });

  if (result.skipped) {
    test.skip(true, "Touch constructor unavailable in this engine");
    return;
  }
  expect(result.pinnedBefore).toBe(true);
  expect(result.distanceWhileFree).toBeGreaterThan(120);
});

test("toggling autoScroll off→on disarms the new-content release", async ({
  page,
}) => {
  await page.goto("/");

  const result = await page.evaluate(async () => {
    const { mount, tick } = await import("/@id/svelte");
    const state = await import("/src/lib/state.svelte.ts");
    const sessionLib = await import("/src/lib/session.ts");
    const { guiPreferences } = await import("/src/lib/settings.svelte.ts");
    const { default: MessageList } =
      await import("/src/lib/components/chat/MessageList.svelte");

    // Start with the preference OFF so the entry pin arms a release
    // signature, then flip it ON before new content arrives.
    guiPreferences.chat.autoScroll = false;

    const target = document.createElement("div");
    target.style.height = "600px";
    document.body.replaceChildren(target);

    const session = sessionLib.createSessionState({
      id: "pref-disarm-test",
      phase: "idle",
      messages_loaded: true,
    });
    state.sessionState.sessions.push(session);
    const live = state.sessionState.sessions.find((s) => s.id === session.id);
    if (!live) throw new Error("session not registered");

    const now = new Date().toISOString();
    const filler = (lines: number) =>
      Array.from({ length: lines }, (_, i) => `line ${i}`).join("\n");
    live.messages.push(
      {
        id: "u1",
        type: "user",
        content: [{ type: "text", text: "hi" }],
        created_at: now,
      },
      {
        id: "a1",
        type: "assistant",
        content: [{ type: "text", text: filler(300) }],
        created_at: now,
      },
    );
    state.sessionState.activeSessionId = session.id;
    mount(MessageList, { target });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    const scroller = target.querySelector<HTMLDivElement>(".overflow-y-auto");
    if (!scroller) throw new Error("scroll container not found");
    const distance = () =>
      scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;

    guiPreferences.chat.autoScroll = true;
    await tick();

    // Genuinely new content arrives: with the preference now ON the pin may
    // only end via user intent, so the growth must be glued.
    live.messages.push({
      id: "a2",
      type: "assistant",
      content: [{ type: "text", text: filler(80) }],
      created_at: now,
    });
    await tick();
    await new Promise(requestAnimationFrame);
    await new Promise((resolve) => setTimeout(resolve, 60));

    return { distanceAfterNewContent: distance() };
  });

  expect(result.distanceAfterNewContent).toBeLessThanOrEqual(2);
});
